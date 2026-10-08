//! 工具执行 —— `execute(name, args, ctx)` 一口分发（内置循环与 MCP 出口共用）。
//!
//! 错误两型（计划「工具清单」节）：[`ToolError::RespondToModel`] 是**业务错误**
//! （占点/未轮到/路径不存在/格式错），文本回给模型当工具结果（isError 语义），
//! 循环继续——模型读错误改下一步是正常工作方式；[`ToolError::Fatal`] 是**环境损坏**
//! （连接断/配置坏/存储不可用），循环终止进 error 态。**判断标准：模型能不能靠自己
//! 修复**——能，回模型；不能，致命。
//!
//! 路径解析规则（全工具共用）：`/game` → 动态合成器；`/game/in/*` → 暂存结构；
//! `/memory` → VfsStore；未知路径 → 错误并附 `/index` 提示。
//!
//! **grep 的正则引擎在本文件内手写**（`mini_regex`）：骨架依赖清单没有 regex crate
//! （「已含全部依赖」——不添依赖是纪律），而计划给 grep 的契约就是「pattern（正则）」。
//! 子集按工具描述承诺的语法给全：字面量/`.`/`*`/`+`/`?`/`{m,n}`/`[]`/`|`/`()`/
//! `^`/`$`/`\d\w\s` 与懒量化，编译成指令程序后**带回溯 + 已访问状态去重**的 VM 匹配
//! （去重让匹配多项式化，仍是纯「能否匹配」语义）；病态模式的步数上限兜底。

use std::sync::Arc;
#[cfg(feature = "mcp")]
use std::time::Duration;

use goptop_net::protocol::CoordT;
use goptop_net::session::UiCommand;

use crate::player::{EmitWatch, EventQueue, PlayerHandle};
use crate::store::{self, VfsStore, MAX_FILE_BYTES, MAX_NS_BYTES};
use crate::tools::{self, ToolDef};
use crate::vfs::{self, InFile, Resolved};
use crate::Driver;

/// submit 落地后的定拍时长（dispatch_submit 内部泵会话到对局规则生效）。
///
/// 固定短拍而非等对手：submit 的回执只关心**我这席**的执行结果（规则收没收下），
/// 对手的响应是 wait_events/事件推送的事。400ms ≈ headless.rs settle 的量级下限，
/// 本地状态机一拍即达，富余给快照重读。
pub const SUBMIT_SETTLE_MS: u64 = 400;

/// read 未给 limit 时的默认行数（长文件翻页的默认页幅；工具描述与实现同源）。
const DEFAULT_READ_LIMIT: usize = 2000;

/// wait_events 阻塞参数上限（计划 R4：≤120s 可续等；超限回模型让它用 120 重试）。
/// **仅 mcp**（阶段⑤ cfg 门）：wasm 不引 tokio select/`macros`，wait_events 是
/// MCP 桌面专属工具（内置模式事件自动推送、不做阻塞等待），整段不编译。
#[cfg(feature = "mcp")]
pub const WAIT_EVENTS_MAX_SECS: u64 = 120;

/// wait_events 阻塞期自泵与查队的节拍（对齐 PlayerHandle::wait_until 的 50ms 一拍）。
#[cfg(feature = "mcp")]
const WAIT_POLL_MS: u64 = 50;

/// grep 单次返回的匹配数上限（防一个宽泛 pattern 把上下文打爆；truncated 标志告知）。
const MAX_GREP_MATCHES: usize = 200;
/// grep 单行回传的字符上限（grid/history 行可能极长，模型要的是定位不是全文）。
const MAX_GREP_LINE_CHARS: usize = 240;

/// game_start 在「既无活局也无待局」时的业务错误文案（计划语义：人侧未就绪是
/// 正常流程，回业务错误文本、非 Fatal）。工具面（[`execute_game_start`]）与
/// mcp 出口的 handler 前置（mcp.rs）共用这一份，两处永不漂移。
pub const GAME_START_NO_GAME: &str = "no game to claim yet — the user starts the pairing from the AgentPage (kind/size/color are chosen there). Retry game_start once a game is being set up.";

/// 工具执行错误的两型。
#[derive(Debug, Clone)]
pub enum ToolError {
    /// 业务错误——文本原样回给模型（isError 文本），循环继续。
    RespondToModel(String),
    /// 致命——连接/配置损坏，循环终止进 error 态（文本进 agent_status.error）。
    Fatal(String),
}

impl ToolError {
    /// 取错误文本（循环回填 isError 与状态上报共用，避免 match 两遍）。
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::RespondToModel(msg) | Self::Fatal(msg) => msg,
        }
    }
}

/// 业务错误的简写（ RespondToModel 是绝对主流分支，一处构造器省得到处写全称）。
fn respond(msg: impl Into<String>) -> ToolError {
    ToolError::RespondToModel(msg.into())
}

/// 子代理执行钩子 —— delegate 的真实循环。
///
/// **为什么是钩子而不是 registry 自己跑**：[`ToolCtx`] 里没有 LLM 客户端（工具执行
/// 不该背决策层的依赖），而子代理=「同 LLM 独立上下文的小循环」——循环层（内置
/// agent_loop / 测试的 Mock）在装配 ctx 时把 runner 注入进来。契约（计划「工具清单」
/// delegate 行）：工具面只读 read/grep、深度 1 不许再嵌套（runner 给子循环的工具清单
/// 里没有 delegate，结构上杜绝）、独立小预算；成功回结论文本，失败回人话错误
/// （ RespondToModel 级——子代理失败父代理该知道原因并继续，不是环境损坏）。
// ?Send 界的取舍见 llm/mod.rs HttpChannel 注（wasm 走 spawn_local，future 非 Send）。
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SubagentRunner: Send + Sync {
    /// 跑一个只读子循环，回最终结论文本。
    ///
    /// # Errors
    /// 子循环失败（预算打满/LLM 不可用等）——文本回父代理。
    async fn run(&self, task: String) -> Result<String, String>;
}

/// 工具执行上下文 —— execute 需要的一切，由 AgentHub/循环装配一次。
///
/// 字段全是 `Arc`/句柄：同一个 ctx 在整局内被反复借用，深拷贝既慢又会撕裂一致性
/// （暂存区尤其不能有两份）。
pub struct ToolCtx {
    /// B 席（Agent 自己）——submit 的动作落点、read/grep 的快照来源。
    pub player: Arc<dyn PlayerHandle>,
    /// B 席的快照推送流（wait_events 的阻塞等待读它；内置模式不消费——
    /// 事件自动推送走 [`Self::events`] 的排空）。
    pub watch: EmitWatch,
    /// 事件队列（/game/events 的数据源；内置模式工具间隙自动排空）。
    pub events: Arc<EventQueue>,
    /// in/ 暂存区（write 落槽、submit 取槽、read 回看，全走这一份）。
    pub staging: Arc<vfs::Staging>,
    /// /memory 的存储后端（native=rusqlite；web=IndexedDB 钩子，阶段⑤）。
    pub memory: Arc<dyn VfsStore>,
    /// 记忆命名空间：内置=`builtin`、MCP=`mcp`（[`Driver::memory_ns`] 的预取值——
    /// execute 在热路径上，不值得每跳一次 enum）。
    pub memory_ns: &'static str,
    /// 驱动方式（wait_events/game_* 只对 Mcp 在册；工具清单过滤在循环装配时做，
    /// 这里只挡「外部 Agent 点名调用不在册工具」的越权口）。
    pub driver: Driver,
    /// 子代理开关（delegate 仅内置可选启用，默认关）。
    pub subagent_enabled: bool,
    /// delegate 的执行体（见 [`SubagentRunner`]；`None`=开关虽开但未装配——回模型
    /// 而非 Fatal，装配缺口不该炸掉整局）。
    pub subagent: Option<Arc<dyn SubagentRunner>>,
    /// 工具调用日志钩子：主循环每次 execute 后回调 (工具名, 是否成功, 耗时 ms,
    /// 操作行摘要, 失败/详情文本)。**全量记账**——成功失败都进环，submit 也不例外
    ///（事件流是唯一现场：失败的 submit 红色换行展示报错，模型与人都能看见）。
    /// `None`=静默（MCP 出口的回执面在 tools/call 响应上）。
    pub on_tool: Option<ToolLogHook>,
}

