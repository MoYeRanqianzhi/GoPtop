//! AgentHub —— 「Agent 对战」的桌面壳接缝（权威规格 `.agents/plan/2026-10-08-agent-battle.md`
//! 「F1 内置 Agent / AgentPage / Tauri 命令」节；阶段③ F 路）。
//!
//! 职责只有「装配」：`pair::pair` 结会话对、`agent_loop::run` 跑决策循环，全部在
//! tokio 任务里转；本模块持有运行表（run id → [`Run`]）并把循环侧的状态/统计/暂存
//! 着法/工具日志翻译成 `agent_*` 命令的线上契约（前端 AgentPage 的唯一 IPC 面）。
//! 决策与工具的全部语义都在 crates/goptop-agent（阶段①+② 已收口），这里一行不重做。
//!
//! **A'（人的专用会话）的落位**：pair() 在 Rust 侧同时创建 A' 与 B（这是 headless.rs
//! 验证过的路径，两个方向都绕不开它），而前端要经 `session_poll/session_cmd` 渲染和
//! 操纵 A'——所以配对完成后把 pair() 产出的 A' 接进会话表（[`crate::session::adopt_session`]）。
//! 落位 id 的来源（按 [`agent_bind`] 契约）：
//! - 前端先 `session_new` 建一个占位会话（自管 poll，同计划「会话对」节第 1 条）、
//!   再 `agent_bind(id)` 登记——配对完成后**该 id 的表项被换成真 A'**，前端对同一 id
//!   的轮询无缝切到真快照（这就是「配对目标」）；
//! - 没绑定时落到新 id，并在 `agent_status.detail` 里以「A' 会话 id=N」回给前端。
//!   该 id 同时是拦截面（[`crate::session::is_opening_cmd`]）的豁免对象。
//!
//! **MCP 模式（本阶段）**：`agent_start(driver="mcp")` 只建 A' 并置 `waiting_mcp`
//! ——外部 Agent 经 `game_start` 认领席位的接线留阶段④，这里不预做。
//!
//! **范围红线**：goptop-net / goptop-transport-native / crates/goptop-agent 一行不改；
//! 接线全部走它们的公开 API。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use goptop_agent::agent_loop::{LoopConfig, LoopDeps, LoopStop, SubagentLoop, DEFAULT_CTX_LIMIT};
use goptop_agent::llm::{
    Block, ChatRequest, HttpChannel, LlmClient, LlmConfig, Msg, NativeHttp, Protocol, Role,
};
use goptop_agent::pair::{PairConfig, SeatColor, pair};
use goptop_agent::player::{
    EmitWatch, EventQueue, NativePlayer, PlayerHandle, event_baseline, run_event_pump,
};
use goptop_agent::prompt::{PromptCfg, build_system_prompt};
use goptop_agent::registry::ToolCtx;
use goptop_agent::store::{NativeStore, VfsStore};
use goptop_agent::vfs::{InFile, Staging};
use goptop_agent::Driver;
use goptop_transport_native::{Host, NativeSession, SessionConfig, enter_runtime, now_ms, rand4};
use tauri::{AppHandle, Manager};

/// 分享基地址（pair 的链接构造用）：与 `frontend/src/net/links.ts` 的
/// `SHARE_ORIGIN_NATIVE` 同值——链接要发给「对方在浏览器打开」，打包壳里没有
/// 自己的公网地址，一律用云端部署地址。TS 常量无法从 Rust 引用，改一处要同步两处。
const SHARE_ORIGIN: &str = "https://goptop.pages.dev";

/* ---------------- store 键（与前端门面同一份 store.json） ---------------- */

/// LLM 连接配置（JSON：protocol/baseUrl/model/maxOutputTokens/replyLang/enableSubagent）。
const KEY_LLM_CONFIG: &str = "goptop:llm-config";
/// API key（**明文**——store.json 无加密，风险如实注明，计划 R1；与配置分开存，
/// 「测试连接」与「改配置不重输 key」两条 UI 路径不用整包搬密钥）。
const KEY_LLM_KEY: &str = "goptop:llm-key";
/// 上下文上限 tokens（clamp [8k, 1M]，缺省 [`DEFAULT_CTX_LIMIT`]）。
const KEY_CTX_LIMIT: &str = "goptop:agent-ctx-limit";
/// MCP 开关（本阶段只落键；server 启动留阶段④）。
const KEY_MCP_ENABLED: &str = "goptop:agent-mcp-enabled";
/// MCP 监听端口（缺省 9537）。
const KEY_MCP_PORT: &str = "goptop:agent-mcp-port";
/// MCP Bearer token（首次置 enabled 时生成并持久化）。
const KEY_MCP_TOKEN: &str = "goptop:agent-mcp-token";
/// 用户的展示名（A' 的会话名；与 `net/identity.ts` 的 myName 同一键）。
const KEY_NAME: &str = "goptop:name";

/// MCP 缺省端口（计划拍板 9537；占用回退随机口是阶段④ server 启动时的事）。
const MCP_DEFAULT_PORT: u32 = 9537;

/// agent_stop 的认输定拍：cmd(Resign) 在本地状态机即刻生效，但要给 RTC 数据面
/// 一拍把「认输」发给对面——立刻拆会话的话对面看到的就不是认输而是断线。
const RESIGN_SETTLE_MS: u64 = 600;

/* ---------------- agent_* 命令面（契约见模块注；F/G 两路逐字一致） ---------------- */

/// 开局配置（`agent_start` 的 cfgJson）。字段照契约：driver/kind/size/myColor/
/// agentName/uiLang；agentName 缺省「Agent」，uiLang 缺省按简体中文（replyLang
/// 为 null 时的兜底——配置里的 replyLang 优先于它）。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCfg {
    driver: Driver,
    kind: String,
    size: u16,
    my_color: String,
    agent_name: Option<String>,
    ui_lang: Option<String>,
}

