//! AgentHub —— 「Agent 对战」的桌面壳接缝（权威规格 `.agents/plan/2026-10-08-agent-battle.md`：
//! 「会话对」节与本文件的会话拓扑/拦截面互锁，「AgentPage（唯一入口，frontend/src/pages/
//! AgentPage.tsx 新建）」节的「Tauri 命令」条目即本模块的命令面；阶段③ F 路）。
//!
//! 职责只有「装配」：结会话对、跑 `agent_loop::run` 决策循环，全部在 tokio 任务里
//! 转；本模块持有运行表（run id → [`Run`]）并把循环侧的状态/统计/暂存着法/工具
//! 日志翻译成 `agent_*` 命令的线上契约（前端 AgentPage 的唯一 IPC 面）。决策与
//! 工具的全部语义都在 crates/goptop-agent（阶段①+② 已收口），这里一行不重做。
//!
//! **会话拓扑（与 AgentPage 的开局流程逐拍互锁）**：A'（人的专用会话）由**前端**
//! 经 `session_new` 创建（固定 serverMode:false，onChange 注入自管 poll——计划
//! 「会话对」节第 1 条）并 `agent_bind(id)` 登记给本 Hub；Hub 只创建 B（无头席，
//! HookHost 包 TauriHost）并把 A'、B 结对。方向随执色（与 goptop-agent pair.rs
//! 同一条已验证路径，仅「谁创建 A'」不同——A' 的表项归前端所有，Hub 不落位不摘除）：
//! - **我执黑**：A' 邀请。`agent_bind` 时若 A' 还空置（phase=home）就代发
//!   `CreateInvite`（前端流程是 bind → 等 inviteUrl 含 rtc → agent_start，而 A'
//!   建在 /p2p 基座上不会自发邀请——bind 是链路里唯一能替 A' 按下「开启对战」的
//!   点）；`agent_start` 后 Hub 读 A' 的邀请链接、携链创建 B，泵到双端 playing。
//! - **我执白**：B 先建局邀请，邀请链接经 `agent_status.detail` 送回前端（前端
//!   `extractInviteLink` 取含 rtc= 的 URL）；前端以该链接经 Boot 路径 `session_new`
//!   创建 A'（本就是 session_new 的 href 参数，无需扩展）并 bind——A' 的回执经
//!   进程内 presence 自动送达 B，Hub 泵到双端 playing。
//!
//! **MCP 模式（阶段④ H2 路）**：`agent_mcp_set(true)` 经 goptop-agent 的
//! `mcp` feature（仅桌面编译）真起内嵌服务器（读 `goptop:agent-mcp-port/token`，
//! 端口被占回退随机口并把实际值回写 info）；`agent_start(driver=mcp)` 接驳 A' 后
//! 置 `waiting_mcp` 并把**认领闭包**存进服务器的待局槽——外部 Agent `game_start`
//! 时才跑配对（用已 bind 的 A' + 现建 B，`pair.rs` 同款原语），认领成功后工具面
//! （wait_events 等）路由到该局；本任务侧观察活局（认领→thinking/waiting、终局/
//! 离场→done），`agent_stop` 先认输再 `stop_game` 拆局——认输的 game_over 事件会
//! 先行物化进事件队列，阻塞中的 wait_events 随之返回，实现与停止的联动。
//!
//! **范围红线**：goptop-net / goptop-transport-native / harmony 一行不改；
//! 接线全部走它们的公开 API。goptop-agent 的 `pair()` 在壳内不经手（它自建 A'，
//! 而本壳的 A' 归前端——配对步骤按 pair.rs 同款原语在本文件落地，语义一致；MCP
//! 待局闭包同款，见 [`claim_seats`]）。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use goptop_agent::agent_loop::{LoopConfig, LoopDeps, LoopStop, SubagentLoop, DEFAULT_CTX_LIMIT};
#[cfg(desktop)]
use goptop_agent::mcp::{McpConfig, McpServer, PendingGame};
use goptop_agent::llm::{
    Block, ChatRequest, HttpChannel, LlmClient, LlmConfig, Msg, NativeHttp, Protocol, Role,
};
use goptop_agent::pair::{self, SeatColor};
use goptop_agent::player::{
    EmitWatch, EventQueue, HookHost, NativePlayer, PlayerHandle, event_baseline, run_event_pump,
};
use goptop_agent::prompt::{PromptCfg, build_system_prompt};
use goptop_agent::registry::ToolCtx;
use goptop_agent::store::{NativeStore, VfsStore};
use goptop_agent::vfs::{InFile, Staging};
use goptop_agent::Driver;
use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession, SessionConfig, enter_runtime, now_ms};
#[cfg(desktop)]
use goptop_transport_native::rand4;
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
/// MCP 开关偏好（[`agent_mcp_set`] 落键、[`mcp_autostart`] 启动时读——两条路
/// 共同保证「重启后开关宣称的状态」与「服务器真实状态」一致）。
#[cfg(desktop)]
const KEY_MCP_ENABLED: &str = "goptop:agent-mcp-enabled";
/// MCP 监听端口（缺省 9537）。
#[cfg(desktop)]
const KEY_MCP_PORT: &str = "goptop:agent-mcp-port";
/// MCP Bearer token（首次置 enabled 时生成并持久化）。
#[cfg(desktop)]
const KEY_MCP_TOKEN: &str = "goptop:agent-mcp-token";

/// MCP 缺省端口（计划拍板 9537；占用回退随机口是阶段④ server 启动时的事）。
#[cfg(desktop)]
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
    // MCP 对局的认领请求只能来自内嵌服务器——没起服务器就开局必然卡死在 waiting_mcp。
    #[cfg(desktop)]
    if cfg.driver == Driver::Mcp && hub.mcp_server().is_none() {
        return Err("MCP 服务器未启用：请先在 MCP 连接卡打开开关再开局".to_string());
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
            Driver::Mcp => run_mcp_task(app2, run2, cfg, seat).await,
        }
    });
    Ok(run.id)
}