/// [`ToolCtx::on_tool`] 的类型：head=操作行（`Op(路径)` 的括号内容），
/// detail=第二行详情（失败报错 / 暂存内容头；None=不显示第二行）。
pub type ToolLogHook = Arc<dyn Fn(&str, bool, u64, &str, Option<&str>) + Send + Sync>;

/// submit 调用的暂存内容头（详情行用；要在 execute **之前**取——提交即清槽）。
pub(crate) fn staged_head(name: &str, args: &serde_json::Value, ctx: &ToolCtx) -> Option<String> {
    if name != crate::tools::TOOL_SUBMIT.name {
        return None;
    }
    let path = args.get("path").and_then(serde_json::Value::as_str)?;
    let file = classify_in(path)?;
    Some(ctx.staging.peek(file).unwrap_or_default().chars().take(80).collect())
}

impl Clone for ToolCtx {
    /// 字段全是 `Arc`/句柄/`&'static str`：克隆廉价且**共享同一局**（同一会话、
    /// 同一暂存区、同一事件队列）。mcp 出口每次 tools/call 从活局槽重建一份
    /// （槽里的 ToolCtx 不外借，借用跨不过 await），内置循环单份直用。
    fn clone(&self) -> Self {
        Self {
            player: Arc::clone(&self.player),
            watch: EmitWatch::new(self.watch.clone_rx()),
            events: Arc::clone(&self.events),
            staging: Arc::clone(&self.staging),
            memory: Arc::clone(&self.memory),
            memory_ns: self.memory_ns,
            driver: self.driver,
            subagent_enabled: self.subagent_enabled,
            subagent: self.subagent.clone(),
            on_tool: self.on_tool.clone(),
        }
    }
}

/// 参数摘要（工具日志用）：优先语义字段（path/pattern/task），其余压成短 JSON。
/// 这是给人看的进度条，不是参数回放——全文模型自己刚发过。
pub(crate) fn args_summary(args: &serde_json::Value) -> String {
    fn clip(s: &str) -> String {
        let t = s.trim();
        if t.chars().count() > 40 {
            format!("{}…", t.chars().take(40).collect::<String>())
        } else {
            t.to_string()
        }
    }
    if let Some(p) = args["pattern"].as_str() {
        // grep：pattern + 搜索范围（它的第二个参数也叫 path，组合才有意义）
        return match args["path"].as_str() {
            Some(scope) => clip(&format!("{p} @ {scope}")),
            None => clip(p),
        };
    }
    if let Some(p) = args["path"].as_str() {
        return clip(p);
    }
    if let Some(t) = args["task"].as_str() {
        return clip(t);
    }
    if let Some(n) = args["timeout_secs"].as_u64() {
        return format!("timeout={n}");
    }
    clip(&args.to_string())
}

/// 工具执行的一口分发。
///
/// 名字不在册（含「在册但本驱动不可用」——如内置模式点 wait_events）→
/// RespondToModel（列出可用工具名，模型自纠）；参数形状不合 schema →
/// RespondToModel；业务拒绝（占点/未轮到/只读路径写/配额超限）→ RespondToModel；
/// 环境损坏 → Fatal。返回值是**序列化好的回执 JSON**（计划样例的返回要点列），
/// 协议适配器只负责装进各自的工具结果壳，不再解释内容。
///
/// game_start/game_leave 的会话对生命周期接线在 MCP handler-state 层（阶段④）——
/// 本函数只做工具面语义，pair() 由 handler 直接调，不从这里穿针。
///
/// # Errors
/// 见 [`ToolError`] 两型的分工。
pub async fn execute(
    name: &str,
    args: &serde_json::Value,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let Some(def) = tools::by_name(name) else {
        return Err(respond(format!(
            "unknown tool \"{name}\". Available tools: {}.",
            available_tools(ctx)
        )));
    };
    if !in_scope(def, ctx) {
        return Err(respond(format!(
            "tool \"{name}\" is not available in this mode. Available tools: {}.",
            available_tools(ctx)
        )));
    }
    let Some(args) = args.as_object() else {
        return Err(respond("tool arguments must be a JSON object."));
    };
    match name {
        "read" => execute_read(args, ctx).await,
        "write" => execute_write(args, ctx).await,
        "submit" => execute_submit(args, ctx).await,
        "edit" => execute_edit(args, ctx).await,
        "grep" => execute_grep(args, ctx).await,
        "delegate" => execute_delegate(args, ctx).await,
        // 仅 mcp（阶段⑤ cfg 门）：见 WAIT_EVENTS_MAX_SECS 注——wasm 不编译本臂。
        #[cfg(feature = "mcp")]
        "wait_events" => execute_wait_events(args, ctx).await,
        "game_start" => execute_game_start(ctx).await,
        "game_leave" => execute_game_leave(ctx).await,
        // by_name 已证明名字在册，九个分支即全集；防御分支只是编译器的穷尽性保险。
        _ => Err(respond(format!("unknown tool \"{name}\"."))),
    }
}

/// 本驱动/开关组合下该工具是否在册（与 [`tools::tools_for`] 同一条过滤规则——
/// 清单给模型看的是它，执行口的越权拦截也是它，两处永不分叉）。
fn in_scope(def: &ToolDef, ctx: &ToolCtx) -> bool {
    match def.scope {
        tools::ToolScope::Shared => true,
        tools::ToolScope::BuiltinOnly => ctx.driver == Driver::Builtin && ctx.subagent_enabled,
        tools::ToolScope::McpOnly => ctx.driver == Driver::Mcp,
    }
}

/// 当前驱动可用的工具名清单（错误文案用——模型读了才知道还能点什么）。
fn available_tools(ctx: &ToolCtx) -> String {
    tools::tools_for(ctx.driver, ctx.subagent_enabled)
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/* ---------------- 参数取值助手（schema 校验的运行期一半） ---------------- */

/// 取一个必填字符串参数。
fn arg_str<'a>(
    args: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str, ToolError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Err(respond(format!(
            "missing required argument \"{key}\"."
        ))),
        Some(v) => v
            .as_str()
            .ok_or_else(|| respond(format!("argument \"{key}\" must be a string."))),
    }
}

/// 取一个可选的非负整数参数（read 的 offset/limit、wait_events 的 timeout_secs）。
fn arg_uint(
    args: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, ToolError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n)),
            None => Err(respond(format!(
                "argument \"{key}\" must be a non-negative integer."
            ))),
        },
    }
}