/// `agent_start(cfgJson) -> u32`：登记运行、起配对/循环任务，返回运行 id。
///
/// **同步命令 + 显式 `enter_runtime()`**：spawn 必须站在传输层的进程级运行时里
/// （见 session.rs 同款说明）；配对与循环都在那个运行时的任务里自转，命令本身
/// 只登记就返回。单局互斥（计划「会话对」节第 4 条）：已有存活局时拒绝再开。
#[tauri::command]
pub fn agent_start(app: AppHandle, cfg_json: String) -> Result<u32, String> {
    let cfg: StartCfg =
        serde_json::from_str(&cfg_json).map_err(|e| format!("cfg 解析失败: {e}"))?;
    let seat = match cfg.my_color.as_str() {
        "black" => SeatColor::Black,
        "white" => SeatColor::White,
        other => return Err(format!("myColor 必须是 \"black\" 或 \"white\"，得到 {other:?}")),
    };
    if cfg.kind != "gomoku" && cfg.kind != "go" {
        return Err(format!("kind 必须是 \"gomoku\" 或 \"go\"，得到 {:?}", cfg.kind));
    }
    if cfg.size < 5 {
        return Err(format!("size 过小（{}）", cfg.size));
    }
    let hub = app.state::<AgentHub>();
    if hub.any_live() {
        // 单局互斥：进程内 BC hub 全局无局号（bc.rs 头注自证缺口），并行两局会互串。
        return Err("已有 Agent 对局进行中，请先停止当前对局".to_string());
    }
    let run = Arc::new(Run::new(hub.next_id()));
    run.set_running(ST_PAIRING);
    run.set_detail(Some("配对中…".into()));
    hub.insert(run.clone());
    let _g = enter_runtime();
    let app2 = app.clone();
    let run2 = run.clone();
    tokio::spawn(async move {
        match cfg.driver {
            Driver::Builtin => run_builtin_task(app2, run2, cfg, seat).await,
            // 本阶段只建 A' 并置 waiting_mcp；game_start 认领接线留阶段④。
            Driver::Mcp => run_mcp_task(app2, run2, cfg).await,
        }
    });
    Ok(run.id)
}


/// `agent_stop(id) -> null`：局中先 resign 再终止并清理。
///
/// 顺序有契约（不可换）：认输先落地（对面要看到终局有因，而不是看到断线），
/// 再取消配对/循环任务——`select!` 的取消就是 drop 那个 future，会话对随局部
/// 变量一起就地清理（pair.rs 的失败路径同一手法）；最后摘掉 A' 的会话表项，
/// 前端的轮询按既有语义（poll 失败即停泵）自然收摊。
#[tauri::command]
pub async fn agent_stop(app: AppHandle, id: u32) -> Result<(), String> {
    let run = app.state::<AgentHub>().run(id).ok_or("run not found")?;
    // 1) 局中先认输（B 席）。锁面必须先收：EnterGuard 不能跨 await。
    let resigned = {
        let _g = enter_runtime();
        let g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.player.as_ref() {
            Some(p) => {
                let snap = p.snapshot();
                let live =
                    snap.get("phase").and_then(|v| v.as_str()) == Some("playing")
                        && snap.get("winner").is_none_or(serde_json::Value::is_null);
                if live {
                    p.cmd(goptop_net::session::UiCommand::Resign);
                }
                live
            }
            None => false,
        }
    };
    if resigned {
        // 定拍（registry::SUBMIT_SETTLE_MS 同一量级）：给认输的 RTC 数据面留出发送窗口。
        tokio::time::sleep(Duration::from_millis(RESIGN_SETTLE_MS)).await;
    }
    // 2) 终止：停机标志 + 取消通知（notify_one 会存许可——任务还没走到 select 点
    // 也丢不了这次取消）。循环侧另有每轮 LLM 调用前的 stop 检查兜底。
    run.abort.store(true, Ordering::Relaxed);
    run.cancel.notify_one();
    run.set_terminal("done", Some("已停止".into()));
    // 3) 清理：A' 表项摘除（前端轮询自然停）、运行表摘除（此后 status/events 报
    //    not found——停止后的运行没有可读状态，属契约外调用）。
    let front_id = { run.inner.lock().unwrap_or_else(|e| e.into_inner()).front_id };
    if let Some(fid) = front_id {
        crate::session::remove_session(&app.state::<crate::session::Sessions>(), fid);
    }
    app.state::<AgentHub>().remove(id);
    Ok(())
}

/// `agent_status(id) -> string`：状态 JSON（契约字段逐字）。
///
/// llmCalls 在对局中用 HTTP 包装层的实时计数（每真实请求 +1，含压缩摘要请求），
/// 循环收尾后被 LoopStats 的账面值覆盖；tokens/compactions 只有循环侧才结得了账，
/// 终态前回 0——前端在局中展示调用次数、终局展示完整用量，够用且不撒谎。
#[tauri::command]
pub fn agent_status(app: AppHandle, id: u32) -> Result<String, String> {
    let run = app.state::<AgentHub>().run(id).ok_or("run not found")?;
    let g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
    let staged = g
        .staging
        .as_ref()
        .and_then(|s| s.peek(InFile::Move))
        .and_then(|t| parse_staged_move(&t));
    let llm_calls = if g.stats_final {
        g.llm_calls
    } else {
        run.llm_calls.load(Ordering::Relaxed)
    };
    Ok(serde_json::json!({
        "state": g.state,
        "detail": g.detail,
        "stagedMove": staged.map(|(x, y)| serde_json::json!({"x": x, "y": y})),
        "llmCalls": llm_calls,
        "tokensIn": g.tokens_in,
        "tokensOut": g.tokens_out,
        "compactions": g.compactions,
    })
    .to_string())
}

/// `agent_events(id, since) -> string`：工具日志环（≤200）的增量拉取。
/// `since` 是上次拿到的 `next`；回 `{"next": 总条数, "items": [since 之后的事件]}`。
#[tauri::command]
pub fn agent_events(app: AppHandle, id: u32, since: u64) -> Result<String, String> {
    let run = app.state::<AgentHub>().run(id).ok_or("run not found")?;
    let (next, items) = run.events.snapshot(since);
    Ok(serde_json::json!({ "next": next, "items": items }).to_string())
}

