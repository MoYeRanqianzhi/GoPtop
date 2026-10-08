//! MCP 出口 —— 外部 Agent 经内嵌 Streamable HTTP 服务器驱动 B 席（feature "mcp"）。
//!
//! **条件编译（计划「MCP 模式」节·用户拍板）**：`mcp` feature 只在桌面目标启用
//! （src-tauri 按 `[target.'cfg(desktop)'.dependencies]` 挂 feature）；安卓/鸿蒙与
//! wasm 完全不编译本模块（不是「编译了但禁用」）。TCP listener 是桌面固有的，
//! 服务器大厅注册属下一任务（registry 的 scope 枚举与 handler-state 已留缝）。
//!
//! 形态对齐范本 `.ref/codex/codex-rs/tui/src/dynamic_tools_mcp.rs`：
//! rmcp `StreamableHttpService` + `Router::nest_service("/mcp")` + Bearer token
//! 中间件 + `axum::serve`。工具表来自 [`crate::tools::tools_for`]（Driver::Mcp，
//! subagent=false——外部 Agent 无 LLM 配置，delegate 无意义），`list_tools` 的
//! 只读注解照 [`crate::tools::ToolDef::read_only`]；工具执行**共用** [`crate::registry::execute`]
//! 一口——`RespondToModel` → `CallToolResult::error`（isError 文本，模型自己读着改），
//! `Fatal` → `McpError::internal_error`（JSON-RPC 协议错）；image 变体回执走
//! MCP `type:"image"` content（base64，承载能力矩阵：MCP ✓）。
//!
//! 会话对生命周期接线在本模块的 handler-state（[`McpShared`]，registry.rs 头注
//! 预留的「阶段④ handler 层」）：UI 侧 [`McpServer::set_pending_game`] 存入待局
//! 装配（棋种/路数/执色以用户 UI 配置为权威），外部 Agent 的 `game_start` 认领并
//! 跑待局里的**配对执行体**（[`PendingGame::pairing`]）——crate 自测用
//! [`PendingGame::from_pair`]（跑同一 [`crate::pair::pair`]，自建两席）；桌面壳的
//! A' 归前端会话表，由壳提供自己的配对闭包（pair.rs 同款原语结对其已登记的 A'）。
//! `game_leave` 收尾拆局。人侧未就绪 → 业务错误文本
//! （[`registry::GAME_START_NO_GAME`]），与工具面同一份。
//!
//! **rmcp 3.2.0 的 feature 实际拼写**（实施首日双重验证：下载源
//! `~/.cargo/registry/src/*/rmcp-3.2.0/Cargo.toml` 的 `[features]` 定义 + 范本
//! tui/Cargo.toml 同款挂法，与计划预写的拼写一致，零调整）：`server` +
//! `transport-streamable-http-server`。default-features 由 workspace 统一全关
//! （auth/schemars 宏面/client 全不进树；`server` 自身要求的 schemars 依赖是
//! rmcp 内部实现细节，与本仓「不引 schemars」纪律无关——本仓 schema 仍手写）。

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::middleware;
use axum::middleware::Next;
use axum::response::Response;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::model::Implementation;
use rmcp::model::JsonObject;
use rmcp::model::ListToolsResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::model::Tool;
use rmcp::model::ToolAnnotations;
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::player::{EmitWatch, EventQueue, NativePlayer, event_baseline, run_event_pump};
use crate::registry::{self, GAME_START_NO_GAME, ToolCtx, ToolError};
use crate::store::VfsStore;
use crate::tools;
use crate::vfs::Staging;
use crate::{Driver, pair};

/// 「有 MCP 出口但还没有活局」时除 game_start 外一切工具的业务错误文案
/// （外部 Agent 没认领席位就想 read/move——业务错误、非协议错， isError 回给它）。
const NO_LIVE_GAME: &str =
    "no live game — call game_start first; the user sets the game up from the AgentPage.";