/* ---------------- read ---------------- */

async fn execute_read(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let path = arg_str(args, "path")?;
    let offset = arg_uint(args, "offset")?.unwrap_or(0) as usize;
    let limit = arg_uint(args, "limit")?.unwrap_or(DEFAULT_READ_LIMIT as u64) as usize;

    // in/ 槽单独走：read 回显暂存内容；空槽回一句话职责说明（取值协议在工具描述、
    // /index、这里三处冗余的最后一处）。
    if let Some(file) = classify_in(path) {
        let content = match ctx.staging.peek(file) {
            Some(text) => text,
            None => format!("(nothing staged yet) — {}", InFile::accepted(file)),
        };
        let (paged, total) = page(&content, offset, limit);
        return Ok(serde_json::json!({
            "ok": true, "path": path, "total_lines": total, "content": paged,
        }));
    }

    let resolved = vfs::resolve(path).map_err(respond)?;
    // 图像变体（/game/board/image.png、/game/history/<n>/image.png）：二进制不进
    // 文本读口——合成 PNG 后以 base64 附在回执的 image 字段。能力判定在消费侧：
    // 内置循环按 [`crate::llm::LlmClient::supports_image_result`] 分派（image block
    // 或占位文本），MCP 出口按同字段装 `type:"image"`。
    if vfs::is_image_file(&resolved) {
        let snap = ctx.player.snapshot();
        let (mime, bytes) =
            vfs::read_image(&resolved, &snap, staged_move_of(ctx)).map_err(respond)?;
        return Ok(serde_json::json!({
            "ok": true, "path": path,
            "image": { "mime": mime, "data_base64": base64_of(&bytes) },
        }));
    }

    let content = read_text_of(&resolved, ctx)?;
    let (paged, total) = page(&content, offset, limit);
    Ok(serde_json::json!({
        "ok": true, "path": path, "total_lines": total, "content": paged,
    }))
}

/// 全文取数（read 工具的严格版：任何失败都要回模型，带恢复线索）。
fn read_text_of(resolved: &Resolved, ctx: &ToolCtx) -> Result<String, ToolError> {
    match resolved {
        Resolved::Index => Ok(vfs::syn_index()),
        Resolved::Memory(rel) => {
            let key = store::normalize_path(rel).map_err(respond)?;
            ctx.memory
                .read(ctx.memory_ns, &key)
                .map_err(ToolError::Fatal)?
                .ok_or_else(|| {
                    respond(format!(
                        "file not found: /memory/{key}. grep(path:\"/memory\") lists what exists."
                    ))
                })
        }
        Resolved::Game(_) => {
            let snap = ctx.player.snapshot();
            let staged = staged_move_of(ctx);
            vfs::read_dynamic(resolved, &snap, staged, &ctx.events).map_err(respond)
        }
    }
}

/// 图像回执的取数口：read 命中图像变体时，回执带 `image{mime,data_base64}`。
/// 内置循环按协议能力分派（支持视觉 → 文本回执剥掉 base64 + 原生 image block；
/// 不支持 → [`crate::vfs::syn_image_placeholder`] 占位文本）；MCP 出口按同字段
/// 装 `type:"image"`——同一份回执喂三种协议，判定只写在这一处消费侧。
#[must_use]
pub fn image_part(receipt: &serde_json::Value) -> Option<(String, String)> {
    let mime = receipt["image"]["mime"].as_str()?.to_string();
    let data = receipt["image"]["data_base64"].as_str()?.to_string();
    Some((mime, data))
}

/// PNG 字节 → base64 文本（图像回执的线上形态；标准字母表含填充）。
fn base64_of(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/* ---------------- write ---------------- */

async fn execute_write(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let path = arg_str(args, "path")?;
    let content = arg_str(args, "content")?;

    // 第一优先：in/ 槽 = 暂存（只查格式回预览，不执行——规则错误留给 submit）。
    // **Write 只建新**（pi/claude code 语义，用户拍板 2026-10-08）：槽里已有暂存
    // 内容 → 报错改走 edit；提交后槽被清空，下一次 write 又是新建——这样每次
    // 重新考虑都留有 Edit 痕迹，盲覆盖写不进来。
    if let Some(file) = classify_in(path) {
        if ctx.staging.peek(file).is_some_and(|t| !t.is_empty()) {
            return Err(respond(format!(
                "{path} already has staged content — use edit on it to reconsider, or submit it first (submit clears the slot)."
            )));
        }
        return ctx.staging.stage(file, content).map_err(respond);
    }

    let resolved = vfs::resolve(path).map_err(respond)?;
    let Resolved::Memory(rel) = &resolved else {
        // /game 动态文件与 /index 只读（计划样例文案逐字，路径代入）。
        let hint = if path == "/index" { " (generated index)" } else { " (synthesized from live game state)" };
        return Err(respond(format!("{path} is read-only{hint}.")));
    };

    let key = store::normalize_path(rel).map_err(respond)?;
    // **Write 只建新**（claude code 语义）：/memory 已存在同名文件 → 改走 edit。
    if ctx.memory.read(ctx.memory_ns, &key).map_err(ToolError::Fatal)?.is_some() {
        return Err(respond(format!(
            "/memory/{key} already exists — use edit to change it (read it first if unsure)."
        )));
    }
    // 配额先查（RespondToModel 级），存储后写（Err 才是真故障 Fatal）。先读旧文件
    // 求净增量：覆盖写不该被旧内容占的额度二次计费。
    if content.len() > MAX_FILE_BYTES {
        return Err(respond(format!(
            "content is {} bytes; a /memory file is limited to {MAX_FILE_BYTES} bytes. Split it into several files.",
            content.len()
        )));
    }
    let old_len = ctx
        .memory
        .read(ctx.memory_ns, &key)
        .map_err(ToolError::Fatal)?
        .map_or(0, |c| c.len());
    let usage = ctx.memory.usage(ctx.memory_ns).map_err(ToolError::Fatal)?;
    let net = usage.saturating_sub(old_len as u64).saturating_add(content.len() as u64);
    if net > MAX_NS_BYTES as u64 {
        return Err(respond(format!(
            "this write would exceed the /memory quota ({net} > {MAX_NS_BYTES} bytes). Shrink or rewrite your existing files to free space."
        )));
    }
    ctx.memory.write(ctx.memory_ns, &key, content).map_err(ToolError::Fatal)?;
    Ok(serde_json::json!({
        "ok": true, "path": format!("/memory/{key}"), "bytes": content.len(), "usage": net,
    }))
}

/* ---------------- submit ---------------- */

async fn execute_submit(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let path = arg_str(args, "path")?;
    let Some(file) = classify_in(path) else {
        return Err(respond(format!(
            "submit only commits staged actions under /game/in/ (move/chat/request/confirm/score/resign), got \"{path}\". Write the slot first, then submit it. Note: /memory is your persistent long-term memory that survives across games and sessions — writes there are already live the moment you write them, and never need submit."
        )));
    };
    vfs::dispatch_submit(file, &*ctx.player, &ctx.staging, SUBMIT_SETTLE_MS)
        .await
        .map_err(respond)
}

/* ---------------- edit ---------------- */

async fn execute_edit(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let path = arg_str(args, "path")?;
    let old = arg_str(args, "old_string")?;
    let new = arg_str(args, "new_string")?;
    let replace_all = match args.get("replace_all") {
        None | Some(serde_json::Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| respond("argument \"replace_all\" must be a boolean."))?,
    };

    // in/ 槽可 edit：Write 只建新后（用户拍板 2026-10-08），重新考虑暂存内容的
    // 唯一路径就是 edit（claude code 语义：write 建、edit 改），复用同一 apply_edit。
    if let Some(file) = classify_in(path) {
        let Some(cur) = ctx.staging.peek(file).filter(|t| !t.is_empty()) else {
            return Err(respond(format!(
                "nothing staged at {path} — write the content first, then edit or submit it."
            )));
        };
        let (applied, occurrences) = apply_edit(&cur, &old, &new, replace_all).map_err(respond)?;
        ctx.staging.stage(file, &applied).map_err(respond)?;
        return Ok(serde_json::json!({
            "ok": true, "path": path, "occurrences": occurrences,
            "note": "staged content updated — submit(path) to commit",
        }));
    }
    let resolved = vfs::resolve(path).map_err(respond)?;
    let Resolved::Memory(rel) = &resolved else {
        return Err(respond(format!(
            "edit works on /memory files only; {path} is read-only (synthesized from live game state)."
        )));
    };
    let key = store::normalize_path(rel).map_err(respond)?;

    let content = ctx
        .memory
        .read(ctx.memory_ns, &key)
        .map_err(ToolError::Fatal)?
        .ok_or_else(|| {
            respond(format!(
                "file not found: /memory/{key}. edit never creates files — write it first."
            ))
        })?;

    let (updated, occurrences) = apply_edit(&content, old, new, replace_all).map_err(respond)?;

    // 与 write 同一条配额先查后写（edit 不该绕过额度）。
    let old_len = content.len();
    if updated.len() > MAX_FILE_BYTES {
        return Err(respond(format!(
            "result would be {} bytes; a /memory file is limited to {MAX_FILE_BYTES} bytes.",
            updated.len()
        )));
    }
    let usage = ctx.memory.usage(ctx.memory_ns).map_err(ToolError::Fatal)?;
    let net = usage.saturating_sub(old_len as u64).saturating_add(updated.len() as u64);
    if net > MAX_NS_BYTES as u64 {
        return Err(respond(format!(
            "this edit would exceed the /memory quota ({net} > {MAX_NS_BYTES} bytes). Shrink your files first."
        )));
    }
    ctx.memory.write(ctx.memory_ns, &key, &updated).map_err(ToolError::Fatal)?;
    Ok(serde_json::json!({
        "ok": true, "path": format!("/memory/{key}"), "occurrences": occurrences, "size": updated.len(),
    }))
}

/// Claude Code 语义的精确替换（纯函数，负例单测钉死）：
/// 空串拒；0 次拒；非 replace_all 且 >1 次拒；命中即全部（replace_all）或唯一处替换。
/// 回 (新内容, 实际替换次数)。
fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old_string is empty — provide the exact text to replace.".into());
    }
    let occurrences = content.matches(old).count();
    match occurrences {
        0 => Err(
            "old_string not found in the file (0 occurrences). Read the file and copy the exact text, including whitespace."
                .to_string(),
        ),
        n if n > 1 && !replace_all => Err(format!(
            "old_string matches {n} times — not unique. Add surrounding lines to make it unique, or set replace_all=true."
        )),
        n => {
            // str::replace 没有次数参数，replace_all=false 时 n==1，整体替换即唯一处替换。
            Ok((content.replace(old, new), n))
        }
    }
}