/// `agent_bind(sessionId) -> null`：登记人类侧 A' 会话 id（配对目标 + 拦截面豁免）。
///
/// **配对目标**：配对完成前登记的 id，是 pair() 产出的 A' 在会话表里的落位
/// （占位表项被换成真 A'，前端轮询无缝切换）；配对完成后登记的 id 只承担豁免。
#[tauri::command]
pub fn agent_bind(app: AppHandle, session_id: String) -> Result<(), String> {
    let id: u32 = session_id
        .trim()
        .parse()
        .map_err(|_| format!("sessionId 必须是数字，得到 {session_id:?}"))?;
    *app.state::<AgentHub>()
        .bound
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(id);
    Ok(())
}

/// `agent_llm_test() -> string`：发一次最小真请求验证配置，"ok" 或人话错误。
///
/// **必须异步**：真请求最坏 60s 超时 × 3 次尝试，同步命令会占死 Tauri 主线程
/// （ai.rs 同款理由）。**不硬编码任何端点/key**：配置全部读 store——真 LLM 实测
/// 是后续专门阶段，此处与测试一律不落真实端点。
#[tauri::command]
pub async fn agent_llm_test(app: AppHandle) -> String {
    let (cfg, key, _ctx, _sub) = match load_llm_cfg(&app, "简体中文") {
        Ok(x) => x,
        Err(e) => return e,
    };
    let http: Arc<dyn HttpChannel> = match NativeHttp::new() {
        Ok(h) => Arc::new(h),
        Err(e) => return format!("HTTP 客户端初始化失败：{e}"),
    };
    // 协议按配置分派（与 run_builtin_task 的装配同一张表，两处永不分叉）。
    let client = match cfg.protocol {
        Protocol::Anthropic => LlmClient::Anthropic { cfg, api_key: key, http },
        Protocol::OpenAiResponses => LlmClient::OpenAiResponses { cfg, api_key: key, http },
        Protocol::OpenAiChat => LlmClient::OpenAiChat { cfg, api_key: key, http },
    };
    let req = ChatRequest {
        system: "You are a connectivity probe. Reply with the single word: ok.".into(),
        messages: vec![Msg { role: Role::User, content: vec![Block::Text { text: "ping".into() }] }],
        tools: Vec::new(),
        max_output_tokens: 16,
    };
    match client.chat(req).await {
        Ok(_) => "ok".into(),
        // 人话错误（契约）：分类文本直出——llm 层的错误文本本就面向展示。
        Err(e) => match e {
            goptop_agent::llm::LlmError::Fatal(m) => format!("连接失败：{m}"),
            goptop_agent::llm::LlmError::Transient(m) => format!("端点不可用（重试后仍失败）：{m}"),
            goptop_agent::llm::LlmError::ContextWindowExceeded { .. } => {
                "上下文上限配置超过模型窗口，请调小上下文上限".into()
            }
        },
    }
}