/// `agent_stop(id) -> null`：局中先 resign 再终止并清理。
///
/// 顺序有契约（不可换）：认输先落地（对面要看到终局有因，而不是看到断线），
/// 再取消配对/循环任务——`select!` 的取消就是 drop 那个 future，会话对随局部
/// 变量一起就地清理（pair.rs 的失败路径同一手法）。
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
                    p.cmd(UiCommand::Resign);
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
    // 3) MCP 对局的装配拆除（服务器本体不停，见 run_mcp_task）：外部 Agent 阻塞
    //    中的 wait_events 已因步骤 1 的认输收到 game_over 事件——600ms 定拍就是
    //    给事件物化留的窗口；此后 tools/call 一律回「no live game」，对局面收口。
    #[cfg(desktop)]
    if let Some(s) = app.state::<AgentHub>().mcp_server() {
        s.stop_game();
    }
    // 4) 清理：运行表摘除（此后 status/events 报 not found——停止后的运行没有
    //    可读状态）。A' 的会话表项**不动**——它归前端所有，由 AgentPage 的
    //    teardown 走 dispose（session_drop）收摊，两边都摘只会互相竞争。
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
/// **配对目标**：登记的 id 就是配对用的那条 A'（Hub 从会话表取它结对 B）——
/// A' 的表项归前端所有，Hub 不落位、不摘除。
///
/// **开局邀请代发**：登记时若该会话还空置（phase=home）就代发一次 `CreateInvite`。
/// 前端流程（AgentPage.start 我执黑分支）是 bind → 等 inviteUrl 含 rtc →
/// agent_start，而 A' 建在 /p2p 基座上、Boot 对 P2p 意图无动作（lobby.rs 的
/// process_intent 空 branch）——不会自发邀请，本命令是链路里唯一能替 A' 按下
/// 「开启对战」的点。**仅在 home 时发**：我执白方向的 A' 是携链 Boot 的受邀席，
/// bind 时已进入受理过程，而 `create_invite` 没有任何 phase/role 守卫（lobby.rs
/// 无条件重置为 Inviter/Waiting），误发会拆掉它正在进行的受理。
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
    let Some(sess) =
        crate::session::session_handle(&app.state::<crate::session::Sessions>(), id)
    else {
        // 会话不在表里（wasm 后端 / 竞态释放）：登记值照存（豁免语义仍成立），
        // 邀请代发无从谈起，不报错——配对任务取不到会话时再给人话错误。
        return Ok(());
    };
    let _g = enter_runtime();
    let front = NativePlayer::new(sess);
    if front.snapshot().get("phase").and_then(serde_json::Value::as_str) == Some("home") {
        front.cmd(UiCommand::CreateInvite);
    }
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

/// `agent_mcp_set(enabled) -> string`：落 store 键并**真启停**内嵌服务器，回 info JSON。
///
/// 启动读 `goptop:agent-mcp-port/token`（token 首次生成并持久化）；配置口被占时
/// [`McpServer::start`] 回退随机口——**实际端口只回写进 info**（url 以真实口拼），
/// 不覆盖用户配置的偏好口，下次启动仍按配置口先试。重复开启先停旧实例（端口/token
/// 可能已改）。**必须异步**：`McpServer::start` 要 await（bind + spawn serve），
/// 同步命令会占死 Tauri 主线程（agent_llm_test 同款理由）。
/// **仅桌面目标**（计划 MCP 节「条件编译」：MCP 相关 Tauri 命令只在桌面注册，
/// 安卓/鸿蒙不编译 MCP 代码；lib.rs 的注册面同门）。
#[cfg(desktop)]
#[tauri::command]
pub async fn agent_mcp_set(app: AppHandle, enabled: bool) -> String {
    mcp_apply(&app, enabled).await
}

/// 启停的完整序列（键面 + 实体）：`agent_mcp_set` 与启动自愈 [`mcp_autostart`]
/// 共用——两条入口对 store/hub 的动作必须一字不差，否则「开关宣称的状态」和
/// 「服务器真实状态」又会分叉。
///
/// 启动读 `goptop:agent-mcp-port/token`（token 首次生成并持久化）；配置口被占时
/// [`McpServer::start`] 回退随机口——**实际端口只回写进 info**（url 以真实口拼），
/// 不覆盖用户配置的偏好口，下次启动仍按配置口先试。重复开启先停旧实例（端口/token
/// 可能已改）。
#[cfg(desktop)]
async fn mcp_apply(app: &AppHandle, enabled: bool) -> String {
    let r = || -> Result<(), String> {
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
    };
    if let Err(e) = r() {
        eprintln!("[agent] mcp_set 落键失败: {e}");
    }
    let hub = app.state::<AgentHub>();
    if enabled {
        // 先停旧实例再起新的：端口/token 以当前 store 为准，旧监听不该残留。
        if let Some(old) = hub.take_mcp_server() {
            old.stop();
        }
        let map = crate::store::store_load(app.clone()).unwrap_or_default();
        let port = map
            .get(KEY_MCP_PORT)
            .and_then(|v| v.trim().parse::<u16>().ok())
            .unwrap_or(MCP_DEFAULT_PORT as u16);
        let token = map
            .get(KEY_MCP_TOKEN)
            .map(String::as_str)
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("")
            .to_string();
        match McpServer::start(McpConfig { port, token }).await {
            Ok(server) => {
                if server.port() != port {
                    eprintln!(
                        "[agent] MCP 端口 {port} 被占，回退随机口 {}（info 以实际口为准）",
                        server.port()
                    );
                }
                hub.set_mcp_server(server);
            }
            Err(e) => eprintln!("[agent] MCP 服务器启动失败（两端口都不可绑）：{e}"),
        }
    } else if let Some(s) = hub.take_mcp_server() {
        // 关闭即全停：serve 任务 + 活局/待局一起拆（agent_stop 只拆对局不停服务器）。
        s.stop();
    }
    mcp_info_json(app)
}