/* ---------------- grep ---------------- */

async fn execute_grep(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let pattern = arg_str(args, "pattern")?;
    let prefix = canonical_prefix(arg_str(args, "path")?);
    let re = MiniRegex::new(pattern).map_err(|e| respond(format!("invalid regex pattern: {e}")))?;

    let snap = ctx.player.snapshot();
    let mut matches: Vec<serde_json::Value> = Vec::new();
    let mut truncated = false;

    // 收集候选：/index、/game 全树（history/<n> 按手数展开）、/memory 前缀树。
    let mut candidates: Vec<String> = Vec::new();
    if path_covers(&prefix, "/index") {
        candidates.push("/index".into());
    }
    for &p in GREP_GAME_FILES.iter().chain(GREP_IN_FILES.iter()) {
        if path_covers(&prefix, p) {
            candidates.push(p.to_string());
        }
    }
    if path_covers(&prefix, "/game/history") {
        let move_count = snap["history"].as_array().map_or(0, |h| h.len());
        for n in 1..=move_count {
            let p = format!("/game/history/{n}");
            // 逐个过前缀：深前缀（如 /game/history/3）只搜那一个局面，
            // 不把 1..N 全部合成一遍（既费时又混入超范围匹配）。
            if path_covers(&prefix, &p) {
                candidates.push(p);
            }
        }
    }

    for path in &candidates {
        if truncated {
            break;
        }
        let Some(text) = grep_file_text(path, ctx, &snap) else { continue };
        truncated = collect_matches(path, &text, &re, &mut matches);
    }

    if path_covers(&prefix, "/memory") {
        let rel_prefix = prefix
            .strip_prefix("/memory")
            .unwrap_or("")
            .trim_matches('/');
        // 清单失败=存储不可用（Fatal 级），与 read/write 的记忆分支同一判型——
        // 静默当「无匹配」会让模型把存储故障误读成「没写过笔记」。
        let keys = ctx.memory.list(ctx.memory_ns, rel_prefix).map_err(ToolError::Fatal)?;
        for key in keys {
            if truncated {
                break;
            }
            let Ok(Some(text)) = ctx.memory.read(ctx.memory_ns, &key) else { continue };
            truncated = collect_matches(&format!("/memory/{key}"), &text, &re, &mut matches);
        }
    }

    Ok(serde_json::json!({
        "ok": true, "pattern": pattern,
        "matches": matches, "total": matches.len(), "truncated": truncated,
    }))
}

/// grep 扫描的 /game 固定文件清单（不含 image.png——二进制不进文本 grep）。
const GREP_GAME_FILES: [&str; 9] = [
    "/game/status",
    "/game/board",
    "/game/board/grid",
    "/game/board/ascii",
    "/game/board/pretty",
    "/game/rules",
    "/game/history",
    "/game/chat",
    "/game/events",
];
/// grep 扫描的 in/ 槽（暂存内容也该能被搜到——「我刚才暂存了什么来着」是真实问题）。
const GREP_IN_FILES: [&str; 6] = [
    "/game/in/move",
    "/game/in/chat",
    "/game/in/request",
    "/game/in/confirm",
    "/game/in/score",
    "/game/in/resign",
];

/// 前缀覆盖判定：path 恰为前缀本身，或在前缀的目录之下（`/game/history` 覆盖
/// `/game/history` 与 `/game/history/3`，但不覆盖 `/game/historyfoo`）。
fn path_covers(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

/// 用户输入的前缀规范化：去空白、补前导 `/`；空串与 `/` 归一为根（搜全部）。
fn canonical_prefix(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "/".into();
    }
    if t.starts_with('/') {
        t.trim_end_matches('/').to_string()
    } else {
        format!("/{}", t.trim_end_matches('/'))
    }
}