/// `agent_mcp_set(enabled) -> string`：落 store 键并回 info JSON。
///
/// **本阶段 server 不启动**（阶段④接线），所以回执的 `enabled` 恒 false——键先落，
/// 阶段④的启动逻辑直接读这份配置。端口缺省 9537、token 首次生成并持久化。
#[tauri::command]
pub fn agent_mcp_set(app: AppHandle, enabled: bool) -> String {
    let r = (|| -> Result<(), String> {
        crate::store::store_set(app.clone(), KEY_MCP_ENABLED.into(), enabled.to_string())?;
        let map = crate::store::store_load(app.clone())?;
        if !map.contains_key(KEY_MCP_PORT) {
            crate::store::store_set(app.clone(), KEY_MCP_PORT.into(), MCP_DEFAULT_PORT.to_string())?;
        }
        if !map.contains_key(KEY_MCP_TOKEN) {
            // 与 HookHost 的临时 userId 同一熵源（transport 的 rand4，真熵）。
            let r = rand4();
            let token = format!("{:08x}{:08x}", r[0], r[1]);
            crate::store::store_set(app.clone(), KEY_MCP_TOKEN.into(), token)?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        eprintln!("[agent] mcp_set 落键失败: {e}");
    }
    mcp_info_json(&app)
}

/// `agent_mcp_info() -> string`：MCP 连接信息 JSON（`{"enabled","url","token"}`）。
/// enabled 恒 false（server 启动留阶段④）；url/token 回已配置值，供连接卡展示。
#[tauri::command]
pub fn agent_mcp_info(app: AppHandle) -> String {
    mcp_info_json(&app)
}

/// info JSON 的拼装（set/info 共用一份形状，防两处字段漂移）。
fn mcp_info_json(app: &AppHandle) -> String {
    let map = crate::store::store_load(app.clone()).unwrap_or_default();
    let port: u32 = map.get(KEY_MCP_PORT).and_then(|v| v.trim().parse().ok()).unwrap_or(MCP_DEFAULT_PORT);
    let token = map.get(KEY_MCP_TOKEN).filter(|t| !t.trim().is_empty()).cloned();
    serde_json::json!({
        // 阶段④前 server 恒不运行：enabled 恒 false（任务契约拍板）。
        "enabled": false,
        "url": format!("http://127.0.0.1:{port}/mcp"),
        "token": token,
    })
    .to_string()
}

/* ---------------- AgentHub：运行表与拦截面判据 ---------------- */

/// 运行表 + 绑定槽。manage 进 Tauri（lib.rs），命令与任务都经 `app.state` 取。
pub struct AgentHub {
    runs: Mutex<HashMap<u32, Arc<Run>>>,
    next_run: AtomicU32,
    /// agent_bind 登记的 A' 会话 id（配对目标；配对完成后由任务取走落位）。
    bound: Mutex<Option<u32>>,
}

impl Default for AgentHub {
    fn default() -> Self {
        Self { runs: Mutex::default(), next_run: AtomicU32::new(1), bound: Mutex::default() }
    }
}

impl AgentHub {
    fn insert(&self, run: Arc<Run>) {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run.id, run);
    }

    fn next_id(&self) -> u32 {
        self.next_run.fetch_add(1, Ordering::Relaxed)
    }

    fn run(&self, id: u32) -> Option<Arc<Run>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
    }

    fn remove(&self, id: u32) {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// 是否有存活的 Agent 局（拦截面的总闸）：pairing/waiting_mcp/thinking/waiting
    /// 都算存活；done/error 的历史运行不拦路——局终了就不该再挡人开新局。
    pub(crate) fn any_live(&self) -> bool {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|r| r.is_live())
    }

    /// 拦截面豁免的 A' 会话 id：优先存活局已落位的表 id，退回未消费的绑定值
    /// （配对中 A' 还没落位，此时豁免的就是前端登记的占位 id）。
    pub(crate) fn exempt_session(&self) -> Option<u32> {
        let live = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|r| r.is_live())
            .and_then(|r| r.inner.lock().unwrap_or_else(|e| e.into_inner()).front_id);
        live.or(*self.bound.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 取走绑定值（配对任务的落位输入；取走即消费，避免影响后续运行）。
    fn take_bound(&self) -> Option<u32> {
        self.bound.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// 拦截面判定（session_cmd 调用）：存活局期间，非豁免会话的开局类命令一律拒绝。
pub(crate) fn intercept(app: &AppHandle, session_id: u32) -> bool {
    let hub = app.state::<AgentHub>();
    hub.any_live() && hub.exempt_session() != Some(session_id)
}

/* ---------------- 运行：状态/统计/日志环 ---------------- */

/// 状态字面量（契约值域）。
const ST_PAIRING: &str = "pairing";
const ST_WAITING_MCP: &str = "waiting_mcp";
const ST_THINKING: &str = "thinking";
const ST_WAITING: &str = "waiting";
const ST_DONE: &str = "done";
const ST_ERROR: &str = "error";

/// 一次运行的可变账本。
struct RunInner {
    state: &'static str,
    detail: Option<String>,
    /// 循环收尾后的账面统计（LoopStats；stats_final 前不回，避免半截数字）。
    llm_calls: u64,
    tokens_in: u64,
    tokens_out: u64,
    compactions: u32,
    stats_final: bool,
    /// B 席（agent_stop 的认输落点；配对完成前为 None）。
    player: Option<NativePlayer>,
    /// B 的暂存区（agent_status 的 stagedMove 读数——与循环/工具面同一份）。
    staging: Option<Arc<Staging>>,
    /// B 的执色（ticker 的 thinking/waiting 判定：轮到 B = thinking）。
    agent_color: Option<String>,
    /// A' 在会话表的落位 id（拦截面豁免 + agent_stop 的清理对象）。
    front_id: Option<u32>,
}

/// 一次 Agent 对局的运行时记录。任务持有它的 Arc，命令面经 AgentHub 查它。
struct Run {
    /// 运行 id（AgentHub 发号后随构造传入）。
    id: u32,
    /// 用户中止标志（LoopDeps.stop；循环每轮 LLM 调用前检查——点了停止就不再花钱）。
    abort: Arc<AtomicBool>,
    /// 硬取消通知（agent_stop 用 notify_one 存许可；select! 消费——drop 配对/循环
    /// future 即会话对就地清理，pair.rs 的失败路径同一手法）。
    cancel: tokio::sync::Notify,
    /// 工具日志环（≤200；agent_events 的数据源）。
    events: Arc<EventRing>,
    /// 实时 LLM 调用计数（LogHttp 每真实请求 +1；终态后被账面值取代）。
    /// Arc：LogHttp 包装层与状态回执两处共享同一计数器。
    llm_calls: Arc<AtomicU64>,
    inner: Mutex<RunInner>,
}

impl Run {
    fn new(id: u32) -> Self {
        Self {
            id,
            abort: Arc::new(AtomicBool::new(false)),
            cancel: tokio::sync::Notify::new(),
            events: Arc::new(EventRing::default()),
            llm_calls: Arc::new(AtomicU64::new(0)),
            inner: Mutex::new(RunInner {
                state: ST_PAIRING,
                detail: None,
                llm_calls: 0,
                tokens_in: 0,
                tokens_out: 0,
                compactions: 0,
                stats_final: false,
                player: None,
                staging: None,
                agent_color: None,
                front_id: None,
            }),
        }
    }

    fn is_live(&self) -> bool {
        !matches!(self.state(), ST_DONE | ST_ERROR)
    }

    fn state(&self) -> &'static str {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state
    }

    fn set_detail(&self, detail: Option<String>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).detail = detail;
    }

    /// 运行中状态切换（thinking/waiting/pairing/waiting_mcp 互转）。**不覆盖终态**：
    /// ticker 与循环收尾存在竞窗，终态一旦落账就不许被运行中状态盖掉。
    fn set_running(&self, state: &'static str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(g.state, ST_DONE | ST_ERROR) {
            g.state = state;
        }
    }

    /// 终态落账（done/error，一锤定音；把循环的账面统计一并收进来）。
    fn set_terminal(&self, state: &'static str, detail: Option<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.state = state;
        g.detail = detail;
    }
}

/* ---------------- 内置模式任务：配对 → 循环 ---------------- */