/// 启动自愈：上次退出时 MCP 开关是开的（`goptop:agent-mcp-enabled`），本次启动
/// 照旧把服务器拉起来。该键若只写不读，重启后开关按 store 初值渲染「已启用」而
/// 服务器没跑——此时开局被「请先在 MCP 连接卡打开开关」拒绝，用户面对的正是一
/// 个已经开着的开关（须点关再点开才能恢复）。lib.rs 的 setup 调用；setup 线程
/// 只读键不等待——起停要 await（[`McpServer::start`] 的 bind + spawn serve），
/// 派进传输层运行时的任务里做（agent_start 同款纪律）。
///
/// **仅桌面目标**（KEY_MCP_ENABLED 与 MCP 服务器槽同门）。
#[cfg(desktop)]
pub fn mcp_autostart(app: &AppHandle) {
    let enabled = crate::store::store_load(app.clone())
        .unwrap_or_default()
        .get(KEY_MCP_ENABLED)
        .map(String::as_str)
        == Some("true");
    if !enabled {
        return;
    }
    let app = app.clone();
    {
        let _g = enter_runtime();
        tokio::spawn(async move {
            let info = mcp_apply(&app, true).await;
            eprintln!("[agent] 上次退出时 MCP 开关为开：本次启动自启服务器 → {info}");
        });
    }
}

/// `agent_mcp_info() -> string`：MCP 连接信息 JSON（`{"enabled","url","token"}`）。
/// 服务器在跑时 url/token 取**运行实例**的真实值（端口回退/token 改键后的唯一
/// 真相）；未运行回已配置值，供连接卡展示。
/// **仅桌面目标**（同 [`agent_mcp_set`] 的门）。
#[cfg(desktop)]
#[tauri::command]
pub fn agent_mcp_info(app: AppHandle) -> String {
    mcp_info_json(&app)
}

/// info JSON 的拼装（set/info 共用一份形状，防两处字段漂移）。
#[cfg(desktop)]
fn mcp_info_json(app: &AppHandle) -> String {
    let (enabled, url, token) = match app.state::<AgentHub>().mcp_server() {
        Some(s) => (true, s.url(), Some(s.token().to_string())),
        None => {
            let map = crate::store::store_load(app.clone()).unwrap_or_default();
            let port: u32 =
                map.get(KEY_MCP_PORT).and_then(|v| v.trim().parse().ok()).unwrap_or(MCP_DEFAULT_PORT);
            let token = map.get(KEY_MCP_TOKEN).filter(|t| !t.trim().is_empty()).cloned();
            (false, format!("http://127.0.0.1:{port}/mcp"), token)
        }
    };
    serde_json::json!({
        "enabled": enabled,
        "url": url,
        "token": token,
    })
    .to_string()
}

/* ---------------- AgentHub：运行表与拦截面判据 ---------------- */

/// 运行表 + 绑定槽 + MCP 服务器槽。manage 进 Tauri（lib.rs），命令与任务都经
/// `app.state` 取。
pub struct AgentHub {
    runs: Mutex<HashMap<u32, Arc<Run>>>,
    next_run: AtomicU32,
    /// agent_bind 登记的 A' 会话 id（配对目标；配对任务取走后，豁免续记在
    /// 存活运行的 front_id 上——见 [`AgentHub::exempt_session`]）。
    bound: Mutex<Option<u32>>,
    /// 内嵌 MCP 服务器（agent_mcp_set 真启停；Arc 使认领闭包与观察循环能持引用）。
    /// None = 未启用/已关闭。仅桌面目标存在该槽（mcp feature 同门）。
    #[cfg(desktop)]
    mcp: Mutex<Option<Arc<McpServer>>>,
}

impl Default for AgentHub {
    fn default() -> Self {
        Self {
            runs: Mutex::default(),
            next_run: AtomicU32::new(1),
            bound: Mutex::default(),
            #[cfg(desktop)]
            mcp: Mutex::default(),
        }
    }
}

impl AgentHub {
    /// 现役 MCP 服务器（None=未启用）。
    #[cfg(desktop)]
    fn mcp_server(&self) -> Option<Arc<McpServer>> {
        self.mcp.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 换上新的 MCP 服务器实例（agent_mcp_set(true) 的启动结果）。
    #[cfg(desktop)]
    fn set_mcp_server(&self, s: McpServer) {
        *self.mcp.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(s));
    }

    /// 取走现役实例（重复开启先停旧 / 关闭时全停）。
    #[cfg(desktop)]
    fn take_mcp_server(&self) -> Option<Arc<McpServer>> {
        self.mcp.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

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

    /// 拦截面豁免的 A' 会话 id（计划「会话对」节第 4 条：Agent 局**存活期间**豁免）。
    /// 两段来源：agent_bind 登记值还在绑定槽里（bind → 配对任务取走的窗口）；取走后
    /// 记在存活运行的 `front_id` 上（A' 归前端所有、id 即配对目标）。终局运行
    /// （done/error）不算豁免——局终了 any_live 已放行，拦截面整条不生效。
    pub(crate) fn exempt_session(&self) -> Option<u32> {
        if let Some(id) = *self.bound.lock().unwrap_or_else(|e| e.into_inner()) {
            return Some(id);
        }
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|r| r.is_live())
            .find_map(|r| r.front_id())
    }

    /// 取走绑定值（配对任务的接驳输入；取走即消费，避免影响后续运行——豁免随取走
    /// 一并记到 [`Run::set_front_id`]，局中继续生效）。
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
    /// A' 的会话表 id（配对任务从绑定槽取到登记值后记账；拦截面豁免的局中依据——
    /// 绑定槽已消费，存活局期间靠它保持 A' 豁免）。
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