/// 待局装配 —— UI 侧（AgentHub）在「开始」时经 [`McpServer::set_pending_game`]
/// 存入，外部 Agent 的 `game_start` 认领。
pub struct PendingGame {
    /// 配对执行体：认领时跑一次、产出配对产物（失败还回待局，重试会再跑一次——
    /// 所以是可重入的 [`Fn`]）。做成闭包的唯一理由：壳层（src-tauri）的 A' 归前端
    /// 会话表，[`pair::pair`] 自建两席的路径接不进去（见模块注）；两条装配路共用
    /// 同一个 handler 前置，棋种/路数/执色仍以用户 UI 配置为权威（闭包在 UI 侧
    /// 装配时就带定了）。
    pub pairing: PairFn,
    /// /memory 的存储后端（ns 按 [`Driver::Mcp.memory_ns`]=`"mcp"`）。
    pub memory: Arc<dyn VfsStore>,
    /// 暂存区**单例**：工具面 ctx 与壳层的状态回执（幽灵子读数）必须共用同一份
    /// ——两份 Staging 会让「工具面 write 的暂存」与「页面看到的拟落」互不相认。
    pub staging: Arc<Staging>,
}

/// 配对执行体的返回 future（Box 装箱，免泛型散布到 handler 与 [`McpServer`] 面）。
pub type PairFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<pair::Paired, String>> + Send>>;

/// 配对执行体：可重入的异步闭包（认领失败还回待局后，重试 game_start 会再跑一次）。
pub type PairFn = Arc<dyn Fn() -> PairFuture + Send + Sync>;

impl PendingGame {
    /// 自配待局：认领时跑 [`pair::pair`]（自建两席）——crate 测试与「A' 无外部
    /// 会话表」场景的便捷构造；暂存区现场新建（单例归本待局私有，无人外借）。
    #[must_use]
    pub fn from_pair(cfg: pair::PairConfig, memory: Arc<dyn VfsStore>) -> Self {
        Self {
            pairing: Arc::new(move || Box::pin(pair::pair(cfg.clone()))),
            memory,
            staging: Arc::new(Staging::new()),
        }
    }
}

/// 活局 —— `game_start` 认领后的状态。
struct LiveGame {
    /// A'（人席）：壳层经 [`McpServer::front_handle`] 取去渲染/轮询。
    front: NativePlayer,
    /// 工具执行上下文（driver=Mcp）；槽内不外借，每次 tools/call 克隆一份。
    ctx: ToolCtx,
    /// 事件物化泵（watch diff → 队列/全量历史）。game_leave 显式 abort；
    /// 局散（会话 drop → watch sender 全灭）时它也自然收摊，abort 只是定拍。
    pump: JoinHandle<()>,
}

/// handler 与 [`McpServer`] 共享的可变状态。
#[derive(Clone)]
struct McpShared {
    /// 生命周期槽（待局/活局的唯一真相）。**std 锁、临界区内零 await**——
    /// 持锁跨 await 的只有 game_start 的认领段，那把锁是下面的 [`Self::claim`]。
    lifecycle: Arc<StdMutex<Lifecycle>>,
    /// game_start 认领互斥：pair() 是秒级 await（ICE 等待），两个客户端同时
    /// 认领必须串行——后到者在锁上等，进去后看到活局已建、直接走工具面。
    claim: Arc<AsyncMutex<()>>,
}

#[derive(Default)]
struct Lifecycle {
    pending: Option<PendingGame>,
    live: Option<LiveGame>,
}

impl McpShared {
    /// 存入待局（同一时刻只留最新一局——UI 重开即覆盖）。
    fn set_pending(&self, game: PendingGame) {
        self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending = Some(game);
    }

    /// 取走待局（认领时消费；pair 失败时 [`Self::put_back_pending`] 还回去，
    /// 外部 Agent 重试 game_start 才有的认领）。
    fn take_pending(&self) -> Option<PendingGame> {
        self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending.take()
    }

    /// pair 失败还回待局（失败→清理、可重试——清理的是半成品会话，装配参数不丢）。
    fn put_back_pending(&self, game: PendingGame) {
        self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending = Some(game);
    }