/// grep 的容错取文：单文件失败/空内容/二进制一律跳过（搜索不该被个别文件炸停）。
/// 与 read 的严格版（[`read_full_text`]）分离——两处的错误语义本来就不同。
fn grep_file_text(path: &str, ctx: &ToolCtx, snap: &serde_json::Value) -> Option<String> {
    if path.ends_with("/image.png") {
        return None;
    }
    if let Some(file) = classify_in(path) {
        return ctx.staging.peek(file).filter(|t| !t.is_empty());
    }
    let resolved = vfs::resolve(path).ok()?;
    Some(match &resolved {
        Resolved::Index => vfs::syn_index(),
        Resolved::Memory(rel) => {
            let key = store::normalize_path(rel).ok()?;
            ctx.memory.read(ctx.memory_ns, &key).ok()??
        }
        Resolved::Game(_) => {
            vfs::read_dynamic(&resolved, snap, staged_move_of(ctx), &ctx.events).ok()?
        }
    })
    .filter(|t| !t.is_empty())
}

/// 逐行匹配并累入结果；返回是否触顶（调用方停止后续文件）。
fn collect_matches(
    path: &str,
    text: &str,
    re: &MiniRegex,
    out: &mut Vec<serde_json::Value>,
) -> bool {
    for (i, line) in text.lines().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        out.push(serde_json::json!({
            "path": path, "line": i + 1, "text": clip_line(line),
        }));
        if out.len() >= MAX_GREP_MATCHES {
            return true;
        }
    }
    false
}

/// 单行截断（字符数计，不撕裂 UTF-8；省略号提示有删节）。
fn clip_line(line: &str) -> String {
    if line.chars().count() <= MAX_GREP_LINE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_GREP_LINE_CHARS).collect();
    format!("{head}…")
}

/* ---------------- delegate ---------------- */

async fn execute_delegate(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let task = arg_str(args, "task")?;
    // scope 门已保证 Builtin + subagent_enabled；runner 未装配是装配缺口，回模型说明。
    let Some(runner) = ctx.subagent.as_ref() else {
        return Err(respond(
            "delegate is enabled but no subagent runner is wired into this session.",
        ));
    };
    let conclusion = runner.run(task.to_string()).await.map_err(respond)?;
    Ok(serde_json::json!({ "ok": true, "conclusion": conclusion }))
}

/* ---------------- wait_events（仅 mcp；见 WAIT_EVENTS_MAX_SECS 注） ---------------- */

#[cfg(feature = "mcp")]
async fn execute_wait_events(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    let timeout_secs = arg_uint(args, "timeout_secs")?.unwrap_or(0);
    if timeout_secs > WAIT_EVENTS_MAX_SECS {
        return Err(respond(format!(
            "timeout_secs must be 0..={WAIT_EVENTS_MAX_SECS} (call again with 120 to keep waiting)."
        )));
    }
    // 先排空：队列里已有事件就瞬间全量返回——「等待」不牺牲已就绪的事件。
    if ctx.events.pending() > 0 {
        return Ok(drain_events(ctx));
    }
    if timeout_secs == 0 {
        return Ok(serde_json::Value::Array(Vec::new()));
    }

    // 阻塞等：watch 每拍（对手动作/聊天/请求都会推新拍）后查队；自泵节拍兜底
    // 手动泵的宿主（测试）。局散（sender drop）= 返回剩余事件（可能空）。
    let mut rx = ctx.watch.clone_rx();
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        let wait = (deadline - now).min(Duration::from_millis(WAIT_POLL_MS));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {
                ctx.player.pump();
                if ctx.events.pending() > 0 {
                    return Ok(drain_events(ctx));
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    return Ok(drain_events(ctx));
                }
                ctx.player.pump();
                if ctx.events.pending() > 0 {
                    return Ok(drain_events(ctx));
                }
            }
        }
    }
}

/// 排空事件队列并序列化成回执数组（空队列= `[]`，wait_events 的空超时语义建立于此）。
#[cfg(feature = "mcp")]
fn drain_events(ctx: &ToolCtx) -> serde_json::Value {
    let drained = ctx.events.drain();
    serde_json::to_value(&drained).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

/* ---------------- game_start / game_leave（工具面语义；pair 接线在阶段④ handler） ---------------- */

async fn execute_game_start(ctx: &ToolCtx) -> Result<serde_json::Value, ToolError> {
    let snap = ctx.player.snapshot();
    match snap["phase"].as_str() {
        Some("playing") => Ok(serde_json::json!({
            "ok": true, "started": true, "my_color": snap["myColor"],
        })),
        Some("waiting") => Ok(serde_json::json!({
            "ok": true, "started": false, "my_color": snap["myColor"],
            "note": "pairing in progress — the human side is connecting; call again shortly.",
        })),
        // 计划：人侧未就绪 → 业务错误文本（不是 Fatal——等用户点开始是正常流程）。
        _ => Err(respond(GAME_START_NO_GAME.to_string())),
    }
}

async fn execute_game_leave(ctx: &ToolCtx) -> Result<serde_json::Value, ToolError> {
    let snap = ctx.player.snapshot();
    let live = snap["phase"].as_str() == Some("playing") && snap["winner"].is_null();
    if !live {
        return Ok(serde_json::json!({
            "ok": true, "resigned": false, "note": "no live game — nothing to resign.",
        }));
    }
    // 局中离场=先认输（对局规则要求终局有因），配对随局散（handler 层拆）。
    ctx.player.cmd(UiCommand::Resign);
    settle(&*ctx.player, SUBMIT_SETTLE_MS).await;
    let after = ctx.player.snapshot();
    Ok(serde_json::json!({
        "ok": true, "resigned": true, "winner": after["winner"],
    }))
}

/// 自泵定拍（game_leave 认输后的等待；submit 的 settle 在 dispatch_submit 内部做）。
/// 50ms 一拍、先泵后看，与 PlayerHandle 的条件等待同一手法；歇拍走
/// [`crate::time_compat::delay`]（wasm 上没有 tokio time）。
async fn settle(player: &dyn PlayerHandle, ms: u64) {
    let rounds = (ms / 50).max(1);
    for _ in 0..rounds {
        player.pump();
        crate::time_compat::delay(50).await;
    }
    player.pump();
}

/* ---------------- 路径判别与暂存读取（registry 私有，单测钉死） ---------------- */

/// in/ 路径判别表 —— submit 的动作分发表、read/write/grep 的暂存分支共用。
///
/// **为什么 registry 自带一张表而不是调 [`InFile::from_path`]**：from_path 是 vfs 的
/// 契约，但其实现归 vfs 负责人；本模块的全部单测必须不依赖别人模块的 todo 体即可
/// 跑绿。六条字面量与计划「一切皆文件」节逐字一致（模型忘写前导 `/` 同样接受——
/// 与 resolve 的宽容一致），前缀之外的任何形态都拒绝。
fn classify_in(path: &str) -> Option<InFile> {
    let t = path.trim().trim_start_matches('/');
    match t {
        "game/in/move" => Some(InFile::Move),
        "game/in/chat" => Some(InFile::Chat),
        "game/in/request" => Some(InFile::Request),
        "game/in/confirm" => Some(InFile::Confirm),
        "game/in/score" => Some(InFile::Score),
        "game/in/resign" => Some(InFile::Resign),
        _ => None,
    }
}

/// 暂存着法文本 → 盘面坐标（合成器的 `staged` 参数取数）。
///
/// "pass" 不是盘面位置（无幽灵子可画）→ None；格式残缺同样 None——暂存槽的内容
/// 已在 write 时过格式校验，这里遇到残缺只可能发生在槽被绕过写入的防御路径，
/// 静默不画比炸掉合成更安全。越界坐标放行：越界是对局规则，submit 才判。
fn parse_staged_move(text: &str) -> Option<CoordT> {
    let t = text.trim();
    if t.is_empty() || t == "pass" {
        return None;
    }
    let (x, y) = t.split_once(',')?;
    let x = x.trim().parse::<u16>().ok()?;
    let y = y.trim().parse::<u16>().ok()?;
    Some(CoordT { x, y })
}

/// 当前暂存着法（动态合成统一入口；无暂存/暂存 pass → None）。
fn staged_move_of(ctx: &ToolCtx) -> Option<CoordT> {
    ctx.staging.peek(InFile::Move).and_then(|t| parse_staged_move(&t))
}

/// 行翻页：offset=0 起始行号（0-based）、limit=最多行数；回 (页文本, 全文行数)。
/// 尾随换行不算一行（`lines()` 语义），空内容 = 0 行。
fn page(content: &str, offset: usize, limit: usize) -> (String, usize) {
    let total = content.lines().count();
    let paged: Vec<&str> = content.lines().skip(offset).take(limit).collect();
    (paged.join("\n"), total)
}

/* ---------------- mini_regex：手写正则（语法子集见模块注） ---------------- */

/// 单条指令。编译自 [`Ast`]，[`MiniRegex::run_at`] 的回溯 VM 消费。
#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any,
    Class { neg: bool, items: Vec<ClassItem> },
    Start,
    End,
    /// 匹配成功（程序只用于 is_match，无捕获组回填）。
    Match,
    Jmp(usize),
    /// 两个续点：VM 栈先弹 `first`（贪心）或 `second`（懒）。
    Split(usize, usize),
}