/// 内置模式的整局装配（tokio 任务；运行在传输层运行时上——agent_start 的
/// enter_runtime 上下文里 spawn，内部无需再进运行时）。
async fn run_builtin_task(app: AppHandle, run: Arc<Run>, cfg: StartCfg, seat: SeatColor) {
    let hub = app.state::<AgentHub>();
    let ui_lang = cfg.ui_lang.clone().unwrap_or_else(|| "简体中文".into());
    let agent_name = cfg.agent_name.clone().unwrap_or_else(|| "Agent".into());

    // —— 配置先于配对：LLM 没配好就不该结会话对（失败立即可见、可重试，不白等 80s）。
    let (llm_cfg, api_key, ctx_limit, subagent_enabled) = match load_llm_cfg(&app, &ui_lang) {
        Ok(x) => x,
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(e));
            return;
        }
    };
    // /memory 的 native 后端（~/.goptop/agent-memory.db；路径由本壳决定——
    // store.rs 的平台落盘规则与 UI 设置同目录）。
    let db = match crate::store::store_dir(&app) {
        Ok(dir) => dir.join("agent-memory.db"),
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(format!("定位数据目录失败：{e}")));
            return;
        }
    };
    let memory = match NativeStore::open(&db) {
        Ok(s) => Arc::new(LogStore { inner: s, ring: run.events.clone() }) as Arc<dyn VfsStore>,
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(format!("记忆库打开失败：{e}")));
            return;
        }
    };
    let http = Arc::new(LogHttp {
        inner: match NativeHttp::new() {
            Ok(h) => h,
            Err(e) => {
                run.set_terminal(ST_ERROR, Some(format!("HTTP 客户端初始化失败：{e}")));
                return;
            }
        },
        ring: run.events.clone(),
        calls: run.llm_calls.clone(),
    });
    let llm = Arc::new(match llm_cfg.protocol {
        Protocol::Anthropic => {
            LlmClient::Anthropic { cfg: llm_cfg.clone(), api_key, http: http.clone() }
        }
        Protocol::OpenAiResponses => {
            LlmClient::OpenAiResponses { cfg: llm_cfg.clone(), api_key, http: http.clone() }
        }
        Protocol::OpenAiChat => LlmClient::OpenAiChat { cfg: llm_cfg.clone(), api_key, http },
    });

    // —— 配对（select 硬取消：drop 配对 future = 半途的会话就地清理）。
    let front_tauri = Arc::new(crate::session::TauriHost::new(app.clone()));
    let agent_tauri = Arc::new(crate::session::TauriHost::new(app.clone()));
    let pair_cfg = PairConfig {
        kind: cfg.kind.clone(),
        size: cfg.size,
        my_color: seat,
        front_name: stored_name(&app),
        agent_name: agent_name.clone(),
        share_origin: SHARE_ORIGIN.into(),
        front_host: front_tauri.clone() as Arc<dyn Host>,
        agent_host: agent_tauri as Arc<dyn Host>,
    };
    let paired = tokio::select! {
        p = pair(pair_cfg) => match p {
            Ok(p) => p,
            Err(e) => {
                run.set_terminal(ST_ERROR, Some(format!("配对失败：{e}")));
                return;
            }
        },
        // 用户在配对期点了停止：future 被 drop，半途建的两席随局部变量收摊。
        _ = run.cancel.notified() => {
            run.set_terminal(ST_DONE, Some("已停止".into()));
            return;
        }
    };

    // —— 事件物化（player.rs 的纪律：基线在 spawn 前本任务同步取定）。
    let queue = Arc::new(EventQueue::new());
    paired.agent_hook.bind_events(&queue);
    let baseline = event_baseline(&paired.agent_watch);
    tokio::spawn(run_event_pump(EmitWatch::new(paired.agent_watch.clone_rx()), queue.clone(), baseline));

    // —— A' 落位：占位 id（agent_bind）或新 id；真 A' 的后台泵自己转（前端轮询
    //    叠加泵是幂等的，与 session_new 建的每个原生会话同款）。
    let front_arc = paired.front.session().clone();
    front_arc.start_pump();
    let front_id = {
        let sessions = app.state::<crate::session::Sessions>();
        crate::session::adopt_session(&sessions, hub.take_bound(), front_arc, front_tauri)
    };
    let agent_color = seat.opponent().as_str().to_string();
    {
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.front_id = Some(front_id);
        g.player = Some(paired.agent.clone());
        g.agent_color = Some(agent_color.clone());
    }
    run.set_detail(Some(format!("已配对，A' 会话 id={front_id}")));

    // —— 工具面：主循环与子代理各持一份 ToolCtx，Arc 底座共享同一局
    //    （Staging 尤其不能有两份——暂存区的一致性建立在单实例上）。
    let staging = Arc::new(Staging::new());
    let make_ctx = || ToolCtx {
        player: Arc::new(LogPlayer { inner: paired.agent.clone(), ring: run.events.clone() }),
        watch: EmitWatch::new(paired.agent_watch.clone_rx()),
        events: queue.clone(),
        staging: staging.clone(),
        memory: memory.clone(),
        memory_ns: Driver::Builtin.memory_ns(),
        driver: Driver::Builtin,
        subagent_enabled,
        subagent: None,
    };
    // 子代理的 ctx 先建（subagent=None——深度 1 的结构保证）；主 ctx 持 runner。
    let sub_ctx = make_ctx();
    let mut ctx = make_ctx();
    if subagent_enabled {
        ctx.subagent = Some(Arc::new(SubagentLoop::new(llm.clone(), sub_ctx)));
    }

    let system = build_system_prompt(&PromptCfg {
        agent_name,
        my_color: agent_color.clone(),
        kind: cfg.kind.clone(),
        size: cfg.size,
        reply_lang: llm_cfg.reply_lang.clone().unwrap_or_else(|| ui_lang.clone()),
        driver: Driver::Builtin,
        subagent_enabled,
    });
    let deps = LoopDeps {
        llm,
        llm_cfg,
        cfg: LoopConfig { max_llm_calls: goptop_agent::agent_loop::DEFAULT_MAX_LLM_CALLS, ctx_limit },
        tools: ctx,
        stop: run.abort.clone(),
        system,
    };

    // —— 状态 ticker：轮到 B=thinking、轮到对手=waiting（循环本体是一口阻塞调用，
    //    内部相位它不外报；用快照的 toMove 反推是最诚实的近似）。
    let ticker_run = run.clone();
    let ticker_player = paired.agent.clone();
    let ticker_color = agent_color;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !ticker_run.is_live() {
                break;
            }
            let to_move = ticker_player
                .snapshot()
                .get("toMove")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if to_move.as_deref() == Some(ticker_color.as_str()) {
                ticker_run.set_running(ST_THINKING);
            } else {
                ticker_run.set_running(ST_WAITING);
            }
        }
    });

    // —— 循环（select 硬取消同配对）：收尾把 LoopStats 记账。
    let outcome = tokio::select! {
        o = goptop_agent::agent_loop::run(deps) => Some(o),
        _ = run.cancel.notified() => None,
    };
    {
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stats_final = true;
        if let Some(o) = outcome {
            g.llm_calls = u64::from(o.stats.llm_calls);
            g.tokens_in = o.stats.input_tokens;
            g.tokens_out = o.stats.output_tokens;
            g.compactions = o.stats.compactions;
            let detail = match &o.stop {
                LoopStop::GameOver => "对局结束".to_string(),
                LoopStop::Resigned => "Agent 已认输".to_string(),
                LoopStop::Stopped => "已停止".to_string(),
                LoopStop::BudgetExhausted => {
                    "思考预算用尽（LLM 调用达上限），对局中止".to_string()
                }
                LoopStop::Fatal(m) => {
                    g.state = ST_ERROR;
                    format!("Agent 异常退出：{m}")
                }
            };
            if g.state != ST_ERROR {
                g.state = ST_DONE;
            }
            g.detail = Some(detail);
        } else {
            g.state = ST_DONE;
            g.detail = Some("已停止".into());
        }
    }
}