    /// A' 的会话表 id（拦截面豁免读；配对任务取到登记值前是 None）。
    fn front_id(&self) -> Option<u32> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).front_id
    }

    /// 配对任务取走绑定槽登记值时记账（见 [`AgentHub::exempt_session`]）。
    fn set_front_id(&self, id: u32) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).front_id = Some(id);
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
    // A' 是前端登记的会话（bind），B 在这里创建；方向随执色（模块注的互锁流程）。
    let agent_tauri = Arc::new(crate::session::TauriHost::new(app.clone()));
    let paired = tokio::select! {
        p = pair_seats(&app, &run, &hub, &cfg, seat, agent_tauri.clone()) => match p {
            Ok(p) => p,
            Err(e) => {
                run.set_terminal(ST_ERROR, Some(format!("配对失败：{e}")));
                return;
            }
        },
        // 用户在配对期点了停止：future 被 drop，半途建的会话随局部变量收摊。
        _ = run.cancel.notified() => {
            run.set_terminal(ST_DONE, Some("已停止".into()));
            return;
        }
    };
    let Seats { agent, agent_watch, agent_hook, front_id } = paired;
    let agent_color = seat.opponent().as_str().to_string();
    {
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.player = Some(agent.clone());
        g.agent_color = Some(agent_color.clone());
    }
    run.set_detail(Some(format!("已配对（A' 会话 id={front_id}），Agent 开始思考")));

    // —— 事件物化（player.rs 的纪律：基线在 spawn 前本任务同步取定，晚起的泵
    //    也吞不掉基线之后的事件）。
    let queue = Arc::new(EventQueue::new());
    agent_hook.bind_events(&queue);
    let baseline = event_baseline(&agent_watch);
    tokio::spawn(run_event_pump(EmitWatch::new(agent_watch.clone_rx()), queue.clone(), baseline));

    // —— 工具面：主循环与子代理各持一份 ToolCtx，Arc 底座共享同一局
    //    （Staging 尤其不能有两份——暂存区的一致性建立在单实例上）。
    let staging = Arc::new(Staging::new());
    // agent_status 的 stagedMove 读数必须挂在**同一份** Staging 上：这里的本地
    // Arc 只喂了 ToolCtx 的话，状态回执永远读不到暂存着法，棋盘的幽灵子不画。
    run.inner.lock().unwrap_or_else(|e| e.into_inner()).staging = Some(staging.clone());
    let make_ctx = || ToolCtx {
        player: Arc::new(agent.clone()),
        watch: EmitWatch::new(agent_watch.clone_rx()),
        events: queue.clone(),
        staging: staging.clone(),
        memory: memory.clone(),
        memory_ns: Driver::Builtin.memory_ns(),
        driver: Driver::Builtin,
        subagent_enabled,
        subagent: None,
        on_tool: {
            // 全量记账（含失败与 submit）：事件流是唯一现场——失败红色换行展示报错，
            // submit 详情取暂存内容头（循环在 execute 前已 peek 好）。
            let ring = Arc::clone(&run.events);
            Some(Arc::new(move |tool: &str, ok: bool, ms: u64, head: &str, detail: Option<&str>| {
                ring.push(tool, ok, ms, head.to_string(), detail.map(str::to_string));
            }) as goptop_agent::registry::ToolLogHook)
        },
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
    let ticker_player = agent.clone();
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

/// MCP 模式任务（阶段④）：接驳 A' → 存认领闭包 → waiting_mcp → 观察活局到收口。
///
/// **与内置模式的分工差异**：配对不在这里跑，而是存进服务器的待局槽——外部 Agent
/// `game_start` 时才由 handler 跑 [`claim_seats`]（用已 bind 的 A' + 现建 B 结对，
/// `pair.rs` 同款原语）。理由：MCP 的「对手」在壳外，A' 建好、B 建好都不等于对手
/// 已接入，`waiting_mcp` 必须如实保持到认领那一刻。两个方向：
/// - **我执黑**：bind 先于 agent_start（前端流程），A' 的邀请链接已就绪——直接进
///   等待；B 在认领时以 A' 的链接创建。
/// - **我执白**：B 先建局出链（链接经 detail 送前端、A' 携链 Boot 入局——同内置
///   方向），B 在本任务先行建好（`McpWhiteSeat`），认领闭包只做「A'+B 泵到对局态」。
///
/// 认领后的观察循环（select 硬取消）：winner 出现 → done；live 消失（game_leave
/// 拆局/服务器关闭）→ done「已离场」；`agent_stop` → cancel → 本任务直接退出
/// （认输/拆局/收账都归命令侧，见 agent_stop 步骤 1/3）。
#[cfg(desktop)]
async fn run_mcp_task(app: AppHandle, run: Arc<Run>, cfg: StartCfg, seat: SeatColor) {
    let hub = app.state::<AgentHub>();
    let Some(server) = hub.mcp_server() else {
        run.set_terminal(ST_ERROR, Some("MCP 服务器未启用：请先在 MCP 连接卡打开开关".into()));
        return;
    };

    // —— 我执白：B 席先行建局出链（与 pair_seats 白分支同款；取消经 run 传播）。
    //    B 挂在本任务的 premade 里，直到被认领闭包消费或任务退出随局部变量收摊。
    let premade = if seat == SeatColor::White {
        let agent_tauri = Arc::new(crate::session::TauriHost::new(app.clone()));
        let (agent_hook, agent_watch) = HookHost::wrap(agent_tauri as Arc<dyn Host>);
        let agent_cfg = SessionConfig {
            name: cfg.agent_name.clone().unwrap_or_else(|| "Agent".into()),
            server_mode: false,
            share_origin: SHARE_ORIGIN.into(),
            kind: cfg.kind.clone(),
            size: cfg.size,
        };
        let base = format!("{}/p2p", SHARE_ORIGIN.trim_end_matches('/'));
        let s = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &base));
        s.start_pump();
        let bp = NativePlayer::new(s);
        run.set_detail(Some("Agent 席生成邀请中…".into()));
        bp.cmd(UiCommand::CreateInvite);
        if let Err(e) = wait_invite(&bp, &run).await {
            if run.state() != ST_DONE {
                run.set_terminal(ST_ERROR, Some(format!("B 席出链失败：{e}")));
            }
            return;
        }
        match invite_link(&bp) {
            Some(link) => {
                run.set_detail(Some(format!(
                    "B 已就绪，请以此邀请链接创建 A'（携链 Boot 入局）：{link}"
                )));
                Some(McpWhiteSeat { player: bp, hook: agent_hook, watch: agent_watch })
            }
            None => {
                run.set_terminal(ST_ERROR, Some("B 的邀请链接未就绪（缺 rtc=）".into()));
                return;
            }
        }
    } else {
        None
    };

    // —— 等 bind（黑：bind 先于 agent_start，秒回；白：等前端携链建 A' 后登记）。
    let front_id = match wait_bound(&run, &hub, BIND_WAIT_SECS).await {
        Some(id) => id,
        None => {
            run.set_terminal(ST_ERROR, Some("等待 agent_bind 超时：请先在前端登记 A' 会话".into()));
            return;
        }
    };
    if crate::session::session_handle(&app.state::<crate::session::Sessions>(), front_id).is_none()
    {
        run.set_terminal(
            ST_ERROR,
            Some(format!("登记的 A' 会话（id={front_id}）不在会话表里")),
        );
        return;
    }

    // —— 记忆库（ns=mcp，与内置同一份 db 文件）+ 暂存区单例。staging 必须先于
    //    待局记进运行表：agent_status 的幽灵子读数与认领后工具面 ctx 用的是
    //    同一份（PendingGame.staging），两份 Staging 会让拟落互不相认。
    let db = match crate::store::store_dir(&app) {
        Ok(dir) => dir.join("agent-memory.db"),
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(format!("定位数据目录失败：{e}")));
            return;
        }
    };
    let memory: Arc<dyn VfsStore> = match NativeStore::open(&db) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(format!("记忆库打开失败：{e}")));
            return;
        }
    };
    let staging = Arc::new(Staging::new());
    run.inner.lock().unwrap_or_else(|e| e.into_inner()).staging = Some(staging.clone());

    // —— 存待局：认领闭包（外部 Agent game_start 时跑）。失败路径由 handler 把
    //    待局还回槽并回业务错，外部 Agent 重试 game_start 即可（闭包可重入）。
    server.set_pending_game(PendingGame {
        pairing: {
            let app2 = app.clone();
            let run2 = run.clone();
            let cfg2 = cfg.clone();
            let agent_tauri = Arc::new(crate::session::TauriHost::new(app.clone()));
            Arc::new(move || {
                let (app, run, cfg, agent_tauri, premade) =
                    (app2.clone(), run2.clone(), cfg2.clone(), agent_tauri.clone(), premade.clone());
                Box::pin(claim_seats(app, run, cfg, seat, agent_tauri, front_id, premade))
            })
        },
        memory,
        staging,
    });
    run.set_running(ST_WAITING_MCP);
    run.set_detail(Some(format!("等待 MCP Agent 接入…（A' 会话 id={front_id}）")));

    // —— 认领观察循环。
    let mut was_live = false;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            // 用户停止：agent_stop 已认输（若在局中）、stop_game 拆了装配、终态
            // 已收账——本任务只管退出，不碰状态。
            _ = run.cancel.notified() => return,
        }
        if server.front_handle().is_some() {
            if !was_live {
                was_live = true;
                run.set_detail(Some("MCP Agent 已接入".into()));
            }
            let (won, to_move, agent_color) = {
                let g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
                match g.player.as_ref() {
                    Some(p) => {
                        let s = p.snapshot();
                        (s.get("winner").is_some_and(|w| !w.is_null()),
                         s.get("toMove").and_then(serde_json::Value::as_str).map(str::to_string),
                         g.agent_color.clone())
                    }
                    None => (false, None, None),
                }
            };
            if won {
                run.set_terminal(ST_DONE, Some("对局结束".into()));
                return;
            }
            // 轮到 B=thinking、轮到人=waiting——与内置 ticker 同口径的诚实近似
            // （外部 Agent 的「思考」本侧不可见，用快照 toMove 反推）。
            if to_move.as_deref() == agent_color.as_deref() {
                run.set_running(ST_THINKING);
            } else {
                run.set_running(ST_WAITING);
            }
        } else if was_live {
            // live 消失 = game_leave 拆局（外部 Agent 先认输后离场）或服务器被关。
            run.set_terminal(ST_DONE, Some("MCP Agent 已离场".into()));
            return;
        }
    }
}