    /// 活局 ctx 的克隆（槽内不外借——`ToolCtx` 克隆共享同一局，见 registry 注）。
    fn live_ctx(&self) -> Option<ToolCtx> {
        self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).live.as_ref().map(|l| l.ctx.clone())
    }

    /// A' 的句柄（壳层渲染用；`NativePlayer` 克隆即多一份同会话引用）。
    fn front_handle(&self) -> Option<NativePlayer> {
        self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).live.as_ref().map(|l| l.front.clone())
    }

    /// 拆活局：abort 事件泵，front/ctx 就地 drop——两席会话的 Drop 置停机标志，
    /// RTC 连接与订阅任务随停（pair.rs 的清理语义，局散配对即散）。
    fn teardown(&self) {
        if let Some(live) = self.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).live.take() {
            live.pump.abort();
        }
    }
}

/// MCP 服务器句柄 —— 起停与装配的唯一入口（lib.rs 对外暴露的面）。
///
/// 生命周期：[`McpServer::start`] 绑定 listener 并 spawn `axum::serve` 任务；
/// [`McpServer::stop`] / `Drop` abort 该任务并拆掉活局/待局（范本
/// dynamic_tools_mcp.rs:176-180 的 Drop abort 手法）。
pub struct McpServer {
    /// 实际绑定端口（9537 被占回退随机口后，这就是回传给 UI 的真实值）。
    port: u16,
    token: String,
    task: JoinHandle<()>,
    shared: McpShared,
}

/// 启动配置 —— 端口可配（计划 `goptop:agent-mcp-port`，默认 9537）、token 随机
/// 持久化（计划 `goptop:agent-mcp-token`，UI 可重新生成；落盘归壳层设置链）。
pub struct McpConfig {
    /// 绑定端口；0 = 直接随机口。被占时回退随机口（实际端口经 [`McpServer::port`]
    /// 回传，UI 据此生成 `.mcp.json` 片段）。
    pub port: u16,
    /// Bearer token（中间件比对的原始值，不含 `"Bearer "` 前缀）。
    pub token: String,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self { port: 9537, token: generate_token() }
    }
}

/// 随机 Bearer token：transport 的 `rand4()`（进程级 RandomState 熵源，HookHost
/// 的临时 userId 同一条路，防同毫秒撞车已验证）→ 4×u32 → 32 个十六进制字符。
/// 128 位熵与 UUID v4 同量级，零新增依赖。
fn generate_token() -> String {
    let r = goptop_transport_native::rand4();
    format!("{:08x}{:08x}{:08x}{:08x}", r[0], r[1], r[2], r[3])
}