/// MCP 模式任务（本阶段）：只建 A' 并置 waiting_mcp。
///
/// 外部 Agent 的 `game_start` 认领、以 B 的邀请链接经 Boot 路径重配（我执白方向
/// 时 A' 要携链创建）都属阶段④的 handler 接线；这里先把「人这席」立起来——
/// 会话表落位与内置模式同一套（bind 落位 / 新 id + detail 回报）。
async fn run_mcp_task(app: AppHandle, run: Arc<Run>, cfg: StartCfg) {
    let hub = app.state::<AgentHub>();
    let host = Arc::new(crate::session::TauriHost::new(app.clone()));
    // 无服务器的 p2p 基座链接：on_boot 只认意图不落页，纯 base 即「坐等开局」。
    let base = format!("{}/p2p", SHARE_ORIGIN.trim_end_matches('/'));
    let session = {
        let _g = enter_runtime();
        Arc::new(NativeSession::new(
            SessionConfig {
                name: stored_name(&app),
                server_mode: false,
                share_origin: SHARE_ORIGIN.into(),
                kind: cfg.kind.clone(),
                size: cfg.size,
            },
            host.clone() as Arc<dyn Host>,
            &base,
        ))
    };
    session.start_pump();
    let front_id = {
        let sessions = app.state::<crate::session::Sessions>();
        crate::session::adopt_session(&sessions, hub.take_bound(), session, host)
    };
    {
        // A' 是人这席：认输/清理的落点在 A'（本阶段 B 还不存在）。
        // player 留空——局都还没结，agent_stop 的认输分支自然跳过。
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.front_id = Some(front_id);
    }
    run.set_running(ST_WAITING_MCP);
    run.set_detail(Some(format!("等待 MCP Agent 接入…（A' 会话 id={front_id}）")));
}

/* ---------------- 配置装载（store → llm 层类型；纯函数便于单测） ---------------- */

/// store 里 goptop:llm-config 的 JSON 形态（与前端设置卡同键同形）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmCfgJson {
    protocol: Protocol,
    base_url: String,
    model: String,
    max_output_tokens: Option<u32>,
    reply_lang: Option<String>,
    enable_subagent: Option<bool>,
}

/// 上下文上限的合法区间（计划：用户可设，clamp [8k, 1M]）。
const CTX_MIN: u32 = 8_000;
const CTX_MAX: u32 = 1_000_000;

/// 读 store 聚齐 LLM 配置：`(LlmConfig, api_key, ctx_limit, subagent_enabled)`。
/// replyLang 为 null/缺省时以 uiLang 兜底（agent_start 由前端传当前 UI 语言）。
fn load_llm_cfg(
    app: &AppHandle,
    ui_lang: &str,
) -> Result<(LlmConfig, String, u32, bool), String> {
    let map = crate::store::store_load(app.clone())?;
    parse_llm_cfg(&map, ui_lang)
}

/// [`load_llm_cfg`] 的纯函数体（对 store 表直接解析，单测不依赖 AppHandle）。
fn parse_llm_cfg(
    map: &BTreeMap<String, String>,
    ui_lang: &str,
) -> Result<(LlmConfig, String, u32, bool), String> {
    let raw = map
        .get(KEY_LLM_CONFIG)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("尚未配置 LLM：请在「Agent 对战」设置卡选择协议、填写端点与模型")?;
    let j: LlmCfgJson =
        serde_json::from_str(raw).map_err(|e| format!("LLM 配置解析失败：{e}（{raw}）"))?;
    let key = map
        .get(KEY_LLM_KEY)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("尚未配置 API Key：请在「Agent 对战」设置卡填写后重试")?
        .to_string();
    let ctx_limit = map
        .get(KEY_CTX_LIMIT)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_CTX_LIMIT)
        .clamp(CTX_MIN, CTX_MAX);
    let cfg = LlmConfig {
        protocol: j.protocol,
        base_url: j.base_url.trim().trim_end_matches('/').to_string(),
        model: j.model.trim().to_string(),
        max_output_tokens: j.max_output_tokens.unwrap_or(1024).max(64),
        reply_lang: Some(
            j.reply_lang
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ui_lang.to_string()),
        ),
    };
    if cfg.base_url.is_empty() || cfg.model.is_empty() {
        return Err("LLM 配置不完整：端点与模型都不能为空".into());
    }
    Ok((cfg, key, ctx_limit, j.enable_subagent.unwrap_or(false)))
}

/// 用户的展示名（A' 会话名 = 聊天记录里人的名字；缺省「玩家」）。
fn stored_name(app: &AppHandle) -> String {
    let map = crate::store::store_load(app.clone()).unwrap_or_default();
    let name = map.get(KEY_NAME).map(String::as_str).map(str::trim).filter(|s| !s.is_empty());
    name.unwrap_or("玩家").to_string()
}

/// 暂存着法文本 → `(x, y)`（agent_status 的 stagedMove；pass/残缺 → None——
/// pass 不是盘面位置，与 registry 的 parse_staged_move 同一口径，但那边私有）。
fn parse_staged_move(text: &str) -> Option<(u16, u16)> {
    let t = text.trim();
    if t.is_empty() || t == "pass" {
        return None;
    }
    let (x, y) = t.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/* ---------------- 日志包装层：循环的执行面没有工具级钩子，日志在三处公开
   trait 的接缝上收口（HTTP 通道 / 玩家命令 / 记忆存储），一行不改 goptop-agent ---------------- */

/// 工具日志环（≤200，`agent_events` 的增量拉取按 `total` 游标推进）。
#[derive(Default)]
struct EventRing {
    inner: Mutex<RingInner>,
}

#[derive(Default)]
struct RingInner {
    items: VecDeque<RingItem>,
    total: u64,
}

struct RingItem {
    ts: u64,
    tool: String,
    ok: bool,
    ms: u64,
    summary: String,
}

impl EventRing {
    fn push(&self, tool: &str, ok: bool, ms: u64, summary: String) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.items.len() >= 200 {
            g.items.pop_front();
        }
        g.items.push_back(RingItem { ts: now_ms(), tool: tool.to_string(), ok, ms, summary });
        g.total += 1;
    }

    /// 回 `(next, items)`：next=总条数（下一轮的 since），items=下标 ≥ since 的事件。
    fn snapshot(&self, since: u64) -> (u64, Vec<serde_json::Value>) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let first = g.total - g.items.len() as u64;
        let items = g
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| first + *i as u64 >= since)
            .map(|(_, it)| {
                serde_json::json!({"ts": it.ts, "tool": it.tool, "ok": it.ok, "ms": it.ms, "summary": it.summary})
            })
            .collect();
        (g.total, items)
    }
}