/// 非桌面目标：mcp feature 不开、`goptop_agent::mcp` 不编译，显式降级（Driver::Mcp
/// 的 cfgJson 在移动端只会得到这条人话错误，绝不静默）。
#[cfg(not(desktop))]
async fn run_mcp_task(_app: AppHandle, run: Arc<Run>, _cfg: StartCfg, _seat: SeatColor) {
    run.set_terminal(ST_ERROR, Some("MCP 对战仅桌面版可用".into()));
}

/// 我执白方向的预建 B 席（认领闭包的现成材料；`Clone` 手工实现——EmitWatch 按
/// clone_rx 克隆，多份接收端共享同一 watch 发送端，语义不变）。
#[cfg(desktop)]
struct McpWhiteSeat {
    player: NativePlayer,
    hook: Arc<HookHost>,
    watch: EmitWatch,
}

#[cfg(desktop)]
impl Clone for McpWhiteSeat {
    fn clone(&self) -> Self {
        Self {
            player: self.player.clone(),
            hook: self.hook.clone(),
            watch: EmitWatch::new(self.watch.clone_rx()),
        }
    }
}

/// MCP 认领的配对段（待局闭包体；game_start 时由 goptop-agent 的 handler 调）：
/// 用已 bind 的 A' 完成配对，产出 [`pair::Paired`] 交给 handler 装配活局。
///
/// **我执黑**：A' 已有邀请链接（bind 代发 + 前端等过 rtc；这里兜底补发），携链现建
/// B；**我执白**：B 已预建（[`McpWhiteSeat`]），只差把两席泵到对局态。取消与终局
/// 经 `run.cancel`/`run.state` 传播——用户在认领配对途中停止时返回 Err，handler
/// 把待局还回槽（待局里的运行已终，重试认领会被开头的存活检查拦下）。
#[cfg(desktop)]
async fn claim_seats(
    app: AppHandle,
    run: Arc<Run>,
    cfg: StartCfg,
    seat: SeatColor,
    agent_tauri: Arc<crate::session::TauriHost>,
    front_id: u32,
    premade: Option<McpWhiteSeat>,
) -> Result<pair::Paired, String> {
    if !run.is_live() {
        return Err("对局已被用户停止，本次认领无效".into());
    }
    let front = take_front(&app, front_id)?;
    let agent_cfg = SessionConfig {
        name: cfg.agent_name.clone().unwrap_or_else(|| "Agent".into()),
        server_mode: false,
        share_origin: SHARE_ORIGIN.into(),
        kind: cfg.kind.clone(),
        size: cfg.size,
    };
    let paired = match seat {
        SeatColor::Black => {
            if invite_link(&front).is_none() {
                front.cmd(UiCommand::CreateInvite);
            }
            run.set_detail(Some("MCP Agent 认领中：等待 A' 的邀请链接就绪…".into()));
            wait_invite(&front, &run).await?;
            let link =
                invite_link(&front).ok_or_else(|| "A' 的邀请链接未就绪（缺 rtc=）".to_string())?;
            let (agent_hook, agent_watch) = HookHost::wrap(agent_tauri as Arc<dyn Host>);
            let s = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &link));
            s.start_pump();
            let agent = NativePlayer::new(s);
            record_b_seat(&run, &agent, seat);
            wait_playing(&front, &agent, &run).await?;
            pair::Paired { front, agent, agent_watch, agent_hook }
        }
        SeatColor::White => {
            let b = premade.ok_or_else(|| "B 席未预建（内部装配错误）".to_string())?;
            run.set_detail(Some("MCP Agent 认领中：等待人席完成入局…".into()));
            record_b_seat(&run, &b.player, seat);
            wait_playing(&front, &b.player, &run).await?;
            pair::Paired {
                front,
                agent: b.player,
                agent_watch: EmitWatch::new(b.watch.clone_rx()),
                agent_hook: b.hook,
            }
        }
    };
    Ok(paired)
}