impl McpServer {
    /// 起服务器：绑定 127.0.0.1、装 Bearer 中间件、spawn serve 任务。
    ///
    /// 端口回退（计划：占用回退随机口并把实际端口回传）：指定口 bind 失败即改
    /// `:0` 重绑一次——回退口只是「能听」，UI 必须按 [`Self::port`] 的实际值生成
    /// 连接串，而不是假设拿到了配置口。
    ///
    /// # Errors
    /// 两端口都 bind 失败（端口资源耗尽等系统级问题）时透传 io::Error。
    pub async fn start(cfg: McpConfig) -> std::io::Result<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", cfg.port)).await {
            Ok(l) => l,
            Err(e) if cfg.port != 0 => TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| e)?,
            Err(e) => return Err(e),
        };
        let port = listener.local_addr()?.port();
        let shared = McpShared {
            lifecycle: Arc::default(),
            claim: Arc::new(AsyncMutex::new(())),
        };
        let handler = McpHandler { shared: shared.clone() };
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        // 范本同款：/mcp 挂服务、全路由包 Bearer 校验（先鉴权后路由）。
        let router = Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn_with_state(
                Arc::new(format!("Bearer {}", cfg.token)),
                require_authorization,
            ));
        // serve 只在 accept 出系统级错误时退出；abort（stop/Drop）是唯一停法。
        // 退出仅静默：桌面壳此时多在拆局，告警无处可去（本 crate 无日志面）。
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { port, token: cfg.token, task, shared })
    }

    /// 实际绑定端口（回退随机口后与配置口不同——连接串以此为准）。
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Streamable HTTP 端点 URL（`.mcp.json` 片段的 url 字段）。
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// Bearer token（`.mcp.json` 片段的 Authorization 头取值）。
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 存入待局装配（用户点「开始」后由壳层调用；重复调用覆盖旧待局）。
    pub fn set_pending_game(&self, game: PendingGame) {
        self.shared.set_pending(game);
    }

    /// A' 的句柄 —— 外部 Agent `game_start` 认领成功后，壳层取去渲染棋盘与轮询
    /// 快照。无活局回 None（壳层据此显示「等待 MCP Agent 接入…」）。
    #[must_use]
    pub fn front_handle(&self) -> Option<NativePlayer> {
        self.shared.front_handle()
    }

    /// 停服务器：abort serve 任务并拆活局/待局。`Drop` 兜底同款。
    pub fn stop(&self) {
        self.task.abort();
        self.shared.teardown();
        self.shared.take_pending();
    }

    /// 只拆对局、不停服务器（壳层 agent_stop / 关闭对局时的收尾口；认输的规则面
    /// 归调用方——与 [`Self::stop`] 的差别仅在不 abort serve 任务）。拆局即把
    /// 工具面撤走：后续 tools/call 回「no live game」，阻塞中的 wait_events 以
    /// 已物化的终局事件先行返回后同样落到这条业务错。
    pub fn stop_game(&self) {
        self.shared.teardown();
        self.shared.take_pending();
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        // abort 即拆：serve 任务 drop 时路由/服务/handler/shared 的 Arc 一起落。
        // 活局的会话停机靠会话自身 Drop（最后一份引用消失时），这里不再强拆——
        // 壳层可能还持 A' 句柄渲染终局画面。
        self.task.abort();
    }
}

/// Bearer 中间件 —— 范本 dynamic_tools_mcp.rs:182-196 逐字同款：头缺/值不等即 401。
/// 常量时间比对在此 threat model（本机 loopback + 本地 UI 持 token）不做要求。
async fn require_authorization(
    State(expected): State<Arc<String>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if request
        .headers()
        .get(AUTHORIZATION)
        .is_some_and(|value| value.as_bytes() == expected.as_bytes())
    {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// ServerHandler 实现 —— list_tools/call_tool 全部转述到本 crate 既有面
/// （tools::tools_for 与 registry::execute），本模块不做第二套工具语义。
#[derive(Clone)]
struct McpHandler {
    shared: McpShared,
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("goptop-agent", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // MCP 工具面：tools_for(Driver::Mcp, subagent=false) —— 8 个（文件族 5 +
        // wait_events/game_start/game_leave；delegate 仅内置）。read_only 注解是
        // ToolDef 的静态事实，两处同源永不分叉。
        let mut out = Vec::new();
        for def in tools::tools_for(Driver::Mcp, false) {
            // 手写 schema 是字符串常量：解析失败=工具负责人写坏了 schema（编译期
            // 不可查），internal_error 炸在 tools/list 正好让自测第一时间显形。
            let schema: JsonObject = serde_json::from_str(def.input_schema)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            let mut tool = Tool::new(def.name, def.description, Arc::new(schema));
            tool.annotations = Some(ToolAnnotations::new().read_only(def.read_only));
            out.push(tool);
        }
        Ok(ListToolsResult::with_all_items(out))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        let name = request.name.as_ref();
        // game_start 前置接线（registry.rs 头注预留的 handler 层）：认领待局并
        // 跑同一 pair()。人侧未就绪 → 业务错误文本（isError），不是协议错。
        if name == "game_start" {
            if let Err(msg) = self.ensure_game().await {
                return Ok(CallToolResult::error(vec![ContentBlock::text(msg)]).into());
            }
        }
        let Some(ctx) = self.shared.live_ctx() else {
            let msg = if name == "game_start" {
                GAME_START_NO_GAME.to_string()
            } else {
                NO_LIVE_GAME.to_string()
            };
            return Ok(CallToolResult::error(vec![ContentBlock::text(msg)]).into());
        };
        let args = serde_json::Value::Object(request.arguments.unwrap_or_default());
        match registry::execute(name, &args, &ctx).await {
            Ok(receipt) => {
                if name == "game_leave" {
                    // 收尾拆局（计划：game_leave 局中自动认输后离场，配对随局散）。
                    // registry 已完成认输的规则面，这里只拆装配。
                    self.shared.teardown();
                }
                // image 变体回执 → MCP type:"image" content（base64 已在回执里，
                // 直接搬）；只发 image 一块——base64 再进 text 会把载荷翻倍，
                // MCP 客户端按 image 渲染，路径信息请求参数里本就有。
                let blocks = match registry::image_part(&receipt) {
                    Some((mime, data)) => vec![ContentBlock::image(data, mime)],
                    None => vec![ContentBlock::text(receipt.to_string())],
                };
                Ok(CallToolResult::success(blocks).into())
            }
            // 业务错误 → isError 文本（模型读着改下一步，是正常工作方式）。
            Err(ToolError::RespondToModel(msg)) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(msg)]).into())
            }
            // 环境损坏（连接断/存储坏）→ JSON-RPC 协议错（internal_error）。
            Err(ToolError::Fatal(msg)) => Err(McpError::internal_error(msg, None)),
        }
    }
}