/// 字符类成员项（`\d`/`\w`/`\s` 及其否定在类内外同形，neg 翻转语义）。
#[derive(Debug, Clone)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

fn class_has(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|it| match it {
        ClassItem::Ch(x) => *x == c,
        ClassItem::Range(a, b) => *a <= c && c <= *b,
        ClassItem::Digit(neg) => c.is_ascii_digit() != *neg,
        ClassItem::Word(neg) => (c.is_alphanumeric() || c == '_') != *neg,
        ClassItem::Space(neg) => c.is_whitespace() != *neg,
    })
}

/// 语法树（仅编译期形态，编译进 [`Inst`] 程序后即弃）。
#[derive(Debug, Clone)]
enum Ast {
    Char(char),
    Any,
    Class { neg: bool, items: Vec<ClassItem> },
    Start,
    End,
    Seq(Vec<Ast>),
    Alt(Vec<Ast>),
    Repeat { node: Box<Ast>, min: u32, max: Option<u32>, greedy: bool },
}

/// 展开上限：`a{0,99999}` 会把程序撑爆，超限直接报错（描述承诺的是实用子集）。
const MAX_REPETITION: u32 = 1000;
/// 单行匹配的步数上限（已访问状态去重后本就多项式，此值只兜极端构造）。
const MAX_MATCH_STEPS: u32 = 1_000_000;

/// 手写正则（grep 专用）——parse → compile → 回溯 VM，见模块注的子集清单。
#[derive(Debug, Clone)]
pub struct MiniRegex {
    prog: Vec<Inst>,
}

impl MiniRegex {
    /// 编译一个模式。
    ///
    /// # Errors
    /// 语法错误（括号/方括号不闭合、悬空量词、坏 `{m,n}`、尾反斜杠、重复数超限），
    /// 文本给人话、可直接回模型。
    pub fn new(pattern: &str) -> Result<Self, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { s: &chars, i: 0 };
        let ast = p.parse_alt()?;
        if p.i < p.s.len() {
            return Err(format!("unexpected '{}' at position {}", p.s[p.i], p.i));
        }
        let mut prog = Vec::new();
        compile(&ast, &mut prog);
        prog.push(Inst::Match);
        Ok(Self { prog })
    }

    /// 整行内是否存在匹配（行级 grep：`^`/`$` 即行首/行尾）。
    #[must_use]
    pub fn is_match(&self, line: &str) -> bool {
        let s: Vec<char> = line.chars().collect();
        // 已访问矩阵跨起点复用：从 (pc,sp) 出发的可达结局与怎么走到这无关——
        // 探过没匹配成功的状态不必再探（这也是复杂度从指数降多项式的关键）。
        let mut visited = vec![vec![false; s.len() + 1]; self.prog.len()];
        for start in 0..=s.len() {
            if self.run_at(&s, start, &mut visited) {
                return true;
            }
        }
        false
    }

    fn run_at(&self, s: &[char], start: usize, visited: &mut [Vec<bool>]) -> bool {
        let mut stack: Vec<(usize, usize)> = vec![(0, start)];
        let mut steps = 0u32;
        while let Some((pc, sp)) = stack.pop() {
            steps += 1;
            if steps > MAX_MATCH_STEPS {
                return false;
            }
            if visited[pc][sp] {
                continue;
            }
            visited[pc][sp] = true;
            match self.prog[pc] {
                Inst::Char(c) => {
                    if s.get(sp) == Some(&c) {
                        stack.push((pc + 1, sp + 1));
                    }
                }
                Inst::Any => {
                    if matches!(s.get(sp), Some(&c) if c != '\n') {
                        stack.push((pc + 1, sp + 1));
                    }
                }
                Inst::Class { neg, ref items } => {
                    if matches!(s.get(sp), Some(&c) if class_has(items, c) != neg) {
                        stack.push((pc + 1, sp + 1));
                    }
                }
                Inst::Start => {
                    if sp == 0 {
                        stack.push((pc + 1, sp));
                    }
                }
                Inst::End => {
                    if sp == s.len() {
                        stack.push((pc + 1, sp));
                    }
                }
                Inst::Match => return true,
                Inst::Jmp(t) => stack.push((t, sp)),
                Inst::Split(a, b) => {
                    // 后进先出：a 是优先续点（贪心放 a，懒放 b——编译期已排好）。
                    stack.push((b, sp));
                    stack.push((a, sp));
                }
            }
        }
        false
    }
}