/// 玩家命令包装：B 席的每个 UiCommand（= submit 落地的动作、计分自动确认、认输）
/// 记一条日志。`cmd` 无返回值（对局规则错误由状态机静默拒），ok 恒 true 表「已下达」。
struct LogPlayer {
    inner: NativePlayer,
    ring: Arc<EventRing>,
}

impl PlayerHandle for LogPlayer {
    fn cmd(&self, cmd: goptop_net::session::UiCommand) {
        let t0 = Instant::now();
        self.inner.cmd(cmd.clone());
        let (kind, summary) = action_summary(&cmd);
        self.ring.push(
            &format!("submit:{kind}"),
            true,
            t0.elapsed().as_millis() as u64,
            summary,
        );
    }

    fn snapshot(&self) -> serde_json::Value {
        self.inner.snapshot()
    }

    fn pump(&self) {
        self.inner.pump();
    }

    fn wait_until(
        &self,
        pred: &mut dyn FnMut(&serde_json::Value) -> bool,
        timeout: Duration,
    ) -> bool {
        self.inner.wait_until(pred, timeout)
    }
}

/// UiCommand → 日志的 (工具名, 摘要)。文本一律截断——日志是给人看的进度条，
/// 不是聊天记录的第二个副本（全文在 /game/chat）。
fn action_summary(cmd: &goptop_net::session::UiCommand) -> (String, String) {
    use goptop_net::session::UiCommand as C;
    fn clip(s: &str) -> String {
        s.chars().take(60).collect()
    }
    match cmd {
        C::Place { x, y } => ("move".into(), format!("落子 ({x},{y})")),
        C::Pass => ("pass".into(), "停一手".into()),
        C::Resign => ("resign".into(), "认输".into()),
        C::SendChat(t) => ("chat".into(), format!("发送消息：{}", clip(t))),
        C::RequestUndo => ("request".into(), "请求悔棋".into()),
        C::RequestReset => ("request".into(), "请求重开".into()),
        C::RequestSwap => ("request".into(), "请求换棋".into()),
        C::ConfirmApprove => ("confirm".into(), "同意对方请求".into()),
        C::ConfirmDecline => ("confirm".into(), "拒绝对方请求".into()),
        C::ConfirmScore => ("score".into(), "确认计分".into()),
        C::ToggleDead { x, y } => ("dead".into(), format!("标记死子 ({x},{y})")),
        _ => ("other".into(), "其他会话命令".into()),
    }
}

/// HTTP 通道包装：每次真实 LLM 请求记一条日志并给运行表 +1 实时调用计数
/// （循环对 Hub 不透明，这是唯一能实时数调用次数的缝）。
struct LogHttp {
    inner: NativeHttp,
    ring: Arc<EventRing>,
    calls: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl HttpChannel for LogHttp {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String), String> {
        let t0 = Instant::now();
        let result = self.inner.post_json(url, headers, body).await;
        let n = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let ms = t0.elapsed().as_millis() as u64;
        match &result {
            Ok((status, text)) => self.ring.push(
                "llm",
                *status < 400,
                ms,
                format!("第 {n} 次模型调用：HTTP {status}，{} 字节回复", text.len()),
            ),
            Err(e) => self.ring.push("llm", false, ms, format!("第 {n} 次模型调用失败：{e}")),
        }
        result
    }
}

/// 记忆存储包装：/memory 的写删记日志（读写不记——读是模型的本分，刷屏无益）。
struct LogStore {
    inner: NativeStore,
    ring: Arc<EventRing>,
}