/// 认领配对成功时把 B 席记进运行表：agent_stop 的认输落点、状态 ticker 的
/// thinking/waiting 判定（agent_color）都靠它——与内置模式 run_builtin_task 的
/// 记账同款。staging 在存待局时已记账（见 run_mcp_task）。
#[cfg(desktop)]
fn record_b_seat(run: &Run, agent: &NativePlayer, seat: SeatColor) {
    let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
    g.player = Some(agent.clone());
    g.agent_color = Some(seat.opponent().as_str().to_string());
}

/* ---------------- 会话对接线（pair.rs 同款原语；A' 归前端，B 由本壳创建） ---------------- */

/// 配对成功后的 B 席与其事件源头（goptop-agent pair::Paired 的本壳变体——
/// A' 归前端，不在此列；front_id 只是拦截面豁免的登记值）。
struct Seats {
    /// B（Agent 的无头会话；决策循环与工具层经 PlayerHandle 用它）。
    agent: NativePlayer,
    /// B 的快照推送流（事件物化的源头）。
    agent_watch: EmitWatch,
    /// B 的装饰宿主（bind 事件队列用；生命周期与整局同长）。
    agent_hook: Arc<HookHost>,
    /// A' 的会话表 id（= agent_bind 的登记值；状态 detail 文案用——豁免记账在
    /// Run 上，见 [`AgentHub::exempt_session`]）。
    front_id: u32,
}

/// 等 agent_bind 登记值出现的时长（覆盖前端的等待/建会话/登记全链路：
/// 我执白方向前端 waitAgentLink 50s 后才 bind，本值留足余量）。
const BIND_WAIT_SECS: u64 = 90;

/// 等 agent_bind 的登记值（轮询 hub 的绑定槽；取消经 run.cancel 传播）。
/// 取走即在本运行的 `front_id` 记账——绑定槽消费后，拦截面豁免靠它续到局终。
async fn wait_bound(run: &Run, hub: &AgentHub, secs: u64) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(id) = hub.take_bound() {
            run.set_front_id(id);
            return Some(id);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = run.cancel.notified() => return None,
        }
    }
}

/// 结对两席（内置模式）。
///
/// **我执黑**：A'（前端登记）已由 agent_bind 代发邀请——等它的 inviteUrl 含 rtc
/// （未发则补发一次），携链创建 B；**我执白**：B 先建局邀请，链接经 `detail` 送
/// 前端，等 agent_bind 接驳 A'（携链 Boot 的回执经进程内 presence 自动到 B）。
/// 两个方向最后都泵到双端 `phase=="playing" && peerConnected`（headless.rs 验证
/// 过的同一条路径；在此之前落子会被 can_place 静默拒绝）。
async fn pair_seats(
    app: &AppHandle,
    run: &Run,
    hub: &AgentHub,
    cfg: &StartCfg,
    seat: SeatColor,
    agent_tauri: Arc<crate::session::TauriHost>,
) -> Result<Seats, String> {
    // B 席的宿主基座进 HookHost（临时 userId / stun 空 / emit 推 watch）——
    // 与 pair.rs 同一纪律：调用方不预包，wrap 的临时 ID 才生效。
    let (agent_hook, agent_watch) = HookHost::wrap(agent_tauri as Arc<dyn Host>);
    let agent_cfg = SessionConfig {
        name: cfg.agent_name.clone().unwrap_or_else(|| "Agent".into()),
        server_mode: false,
        share_origin: SHARE_ORIGIN.into(),
        kind: cfg.kind.clone(),
        size: cfg.size,
    };

    // 方向决定先后（模块注的互锁流程；每分支自取 A' 的登记值并建 B 会话）：
    // Black 先等登记（bind 先于 agent_start），White 先出链（bind 后于 agent_start）
    // ——两段 wait_bound 不能对调，否则就是「等一个只有出链才会发生的登记」死锁。
    let (front, front_id, agent_session) = match seat {
        SeatColor::Black => {
            let front_id = wait_bound(run, hub, BIND_WAIT_SECS)
                .await
                .ok_or_else(|| "等待 agent_bind 超时：请先在前端登记 A' 会话".to_string())?;
            let front = take_front(app, front_id)?;
            // 邀请方是 A'：agent_bind 已代发（空置时）；这里兜底补发并等 rtc 编入链接。
            if invite_link(&front).is_none() {
                front.cmd(UiCommand::CreateInvite);
            }
            run.set_detail(Some("等待 A' 的邀请链接就绪…".into()));
            wait_invite(&front, run).await?;
            let link =
                invite_link(&front).ok_or_else(|| "A' 的邀请链接未就绪（缺 rtc=）".to_string())?;
            let s = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &link));
            s.start_pump();
            (front, front_id, s)
        }
        SeatColor::White => {
            // B 先建局邀请（基座 /p2p），链接经 detail 送前端（AgentPage 的
            // extractInviteLink 取含 rtc= 的 URL），前端携链 session_new 创建 A'
            //（Boot 路径，session_new 的 href 参数本就支持）后 agent_bind 接驳；
            // A' 的回执经进程内 presence 自动到 B，无需本任务转发。
            let base = format!("{}/p2p", SHARE_ORIGIN.trim_end_matches('/'));
            let s = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &base));
            s.start_pump();
            let bp = NativePlayer::new(s.clone());
            run.set_detail(Some("Agent 席生成邀请中…".into()));
            bp.cmd(UiCommand::CreateInvite);
            wait_invite(&bp, run).await?;
            let link =
                invite_link(&bp).ok_or_else(|| "B 的邀请链接未就绪（缺 rtc=）".to_string())?;
            run.set_detail(Some(format!(
                "B 已就绪，请以此邀请链接创建 A'（携链 Boot 入局）：{link}"
            )));
            let front_id = wait_bound(run, hub, BIND_WAIT_SECS)
                .await
                .ok_or_else(|| "等待前端携链创建 A' 并 agent_bind 超时（90 秒）".to_string())?;
            (take_front(app, front_id)?, front_id, s)
        }
    };

    let agent = NativePlayer::new(agent_session);
    wait_playing(&front, &agent, run).await?;
    // A'（front）不进 Seats：它的表项与泵都归前端，结对完成即可放手。
    Ok(Seats { agent, agent_watch, agent_hook, front_id })
}