impl McpHandler {
    /// game_start 的认领段：无活局则取待局 → pair() → 装配 ctx/事件泵 → 入槽。
    ///
    /// 认领互斥（[`McpShared::claim`]）盖住整个 pair()（秒级 await）：并发认领
    /// 串行化，后到者进锁看到活局已在、直接落回工具面（幂等）。pair 失败把
    /// 待局还回槽（外部 Agent 可重试），半成品会话由 pair 自身清理。
    async fn ensure_game(&self) -> Result<(), String> {
        let _claim = self.shared.claim.lock().await;
        if self.shared.live_ctx().is_some() {
            return Ok(());
        }
        let Some(pending) = self.shared.take_pending() else {
            return Err(GAME_START_NO_GAME.to_string());
        };
        // 跑待局自带的配对执行体（from_pair=pair::pair 自建两席；壳层=结对它已登记
        // 的 A'）：失败路径把待局原样还回槽，外部 Agent 重试 game_start 才有得认领
        //（闭包可重入，见 PairFn 注）。
        let paired = match (pending.pairing)().await {
            Ok(p) => p,
            Err(e) => {
                self.shared.put_back_pending(pending);
                return Err(format!("pairing failed: {e}"));
            }
        };
        // 事件物化（player.rs 纪律）：队列绑进 HookHost（协商 Ack 落点），基线在
        // spawn 前于本任务同步取定——晚起的泵吞不掉基线之后的事件。
        let queue = Arc::new(EventQueue::new());
        paired.agent_hook.bind_events(&queue);
        let baseline = event_baseline(&paired.agent_watch);
        let pump = tokio::spawn(run_event_pump(
            EmitWatch::new(paired.agent_watch.clone_rx()),
            queue.clone(),
            baseline,
        ));
        let ctx = ToolCtx {
            player: Arc::new(paired.agent),
            watch: EmitWatch::new(paired.agent_watch.clone_rx()),
            events: queue,
            staging: pending.staging,
            memory: pending.memory,
            memory_ns: Driver::Mcp.memory_ns(),
            driver: Driver::Mcp,
            subagent_enabled: false,
            subagent: None,
            on_tool: None, // MCP 出口的回执面在 tools/call 响应上；环由宿主按需接
        };
        let mut g = self.shared.lifecycle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.live = Some(LiveGame { front: paired.front, ctx, pump });
        Ok(())
    }
}