/// 递归下降解析器（alt → concat → postfix → atom 四层）。
struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn parse_alt(&mut self) -> Result<Ast, String> {
        let mut branches = vec![self.parse_concat()?];
        while self.peek() == Some('|') {
            self.i += 1;
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Ast::Alt(branches)
        })
    }

    fn parse_concat(&mut self) -> Result<Ast, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.parse_atom()?;
            items.push(self.parse_postfix(atom)?);
        }
        Ok(match items.len() {
            0 => Ast::Seq(Vec::new()),
            1 => items.remove(0),
            _ => Ast::Seq(items),
        })
    }

    /// 原子后的量词后缀（可叠：`a+?` 是懒 plus、`a{2}{3}` 无意义但合法地叠）。
    fn parse_postfix(&mut self, atom: Ast) -> Result<Ast, String> {
        let mut node = atom;
        loop {
            let (min, max) = match self.peek() {
                Some('*') => {
                    self.i += 1;
                    (0, None)
                }
                Some('+') => {
                    self.i += 1;
                    (1, None)
                }
                Some('?') => {
                    self.i += 1;
                    (0, Some(1))
                }
                Some('{') => match self.parse_brace()? {
                    Some((min, max)) => (min, max),
                    None => break,
                },
                _ => break,
            };
            let greedy = if self.peek() == Some('?') {
                self.i += 1;
                false
            } else {
                true
            };
            node = Ast::Repeat { node: Box::new(node), min, max, greedy };
        }
        Ok(node)
    }

    /// `{m}` / `{m,}` / `{m,n}`；返回 None = 不是量词而是字面 `{`（保留给不转义的
    /// 字面花括号，如时间戳 `10{2}` 之外的普通文本）。
    fn parse_brace(&mut self) -> Result<Option<(u32, Option<u32>)>, String> {
        let save = self.i;
        self.i += 1; // '{'
        let Some(min) = self.parse_num() else {
            self.i = save;
            return Ok(None);
        };
        let max = if self.peek() == Some(',') {
            self.i += 1;
            if self.peek() == Some('}') {
                None
            } else {
                Some(self.parse_num().ok_or_else(|| "bad repetition {m,n}".to_string())?)
            }
        } else {
            Some(min)
        };
        if self.peek() != Some('}') {
            self.i = save;
            return Ok(None);
        }
        self.i += 1;
        if let Some(mx) = max {
            if mx < min {
                return Err(format!("repetition {{{min},{mx}}}: max < min"));
            }
        }
        if min > MAX_REPETITION || max.is_some_and(|m| m > MAX_REPETITION) {
            return Err(format!("repetition count over {MAX_REPETITION} is not supported"));
        }
        Ok(Some((min, max)))
    }

    fn parse_num(&mut self) -> Option<u32> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        self.s[start..self.i].iter().collect::<String>().parse().ok()
    }

    fn parse_atom(&mut self) -> Result<Ast, String> {
        match self.next() {
            None => Err("unexpected end of pattern".into()),
            Some('(') => {
                let inner = self.parse_alt()?;
                if self.next() != Some(')') {
                    return Err("unclosed group '('".into());
                }
                Ok(inner)
            }
            Some(')') => Err("unmatched ')'".into()),
            Some('[') => self.parse_class(),
            Some('.') => Ok(Ast::Any),
            Some('^') => Ok(Ast::Start),
            Some('$') => Ok(Ast::End),
            Some(c @ ('*' | '+' | '?')) => Err(format!("dangling quantifier '{c}'")),
            Some('\\') => self.parse_escape(),
            Some(c) => Ok(Ast::Char(c)),
        }
    }

    fn parse_escape(&mut self) -> Result<Ast, String> {
        match self.next() {
            None => Err("trailing backslash".into()),
            Some('d') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Digit(false)] }),
            Some('D') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Digit(true)] }),
            Some('w') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Word(false)] }),
            Some('W') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Word(true)] }),
            Some('s') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Space(false)] }),
            Some('S') => Ok(Ast::Class { neg: false, items: vec![ClassItem::Space(true)] }),
            Some('n') => Ok(Ast::Char('\n')),
            Some('t') => Ok(Ast::Char('\t')),
            Some('r') => Ok(Ast::Char('\r')),
            // 其余转义一律取字面量（\. \* \\ \[ …）；未知字母转义同样按字面，
            // 宽容于严格——模型写 \q 的代价是匹配 "q"，不是一次工具报错。
            Some(c) => Ok(Ast::Char(c)),
        }
    }

    /// `[...]` 字符类（进此函数时 `[` 已消费）：首 `^` 否定；`]` 紧随 `[`/`[^` 时是
    /// 字面量；支持 `a-z` 区间与类内 `\d\w\s` 转义。
    fn parse_class(&mut self) -> Result<Ast, String> {
        let neg = if self.peek() == Some('^') {
            self.i += 1;
            true
        } else {
            false
        };
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let lo = self.next().ok_or("unclosed character class '['")?;
            if lo == ']' && !first {
                break;
            }
            first = false;
            let lo = if lo == '\\' {
                match self.next() {
                    None => return Err("trailing backslash".into()),
                    Some('d') => {
                        items.push(ClassItem::Digit(false));
                        continue;
                    }
                    Some('D') => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    Some('w') => {
                        items.push(ClassItem::Word(false));
                        continue;
                    }
                    Some('W') => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    Some('s') => {
                        items.push(ClassItem::Space(false));
                        continue;
                    }
                    Some('S') => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r',
                    Some(e) => e,
                }
            } else {
                lo
            };
            // 区间：`-` 且后面不是 `]`（`[a-]` 的 `-` 是字面量）。
            if self.peek() == Some('-') && self.s.get(self.i + 1) != Some(&']') {
                self.i += 1;
                let hi = self.next().ok_or("unclosed character class '['")?;
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Ch(lo));
            }
        }
        Ok(Ast::Class { neg, items })
    }
}