/// 从会话表取登记的 A'（配对目标；表项归前端，这里只借句柄）。
fn take_front(app: &AppHandle, front_id: u32) -> Result<NativePlayer, String> {
    let arc = crate::session::session_handle(&app.state::<crate::session::Sessions>(), front_id)
        .ok_or_else(|| format!("登记的 A' 会话（id={front_id}）不在会话表里"))?;
    Ok(NativePlayer::new(arc))
}

/// 等一条会话的 inviteUrl 编入 rtc（≤40s；pair.rs 同口径同文案）。边泵边等。
async fn wait_invite(p: &NativePlayer, run: &Run) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        p.pump();
        if invite_link(p).is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            // 取消竞窗：停止与超时同时到时，停止优先（不再报一条误导性的超时）。
            if run.state() == ST_DONE {
                return Err("已停止".into());
            }
            return Err("40 秒内未生成含 rtc 的邀请链接（ICE gathering 未完成？）".into());
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            _ = run.cancel.notified() => return Err("已停止".into()),
        }
    }
}

/// 泵两席到双双进入对局态（≤40s；pair.rs 的 wait_playing 同款与同文案）。
async fn wait_playing(front: &NativePlayer, agent: &NativePlayer, run: &Run) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        front.pump();
        agent.pump();
        let (fs, as_) = (front.snapshot(), agent.snapshot());
        if str_of(&fs, "phase") == "playing"
            && str_of(&as_, "phase") == "playing"
            && connected(&fs)
            && connected(&as_)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            if run.state() == ST_DONE {
                return Err("已停止".into());
            }
            // 失败带两席现场：卡在 waiting=信令没走到、playing 但未连=ICE 没通。
            return Err(format!(
                "40 秒内未双双进入对局态：front phase={} connected={} / agent phase={} connected={}",
                str_of(&fs, "phase"),
                connected(&fs),
                str_of(&as_, "phase"),
                connected(&as_),
            ));
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            _ = run.cancel.notified() => return Err("已停止".into()),
        }
    }
}

/// 邀请链接是否就绪（`rtc=` 参数在场——同源链接一开始就有，但不含 rtc 不算就绪）。
fn invite_link(p: &NativePlayer) -> Option<String> {
    p.snapshot()
        .get("inviteUrl")
        .and_then(serde_json::Value::as_str)
        .filter(|u| u.contains("rtc="))
        .map(str::to_string)
}

fn str_of(s: &serde_json::Value, key: &str) -> String {
    s.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string()
}

fn connected(s: &serde_json::Value) -> bool {
    s.get("peerConnected").and_then(serde_json::Value::as_bool) == Some(true)
}

/* ---------------- 配置装载（store → llm 层类型；纯函数便于单测） ---------------- */

/// store 里 goptop:llm-config 的 JSON 形态（与前端设置卡同键同形）。
/// protocol 先收字符串再归一（见 [`parse_protocol`]——拼写变体在线上出现过）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmCfgJson {
    protocol: String,
    base_url: String,
    model: String,
    max_output_tokens: Option<u32>,
    reply_lang: Option<String>,
    enable_subagent: Option<bool>,
    effort: Option<String>,
    debug: Option<bool>,
    stream: Option<bool>,
}