impl VfsStore for LogStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        self.inner.read(ns, path)
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        let r = self.inner.write(ns, path, content);
        self.ring.push(
            "memory",
            r.is_ok(),
            0,
            format!("记忆写入 /memory/{path}（{} 字节）", content.len()),
        );
        r
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        let r = self.inner.delete(ns, path);
        self.ring.push("memory", r.as_ref().is_ok_and(|&d| d), 0, format!("记忆删除 /memory/{path}"));
        r
    }

    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String> {
        self.inner.list(ns, prefix)
    }

    fn usage(&self, ns: &str) -> Result<u64, String> {
        self.inner.usage(ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ---- 暂存着法解析（agent_status.stagedMove 的口径） ---- */

    #[test]
    fn 暂存着法解析_坐标pass与残缺() {
        assert_eq!(parse_staged_move("8,8"), Some((8, 8)));
        assert_eq!(parse_staged_move(" 7,7 "), Some((7, 7)));
        assert_eq!(parse_staged_move("pass"), None, "pass 不是盘面位置，无幽灵子可画");
        assert_eq!(parse_staged_move(""), None);
        assert_eq!(parse_staged_move("abc"), None);
        assert_eq!(parse_staged_move("7"), None);
        assert_eq!(parse_staged_move("7,7,7"), None);
    }

    /* ---- 日志环：≤200 上限与 since 游标 ---- */

    #[test]
    fn 日志环_环形上限与增量游标() {
        let ring = EventRing::default();
        for i in 0..205 {
            ring.push("llm", true, i, format!("第 {i} 次"));
        }
        let (next, items) = ring.snapshot(0);
        assert_eq!(next, 205, "next 是总条数，不因环截断而变小");
        assert_eq!(items.len(), 200, "环形 ≤200，最老的被挤掉");
        assert_eq!(items[0]["summary"], "第 5 次", "挤掉的应是前 5 条");
        // 增量：since=203 只回第 203、204 两条（下标从 5 起）。
        let (next2, items2) = ring.snapshot(203);
        assert_eq!(next2, 205);
        assert_eq!(items2.len(), 2);
        assert_eq!(items2[0]["summary"], "第 203 次");
        // 落后的 since 也不会重复给（下标 < since 的全部滤掉）。
        assert_eq!(ring.snapshot(100).1.len(), 105);
    }

    /* ---- LLM 配置解析（store 表 → llm 层类型） ---- */

    fn cfg_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn llm配置解析_全字段与兜底() {
        let map = cfg_map(&[
            (KEY_LLM_CONFIG, r#"{"protocol":"anthropic","baseUrl":"https://api.example.com/v1/","model":"claude-x","maxOutputTokens":2048,"replyLang":"喵语","enableSubagent":true}"#),
            (KEY_LLM_KEY, " sk-test "),
            (KEY_CTX_LIMIT, "32000"),
        ]);
        let (cfg, key, ctx, sub) = parse_llm_cfg(&map, "简体中文").expect("应解析成功");
        assert!(matches!(cfg.protocol, Protocol::Anthropic));
        assert_eq!(cfg.base_url, "https://api.example.com/v1", "尾斜杠剥掉，适配器只拼尾路径");
        assert_eq!(cfg.model, "claude-x");
        assert_eq!(cfg.max_output_tokens, 2048);
        assert_eq!(cfg.reply_lang.as_deref(), Some("喵语"));
        assert_eq!(key, "sk-test", "key 去空白");
        assert_eq!(ctx, 32_000);
        assert!(sub);
    }

    #[test]
    fn llm配置解析_replyLang空走uiLang_上限clamp与缺省() {
        let map = cfg_map(&[
            (KEY_LLM_CONFIG, r#"{"protocol":"open_ai_chat","baseUrl":"http://h","model":"m"}"#),
            (KEY_LLM_KEY, "k"),
            (KEY_CTX_LIMIT, "1"),
        ]);
        let (cfg, _, ctx, sub) = parse_llm_cfg(&map, "English").expect("应解析成功");
        assert_eq!(cfg.reply_lang.as_deref(), Some("English"), "replyLang 缺省=跟随 UI 语言");
        assert_eq!(cfg.max_output_tokens, 1024, "maxOutputTokens 缺省 1024（计划拍板）");
        assert_eq!(ctx, CTX_MIN, "越下限 clamp 到 8k");
        assert!(!sub, "子代理默认关");

        let map2 = cfg_map(&[
            (KEY_LLM_CONFIG, r#"{"protocol":"open_ai_responses","baseUrl":"http://h","model":"m","replyLang":"  "}"#),
            (KEY_LLM_KEY, "k"),
        ]);
        let (cfg2, _, ctx2, _) = parse_llm_cfg(&map2, "中文").expect("应解析成功");
        assert_eq!(cfg2.reply_lang.as_deref(), Some("中文"), "空白 replyLang 视同未填");
        assert_eq!(ctx2, DEFAULT_CTX_LIMIT, "未配置=176k 缺省");
    }

    #[test]
    fn llm配置解析_缺配置与坏配置都是人话错误() {
        let empty = cfg_map(&[]);
        let e = parse_llm_cfg(&empty, "zh").unwrap_err();
        assert!(e.contains("尚未配置 LLM"), "{e}");
        let nokey = cfg_map(&[(KEY_LLM_CONFIG, r#"{"protocol":"anthropic","baseUrl":"http://h","model":"m"}"#)]);
        assert!(parse_llm_cfg(&nokey, "zh").unwrap_err().contains("API Key"));
        let bad = cfg_map(&[(KEY_LLM_CONFIG, "{ 不是 JSON"), (KEY_LLM_KEY, "k")]);
        assert!(parse_llm_cfg(&bad, "zh").unwrap_err().contains("解析失败"));
        let incomplete =
            cfg_map(&[(KEY_LLM_CONFIG, r#"{"protocol":"anthropic","baseUrl":" ","model":"m"}"#), (KEY_LLM_KEY, "k")]);
        assert!(parse_llm_cfg(&incomplete, "zh").unwrap_err().contains("不完整"));
    }

    /* ---- agent_start 的 cfg JSON（前端契约形状） ---- */

    #[test]
    fn start_cfg_契约形状与小写驱动() {
        let cfg: StartCfg = serde_json::from_str(
            r#"{"driver":"builtin","kind":"gomoku","size":15,"myColor":"black",
                "agentName":null,"uiLang":"zh-CN"}"#,
        )
        .expect("契约形状应可解析");
        assert!(matches!(cfg.driver, Driver::Builtin));
        assert_eq!(cfg.agent_name, None);
        assert_eq!(cfg.ui_lang.as_deref(), Some("zh-CN"));

        let cfg: StartCfg = serde_json::from_str(
            r#"{"driver":"mcp","kind":"go","size":9,"myColor":"white"}"#,
        )
        .expect("缺省字段应可解析");
        assert!(matches!(cfg.driver, Driver::Mcp));
    }

    /* ---- 动作摘要（日志环的 tool/summary 口径） ---- */

    #[test]
    fn 动作摘要_主类与文本截断() {
        use goptop_net::session::UiCommand as C;
        assert_eq!(action_summary(&C::Place { x: 7, y: 7 }), ("move".into(), "落子 (7,7)".into()));
        let long = "x".repeat(200);
        let (_, s) = action_summary(&C::SendChat(long));
        assert_eq!(s.chars().count(), 60 + "发送消息：".chars().count(), "文本截断，日志不是聊天副本");
        assert_eq!(action_summary(&C::Resign).0, "resign");
        assert_eq!(action_summary(&C::SetName("甲".into())).0, "other");
    }

    /* ---- 运行状态机：终态不被运行中状态覆盖 ---- */

    #[test]
    fn 运行状态_终态一锤定音() {
        let run = Run::new(1);
        run.set_running(ST_THINKING);
        assert_eq!(run.state(), ST_THINKING);
        assert!(run.is_live());
        run.set_terminal(ST_ERROR, Some("boom".into()));
        assert!(!run.is_live(), "error 态不拦路");
        run.set_running(ST_WAITING);
        assert_eq!(run.state(), ST_ERROR, "终态后 ticker 的运行中切换必须无效");
    }
}