/// AST → 指令程序（拼接直排；分支/量词用 Split/Jmp 回填）。
fn compile(ast: &Ast, prog: &mut Vec<Inst>) {
    match ast {
        Ast::Char(c) => prog.push(Inst::Char(*c)),
        Ast::Any => prog.push(Inst::Any),
        Ast::Class { neg, items } => prog.push(Inst::Class { neg: *neg, items: items.clone() }),
        Ast::Start => prog.push(Inst::Start),
        Ast::End => prog.push(Inst::End),
        Ast::Seq(items) => items.iter().for_each(|i| compile(i, prog)),
        Ast::Alt(branches) => {
            // b0 | b1 | b2：每个非末分支前放 Split(本分支, 下分支起点)，分支尾 Jmp 到总出口。
            let mut jumps = Vec::new();
            for (i, b) in branches.iter().enumerate() {
                if i + 1 < branches.len() {
                    let split_pc = prog.len();
                    prog.push(Inst::Jmp(0)); // 占位
                    compile(b, prog);
                    jumps.push(prog.len());
                    prog.push(Inst::Jmp(0)); // 占位
                    let next = prog.len();
                    prog[split_pc] = Inst::Split(split_pc + 1, next);
                } else {
                    compile(b, prog);
                }
            }
            let end = prog.len();
            for j in jumps {
                prog[j] = Inst::Jmp(end);
            }
        }
        Ast::Repeat { node, min, max, greedy } => {
            for _ in 0..*min {
                compile(node, prog);
            }
            match max {
                None => {
                    // 星号：Split(体, 出口)（懒则反转优先），体尾 Jmp 回 Split。
                    let split_pc = prog.len();
                    prog.push(Inst::Jmp(0)); // 占位
                    compile(node, prog);
                    prog.push(Inst::Jmp(split_pc));
                    let after = prog.len();
                    prog[split_pc] = if *greedy {
                        Inst::Split(split_pc + 1, after)
                    } else {
                        Inst::Split(after, split_pc + 1)
                    };
                }
                Some(n) => {
                    // 有界：再叠 (n-min) 个可选副本（每副本一个 Split），无回边、零空转环。
                    let mut splits = Vec::new();
                    for _ in *min..*n {
                        let sp_pc = prog.len();
                        prog.push(Inst::Jmp(0)); // 占位
                        splits.push(sp_pc);
                        compile(node, prog);
                    }
                    let after = prog.len();
                    for sp in splits {
                        prog[sp] = if *greedy {
                            Inst::Split(sp + 1, after)
                        } else {
                            Inst::Split(after, sp + 1)
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::GameEvent;

    /* ---- 正则引擎（grep 的承诺面） ---- */

    #[test]
    fn 正则_字面量类与锚点_重复() {
        let m = |p: &str, line: &str| MiniRegex::new(p).expect(p).is_match(line);
        assert!(m("7,7", "move to (7,7) please"));
        assert!(!m("8,8", "move to (7,7) please"));
        assert!(m("^ab", "abc"));
        assert!(!m("^ab", "xabc"));
        assert!(m("c$", "abc"));
        assert!(!m("c$", "cab"));
        assert!(m("[a-c]x", "bx"));
        assert!(!m("[a-c]x", "dx"));
        assert!(m("[^0-9]x", "ax"));
        assert!(!m("[^0-9]x", "5x"));
        assert!(m("a|bc", "zbc"));
        assert!(!m("a|bc", "zb"));
        assert!(m(r"\d{2,3}", "x123"));
        assert!(!m(r"\d{2,3}", "x1"));
        assert!(m("colou?r", "color"));
        assert!(m("colou?r", "colour"));
        assert!(m(r"\w+@\w+\.com", "mail a@b.com here"));
        assert!(m("(ab)+c", "ababc"));
        assert!(!m("(ab)+c", "ac"));
        assert!(m("a.c", "abc"));
        assert!(!m("a.c", "ac"));
        assert!(m("", "anything"), "空模式恒匹配");
        // 类内转义与首 ] 字面量。
        assert!(m(r"[\d]+", "42"));
        assert!(m("[]]x", "]x"));
        // 懒量化语法合法（is_match 布尔语义与贪心相同）。
        assert!(MiniRegex::new("<.+?>").is_ok());
    }

    #[test]
    fn 正则_非法模式报错() {
        for bad in ["a(", "a)", "[ab", "a\\", "*a", "+a", "a{2,1}", "a{0,99999}"] {
            assert!(MiniRegex::new(bad).is_err(), "模式 {bad} 应报错");
        }
    }

    /* ---- submit 路径判别表（计划「工具清单」submit 行） ---- */

    #[test]
    fn submit_路径判别表() {
        let cases = [
            ("/game/in/move", InFile::Move),
            ("game/in/chat", InFile::Chat),
            ("/game/in/request", InFile::Request),
            ("/game/in/confirm", InFile::Confirm),
            ("/game/in/score", InFile::Score),
            ("/game/in/resign", InFile::Resign),
        ];
        for (path, expect) in cases {
            assert_eq!(classify_in(path), Some(expect), "{path} 判别错误");
        }
        for bad in [
            "", "/game", "/game/in", "/game/in/moving", "/game/in/move/x",
            "/game/board", "/memory/x", "GAME/IN/MOVE", "/game/in/move/",
        ] {
            assert_eq!(classify_in(bad), None, "{bad} 不应判为控制文件");
        }
    }

    /* ---- edit 唯一性（Claude Code 语义负例） ---- */

    #[test]
    fn edit_唯一性负例与替换() {
        assert_eq!(
            apply_edit("hello world", "world", "there", false),
            Ok(("hello there".into(), 1))
        );
        // 0 次：拒。
        assert!(apply_edit("abc", "x", "y", false).is_err());
        // >1 次且未 replace_all：拒（不唯一）。
        assert!(apply_edit("aaa", "a", "b", false).is_err());
        // replace_all=true：全替换并报实数。
        assert_eq!(apply_edit("aaa", "a", "b", true), Ok(("bbb".into(), 3)));
        assert_eq!(apply_edit("abab", "ab", "cd", true), Ok(("cdcd".into(), 2)));
        // 空串 old_string：拒（否则到处匹配）。
        assert!(apply_edit("abc", "", "y", false).is_err());
        // 替换文本含原文（自我嵌套）也能收敛。
        assert_eq!(apply_edit("aa", "a", "aa", true), Ok(("aaaa".into(), 2)));
    }

    /* ---- 翻页 / 截断 / 暂存着法解析 / 错误文本 / 事件形状 ---- */

    #[test]
    fn 翻页_offset_limit_total_lines() {
        assert_eq!(page("a\nb\nc", 0, 2), ("a\nb".into(), 3));
        assert_eq!(page("a\nb\nc", 1, 2), ("b\nc".into(), 3));
        assert_eq!(page("a\nb\nc", 5, 2), (String::new(), 3));
        assert_eq!(page("", 0, 10), (String::new(), 0));
        assert_eq!(page("a\n", 0, 10), ("a".into(), 1), "尾随换行不算一行");
    }

    #[test]
    fn 暂存着法解析() {
        assert_eq!(parse_staged_move("8,8"), Some(CoordT { x: 8, y: 8 }));
        assert_eq!(parse_staged_move(" 7,7 "), Some(CoordT { x: 7, y: 7 }));
        assert_eq!(parse_staged_move("pass"), None);
        assert_eq!(parse_staged_move(""), None);
        assert_eq!(parse_staged_move("abc"), None);
        assert_eq!(parse_staged_move("7"), None);
        assert_eq!(parse_staged_move("7,7,7"), None);
        assert_eq!(parse_staged_move("-1,2"), None);
        // 越界坐标格式合法（越界是对局规则，submit 才判）。
        assert_eq!(parse_staged_move("700,700"), Some(CoordT { x: 700, y: 700 }));
    }

    #[test]
    fn 前缀覆盖判定() {
        assert!(path_covers("/game", "/game/status"));
        assert!(path_covers("/game/history", "/game/history/3"));
        assert!(path_covers("/game/history", "/game/history"));
        assert!(!path_covers("/game/history", "/game/historyfoo"));
        assert!(!path_covers("/memory/notes", "/memory/other"));
        assert!(path_covers("/", "/anything"));
        assert_eq!(canonical_prefix(" game/history "), "/game/history");
        assert_eq!(canonical_prefix(""), "/");
        assert_eq!(canonical_prefix("/memory/"), "/memory");
    }

    #[test]
    fn 行截断_字符数不计字节() {
        let short = clip_line("abc");
        assert_eq!(short, "abc");
        let long = "x".repeat(500);
        let clipped = clip_line(&long);
        assert_eq!(clipped.chars().count(), MAX_GREP_LINE_CHARS + 1); // 含省略号
    }

    #[test]
    fn 工具错误文本直取() {
        assert_eq!(ToolError::RespondToModel("a".into()).message(), "a");
        assert_eq!(ToolError::Fatal("b".into()).message(), "b");
    }

    /// wait_events 回执的事件形状（tag="t" snake_case、字段 camelCase——与
    /// /game/events 文件同一 serde 形态，排空即文件行）。
    #[test]
    fn 事件回执序列化形状() {
        let v = serde_json::to_value(&GameEvent::Move { seq: 2, by: "black".into(), x: 7, y: 7 })
            .expect("序列化不可失败");
        assert_eq!(v["t"], "move");
        assert_eq!(v["x"], 7);
        let v = serde_json::to_value(&GameEvent::ScoreResult {
            seq: 3, black: 1.5, white: 0.0, winner: "black".into(), dead_removed: 2,
        })
        .expect("序列化不可失败");
        assert_eq!(v["t"], "score_result");
        assert_eq!(v["deadRemoved"], 2);
    }
}