/// 协议拼写归一：goptop-agent 的 Protocol serde 值域是 snake_case
/// （`anthropic` / `open_ai_responses` / `open_ai_chat`），而前端 AgentPage 写入的
/// 拼写是 kebab 风格（`openai-responses` / `openai-chat`）——两路同键同字段不同拼，
/// 归一在读取侧做（唯一读者），分隔符与大小写一律抹平后比对。
fn parse_protocol(raw: &str) -> Result<Protocol, String> {
    let t: String = raw.trim().to_lowercase().chars().filter(|c| *c != '-' && *c != '_').collect();
    match t.as_str() {
        "anthropic" => Ok(Protocol::Anthropic),
        "openairesponses" => Ok(Protocol::OpenAiResponses),
        "openaichat" => Ok(Protocol::OpenAiChat),
        _ => Err(format!(
            "LLM 配置的 protocol 无法识别：{raw:?}（可选 anthropic / openai-responses / openai-chat）"
        )),
    }
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
        protocol: parse_protocol(&j.protocol)?,
        base_url: j.base_url.trim().trim_end_matches('/').to_string(),
        model: j.model.trim().to_string(),
        max_output_tokens: j.max_output_tokens.unwrap_or(1024).max(64),
        reply_lang: Some(
            j.reply_lang
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ui_lang.to_string()),
        ),
        effort: goptop_agent::llm::sanitize_effort(&j.effort),
        debug: j.debug.unwrap_or(false),
        stream: j.stream.unwrap_or(false),
    };
    if cfg.base_url.is_empty() || cfg.model.is_empty() {
        return Err("LLM 配置不完整：端点与模型都不能为空".into());
    }
    Ok((cfg, key, ctx_limit, j.enable_subagent.unwrap_or(false)))
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
    detail: Option<String>,
}

impl EventRing {
    fn push(&self, tool: &str, ok: bool, ms: u64, summary: String, detail: Option<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.items.len() >= 200 {
            g.items.pop_front();
        }
        g.items.push_back(RingItem { ts: now_ms(), tool: tool.to_string(), ok, ms, summary, detail });
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
                serde_json::json!({"ts": it.ts, "tool": it.tool, "ok": it.ok, "ms": it.ms, "summary": it.summary, "detail": it.detail})
            })
            .collect();
        (g.total, items)
    }
}

/// 玩家命令包装已退场：UiCommand 级的记账由循环的 on_tool 全量钩子承担
///（submit 的 head=路径、detail=暂存内容头），双层记账只会两行一动作。

/// UiCommand → 日志的 (工具名, 摘要)：已随 LogPlayer 退场——UiCommand 级记账由
/// 循环的 on_tool 全量钩子承担（submit 的 head=路径、detail=暂存内容头），
/// 双层记账只会两行一动作。

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
            // 成功调用不进环（次数在统计行）；**失败必须进**（红色详情行——
            // 环里 llm ok=false 是「卡局诊断」的第一现场）。
            Ok((status, text)) if *status < 400 => self.ring.push(
                "llm", true, ms,
                format!("第 {n} 次模型调用：HTTP {status}"), None,
            ),
            Ok((status, text)) => self.ring.push(
                "llm", false, ms,
                format!("第 {n} 次模型调用：HTTP {status}"),
                Some(trunc_err(text)),
            ),
            Err(e) => self.ring.push("llm", false, ms, format!("第 {n} 次模型调用失败"), Some(e.clone())),
        }
        result
    }
}

/// 错误详情钳制（前端还会再钳一次；源头收口防超长 HTML 灌环）。
fn trunc_err(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('<') {
        return format!("<非 JSON 响应：{} 字节>", text.len());
    }
    let single: String = t.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
    let mut out: String = single.chars().take(160).collect();
    if single.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// 记忆存储包装：删除记日志（写已由 on_tool 的 Write 行覆盖，重复即两行一动作）。
struct LogStore {
    inner: NativeStore,
    ring: Arc<EventRing>,
}

impl VfsStore for LogStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        self.inner.read(ns, path)
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        self.inner.write(ns, path, content)
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        let r = self.inner.delete(ns, path);
        self.ring.push(
            "memory_delete",
            r.as_ref().is_ok_and(|&d| d),
            0,
            format!("/memory/{path}"),
            None,
        );
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
            ring.push("llm", true, i, format!("第 {i} 次"), None);
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

    /* ---- 协议拼写归一（F/G store 缝：前端 kebab、crate serde snake_case） ---- */

    #[test]
    fn 协议拼写归一_kebab与snake与大小写() {
        assert!(matches!(parse_protocol("anthropic"), Ok(Protocol::Anthropic)));
        assert!(matches!(parse_protocol("Anthropic"), Ok(Protocol::Anthropic)));
        assert!(matches!(parse_protocol("openai-responses"), Ok(Protocol::OpenAiResponses)));
        assert!(matches!(parse_protocol("openai_responses"), Ok(Protocol::OpenAiResponses)));
        assert!(matches!(parse_protocol("openai-chat"), Ok(Protocol::OpenAiChat)));
        assert!(matches!(parse_protocol("open_ai_chat"), Ok(Protocol::OpenAiChat)));
        assert!(parse_protocol("claude").is_err(), "未知协议要给人话错误");
        // 走全链路：前端写的 kebab 拼在 parse_llm_cfg 里也能吃下。
        let map = cfg_map(&[
            (KEY_LLM_CONFIG, r#"{"protocol":"openai-chat","baseUrl":"http://h","model":"m"}"#),
            (KEY_LLM_KEY, "k"),
        ]);
        let (cfg, ..) = parse_llm_cfg(&map, "zh").expect("kebab 拼写应可解析");
        assert!(matches!(cfg.protocol, Protocol::OpenAiChat));
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

    /* ---- 拦截面豁免：绑定槽消费后由存活运行的 front_id 续到局终 ---- */

    #[test]
    fn 拦截面豁免_绑定槽与存活运行续期() {
        let hub = AgentHub::default();
        *hub.bound.lock().unwrap() = Some(7);
        assert_eq!(hub.exempt_session(), Some(7), "bind 后、配对取走前：豁免在绑定槽");
        // wait_bound 的语义：取走即消费，同时在运行上记账。
        let run = Arc::new(Run::new(1));
        if let Some(id) = hub.take_bound() {
            run.set_front_id(id);
        }
        assert_eq!(hub.take_bound(), None, "取走即消费，不影响后续运行");
        assert_eq!(hub.exempt_session(), None, "未记账前豁免断了——修复前局中正是这个洞");
        hub.insert(run.clone());
        assert_eq!(hub.exempt_session(), Some(7), "存活运行记账后续期到局终");
        run.set_terminal(ST_DONE, None);
        assert_eq!(hub.exempt_session(), None, "终局运行不算豁免（局终了 any_live 已放行）");
    }
}
