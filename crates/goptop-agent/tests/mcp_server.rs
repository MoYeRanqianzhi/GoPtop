//! MCP 出口的全量测试 —— 进程内起 server（随机口 + 已知 token），**裸 JSON-RPC
//! over HTTP 客户端**（计划「测试计划」第 4 条：不引 rmcp client，按 streamable
//! HTTP 规范手搓）驱动一整局：
//!
//! ```text
//! POST initialize → 记录 mcp-session-id 头 → notifications/initialized →
//! tools/list（断言工具集与 read_only 注解）→ 无待局业务错 → tools/call
//! game_start 认领 → wait_events / read /game/board / move（write+submit）/
//! chat 驱动对局（对手侧 = 进程内另一 PlayerHandle 脚本化）→ image 变体回
//! image content → 错误分支 isError:true → game_leave 拆局。
//! ```
//!
//! 规范细节：Accept 同时含 `application/json` 与 `text/event-stream`——服务端
//! 默认（json_response=false）回 SSE 帧，客户端两种响应形态都解；鉴权中间件
//! 的 401 分支（无 token / 错 token）与端口占用回退随机口各自成用例。
//!
//! SERIAL 锁纪律与 loop_headless.rs 同款：进程内 BC hub 全局无局号，配对类
//! 用例必须串行；用例尾部显式 server.stop()，panic 展开时 McpServer 的 Drop
//! abort serve 任务兜底（测试进程随即退出，OS 回收端口与内存）。

// 本文件只在 mcp feature 下有意义（mcp 模块整个被门住）；无 feature 时本测试
// 目标编译为空，`cargo test -p goptop-agent`（无 feature）保持全绿。
#![cfg(feature = "mcp")]
// SERIAL 锁必须横跨整个用例的 await（串行化的本意）——与 loop_headless.rs /
// headless.rs 的既有纪律同款，不是「锁忘放」。
#![allow(clippy::await_holding_lock)]

use std::sync::Arc;
use std::time::Duration;

use goptop_agent::mcp::{McpConfig, McpServer, PendingGame};
use goptop_agent::pair::{PairConfig, SeatColor};
use goptop_agent::player::{NativePlayer, PlayerHandle};
use goptop_agent::store::NativeStore;
use goptop_net::session::UiCommand;
use goptop_transport_native::{HeadlessHost, Host};
use serde_json::{Value, json};

/// 进程内 BC hub 全局无局号（bc.rs 头注自证缺口）——配对类用例串行。
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 测试用的固定 token（测试即「持有 token 的本地 UI」，真值不入库）。
const TOKEN: &str = "mcp-test-token-0123456789abcdef";

/* ---------------- 裸 JSON-RPC over HTTP 客户端 ---------------- */

/// streamable HTTP 规范的手搓客户端：Bearer + 双形态 Accept + mcp-session-id
/// 回传；响应按 content-type 解 JSON 或 SSE（取最后一个可解析的 data 帧）。
struct RpcClient {
    http: reqwest::Client,
    url: String,
    token: Option<String>,
    session: Option<String>,
    next_id: u64,
}

impl RpcClient {
    fn new(port: u16, token: Option<String>) -> Self {
        // rustls 0.23 的进程级 provider 要显式安装（与 native_http.rs 同款幂等
        // 安装，重复安装回 Err 忽略即成）；reqwest Client 构造期就要 provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            http: reqwest::Client::new(),
            url: format!("http://127.0.0.1:{port}/mcp"),
            token,
            session: None,
            next_id: 0,
        }
    }

    /// 单次 POST。回 (HTTP 状态, 解析出的 JSON-RPC 载荷；通知/空体为 None)。
    async fn post(&mut self, body: Value) -> (u16, Option<Value>) {
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(sid) = &self.session {
            req = req.header("mcp-session-id", sid);
        }
        // initialize 之后所有请求按 2025-06-18 规范带版本头。
        if self.session.is_some() {
            req = req.header("MCP-Protocol-Version", "2025-06-18");
        }
        let resp = req.send().await.expect("HTTP 请求失败");
        let status = resp.status().as_u16();
        if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(sid.to_string());
        }
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let text = resp.text().await.expect("读响应体");
        (status, parse_payload(&ctype, &text))
    }

    /// JSON-RPC 请求：断言 HTTP 200、id 回配、无协议错误，回 result。
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let (status, payload) =
            self.post(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await;
        assert_eq!(status, 200, "{method} 的 HTTP 状态");
        let v = payload.unwrap_or_else(|| panic!("{method} 无响应载荷"));
        assert_eq!(v["id"], json!(id), "{method} 响应 id 不匹配: {v}");
        assert!(v["error"].is_null(), "{method} 不应返回协议错误: {}", v["error"]);
        v["result"].clone()
    }

    /// tools/call 的简写：回 (isError, 首个 text content 的文本)。
    async fn call_tool(&mut self, tool: &str, args: Value) -> (bool, String) {
        let result = self.raw_call(tool, args).await;
        let is_err = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|b| b["text"].as_str())
            .unwrap_or_default()
            .to_string();
        (is_err, text)
    }

    /// tools/call 全量 result（image 断言要摸原始 content 数组）。
    async fn raw_call(&mut self, tool: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": args})).await
    }

    /// tools/call 成功分支的回执 JSON（text content 解析为结构化回执）。
    async fn call_tool_json(&mut self, tool: &str, args: Value) -> Value {
        let (is_err, text) = self.call_tool(tool, args).await;
        assert!(!is_err, "{tool} 不应 isError: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{tool} 回执不是 JSON: {e}: {text}"))
    }

    /// read 动态文件的全量取数：read 回执是 {ok,path,content,...} 壳，content
    /// 才是文件本体（JSON 文件再解一层）。
    async fn read_json(&mut self, path: &str) -> Value {
        let receipt = self.call_tool_json("read", json!({"path": path})).await;
        let content = receipt["content"].as_str().unwrap_or_default();
        serde_json::from_str(content).unwrap_or_else(|e| panic!("{path} 内容不是 JSON: {e}: {content}"))
    }

    /// 轮询 wait_events 直到出现满足谓词的事件。事件是**分批到达**的（队列里
    /// 已有事件就瞬间全量排空——对手的 move 与 chat 常落在两拍），单次调用
    /// 断言会撞上「只等到先到的那批」。每次排空间隔 50ms 重试，超时 panic。
    async fn wait_event(&mut self, what: &str, pred: impl Fn(&Value) -> bool, secs: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let events = self.call_tool_json("wait_events", json!({"timeout_secs": 1})).await;
            if events.as_array().is_some_and(|a| a.iter().any(&pred)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "超时等事件（{what}）；最后一批: {events:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// 响应载荷解析：`application/json` 直解；`text/event-stream` 按事件块拆
/// （空行分隔），拼 `data:` 行逐块试解，取最后一个可解析帧（rmcp 的请求
/// 响应流里请求结果就是收尾那帧）；空体（通知回执）回 None。
fn parse_payload(ctype: &str, body: &str) -> Option<Value> {
    if body.trim().is_empty() {
        return None;
    }
    if ctype.contains("text/event-stream") {
        let mut last = None;
        for block in body.split("\n\n") {
            let data: Vec<&str> = block
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            if data.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&data.join("\n")) {
                last = Some(v);
            }
        }
        last
    } else {
        serde_json::from_str(body).ok()
    }
}

/* ---------------- 夹具 ---------------- */

/// 每用例独享的记忆库文件（并行用例同进程，文件名撞车会互读对方的表）。
fn temp_db() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("goptop-agent-mcp-{}-{n}.db", std::process::id()))
}

/// 无 STUN 的宿主：本地候选即刻收集完，省掉 gathering 的 8s 超时兜底。
fn host() -> Arc<HeadlessHost> {
    let h = Arc::new(HeadlessHost::default());
    h.storage_set("goptop:stun", Some("[]"));
    h
}

/// MCP 席（白）的对手脚本台：A'（人席）句柄 + 常驻泵任务。
/// Black 方向 pair() 不给 A' start_pump（前端 poll 模式），测试里由本任务代泵。
struct FrontSeat {
    player: NativePlayer,
    _pump: tokio::task::JoinHandle<()>,
}

fn front_seat(server: &McpServer) -> FrontSeat {
    let front = server.front_handle().expect("game_start 后应能取到 A' 句柄");
    let session = front.session().clone();
    let pump = tokio::spawn(async move {
        loop {
            session.pump();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    FrontSeat { player: front, _pump: pump }
}

/// 存入待局装配（gomoku 15、用户执黑 → Agent 执白）。
fn set_game(server: &McpServer) {
    server.set_pending_game(PendingGame {
        pair: PairConfig {
            kind: "gomoku".into(),
            size: 15,
            my_color: SeatColor::Black,
            front_name: "甲".into(),
            agent_name: "Agent".into(),
            share_origin: "http://localhost".into(),
            front_host: host(),
            agent_host: host(),
        },
        memory: Arc::new(NativeStore::open(&temp_db()).expect("temp memory db")),
    });
}

/// 起服务器（随机口 + 固定 token）。
async fn start_server() -> McpServer {
    McpServer::start(McpConfig { port: 0, token: TOKEN.into() })
        .await
        .expect("MCP 服务器应能起")
}

/* ---------------- 用例 ---------------- */

/// 端口占用回退随机口 + 默认配置口径（计划：默认 9537，占用回退并把实际端口回传）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 端口占用回退随机口与默认配置() {
    let def = McpConfig::default();
    assert_eq!(def.port, 9537, "默认端口 9537");
    assert_eq!(def.token.len(), 32, "随机 token = 4×u32 的 32 个十六进制字符");
    assert!(def.token.chars().all(|c| c.is_ascii_hexdigit()));

    // 占住一个口，再让服务器按这个口起——必须回退随机口且实际可达。
    let occupy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("占口");
    let taken = occupy.local_addr().unwrap().port();
    let server = McpServer::start(McpConfig { port: taken, token: "t".into() })
        .await
        .expect("占用回退后应能起");
    assert_ne!(server.port(), taken, "被占端口必须回退随机口");
    assert_eq!(server.url(), format!("http://127.0.0.1:{}/mcp", server.port()));
    // 回退口真的在听：无 token 的请求应得到 401（而不是连接拒绝）。
    let mut client = RpcClient::new(server.port(), None);
    let (status, _) = client
        .post(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .await;
    assert_eq!(status, 401, "回退口必须可达且鉴权生效");
    server.stop();
    drop(occupy);
}

/// 全流程：鉴权 401 → initialize/会话头 → tools/list 工具集与注解 →
/// 无待局业务错 → game_start 认领 → 驱动一整局 → game_leave 拆局。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_全流程_初始化_工具面与整局驱动() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server().await;

    /* ---- 鉴权：无 token / 错 token → 401 ---- */
    let mut anonymous = RpcClient::new(server.port(), None);
    let (status, _) = anonymous
        .post(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .await;
    assert_eq!(status, 401, "无 token 必须拒");
    let mut impostor = RpcClient::new(server.port(), Some("wrong-token".into()));
    let (status, _) = impostor
        .post(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .await;
    assert_eq!(status, 401, "错 token 必须拒");

    /* ---- initialize：记录 mcp-session-id 头（后续请求回传） ---- */
    let mut rpc = RpcClient::new(server.port(), Some(TOKEN.into()));
    let init = rpc
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "goptop-mcp-test", "version": "0.0.0"},
            }),
        )
        .await;
    assert_eq!(init["protocolVersion"], json!("2025-06-18"), "服务端应回显协商版本");
    let session_id = rpc.session.clone().expect("initialize 响应必须带 mcp-session-id 头");
    assert!(!session_id.is_empty());
    // 通知（无 id）：服务器回 202/空体（个别实现回 200 + 空体，同样合格）。
    let (status, payload) = rpc.post(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    assert!((200..=202).contains(&status), "initialized 通知应回 2xx 空回执: {status}");
    assert!(payload.is_none(), "通知无 JSON-RPC 载荷");

    /* ---- tools/list：8 个 MCP 工具 + read_only 注解（delegate 不在册） ---- */
    let tools = rpc.request("tools/list", json!({})).await;
    let list = tools["tools"].as_array().expect("tools 数组");
    let names: Vec<&str> = list.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        vec!["read", "write", "submit", "edit", "grep", "wait_events", "game_start", "game_leave"],
        "MCP 工具清单 = 文件族 5 + 模式专属 3（tools_for(Driver::Mcp) 的呈现序）"
    );
    for t in list {
        let name = t["name"].as_str().unwrap_or_default();
        let read_only = t["annotations"]["readOnlyHint"].as_bool().unwrap_or(false);
        let expect = matches!(name, "read" | "grep" | "wait_events");
        assert_eq!(read_only, expect, "{name} 的 readOnlyHint 注解");
        // 手写 schema 原样进 rmcp：顶层 object。
        assert_eq!(t["inputSchema"]["type"], json!("object"), "{name} schema 顶层");
    }

    /* ---- 无待局：game_start 回业务错误（isError 文本、非协议错） ---- */
    let (is_err, text) = rpc.call_tool("game_start", json!({})).await;
    assert!(is_err, "无人开局时 game_start 应 isError");
    assert!(
        text.contains("no game to claim yet"),
        "业务错误文案应来自 GAME_START_NO_GAME：{text}"
    );

    /* ---- game_start 认领：待局装配 → pair() → started/my_color ---- */
    set_game(&server);
    let start = rpc.call_tool_json("game_start", json!({})).await;
    assert_eq!(start["ok"], json!(true));
    assert_eq!(start["started"], json!(true), "认领后配对应已完成（headless 秒级）");
    assert_eq!(start["my_color"], json!("white"), "用户执黑 → Agent 执白");
    let front = front_seat(&server);

    /* ---- 对手落子 → wait_events 送达 move 事件 ---- */
    front.player.cmd(UiCommand::Place { x: 7, y: 7 });
    rpc.wait_event("黑方落子", |e| {
        e["t"] == json!("move") && e["x"] == json!(7) && e["y"] == json!(7) && e["by"] == json!("black")
    }, 30)
    .await;
    // 排空后再查：空队列 + timeout 0 → []（正常返回非错误）。
    let empty = rpc.call_tool_json("wait_events", json!({"timeout_secs": 0})).await;
    assert_eq!(empty, json!([]), "排空后 timeout 0 应回空数组");

    /* ---- 落子两段式：write 暂存（盘面未动）→ submit 落子 ---- */
    let staged = rpc.call_tool_json("write", json!({"path": "/game/in/move", "content": "8,8"})).await;
    assert_eq!(staged["ok"], json!(true), "write 只暂存: {staged}");
    assert_eq!(staged["staged"]["path"], json!("/game/in/move"));
    let board_before = rpc.read_json("/game/board").await;
    assert!(
        board_before["stones"]["white"].as_array().is_some_and(|w| w.is_empty()),
        "暂存不该落子: {board_before}"
    );
    let placed = rpc.call_tool_json("submit", json!({"path": "/game/in/move"})).await;
    assert_eq!(placed["ok"], json!(true));
    assert_eq!(placed["action"], json!("move"));
    assert_eq!(placed["move_count"], json!(2), "黑 7,7 + 白 8,8");
    assert!(
        front.player.wait_until(
            &mut |s| s["moveCount"] == json!(2),
            Duration::from_secs(15),
        ),
        "白方 (8,8) 应同步到人席"
    );

    /* ---- read /game/board：稀疏 JSON 与快照一致 ---- */
    let board = rpc.read_json("/game/board").await;
    assert_eq!(board["stones"]["black"], json!([[7, 7]]));
    assert_eq!(board["stones"]["white"], json!([[8, 8]]));
    assert_eq!(board["to_move"], json!("black"));
    assert_eq!(board["last_move"]["by"], json!("white"));

    /* ---- 聊天双向：人发 → 事件送达；Agent 发 → 人收到 ---- */
    front.player.cmd(UiCommand::SendChat("你好，请多指教".into()));
    rpc.wait_event(
        "人方聊天",
        |e| e["t"] == json!("chat") && e["text"] == json!("你好，请多指教"),
        30,
    )
    .await;
    let _ = rpc.call_tool_json("write", json!({"path": "/game/in/chat", "content": "你好！请多指教。"})).await;
    let _ = rpc.call_tool_json("submit", json!({"path": "/game/in/chat"})).await;
    assert!(
        front.player.wait_until(
            &mut |s| s["chatLog"].as_array().is_some_and(|l| l.iter().any(|m| m["text"] == json!("你好！请多指教。"))),
            Duration::from_secs(15),
        ),
        "Agent 的聊天应送达人席"
    );

    /* ---- image 变体：MCP type:"image" content（base64 PNG） ---- */
    let image = rpc.raw_call("read", json!({"path": "/game/board/image.png"})).await;
    assert_eq!(image["isError"], json!(false), "image 变体不应报错: {image}");
    let first = &image["content"][0];
    assert_eq!(first["type"], json!("image"), "image 变体必须走 image content: {first}");
    assert_eq!(first["mimeType"], json!("image/png"));
    // iVBORw0KGgo = \x89PNG\r\n\x1a\n 的标准 base64 前缀（PNG 签名，免解全量）。
    let data = first["data"].as_str().expect("image content 应带 base64 data");
    assert!(data.starts_with("iVBORw0KGgo"), "data 应是 PNG 的 base64: {data}");

    /* ---- 错误分支：isError:true（业务错误文本回模型） ---- */
    let (is_err, text) = rpc.call_tool("write", json!({"path": "/game/in/move", "content": "abc"})).await;
    assert!(is_err, "格式错误应在 write 即拒");
    assert!(text.contains("invalid move"), "格式错误文案: {text}");
    let (is_err, _) = rpc.call_tool("nope", json!({})).await;
    assert!(is_err, "未在册工具回业务错（列可用工具），不是协议错");
    // 白方已落、黑方行棋——Agent（白）再 submit 落子撞「未轮到/占点」规则错误
    // （write 合法暂存、submit 规则拒）。
    let _ = rpc.call_tool_json("write", json!({"path": "/game/in/move", "content": "8,8"})).await;
    let (is_err, text) = rpc.call_tool("submit", json!({"path": "/game/in/move"})).await;
    assert!(is_err, "未轮到/占点的规则错误应在 submit 拒: {text}");

    /* ---- game_leave：局中认输收尾 + 拆局 ---- */
    let leave = rpc.call_tool_json("game_leave", json!({})).await;
    assert_eq!(leave["ok"], json!(true));
    assert_eq!(leave["resigned"], json!(true), "局中离场先认输");
    assert_eq!(leave["winner"], json!("black"), "Agent（白）认输 → 黑胜");
    assert!(
        front.player.wait_until(
            &mut |s| s["winner"] == json!("black"),
            Duration::from_secs(15),
        ),
        "人席应看到同一胜者"
    );
    assert!(
        server.front_handle().is_none(),
        "game_leave 后活局应拆干净（front_handle 回 None）"
    );
    // 拆局后工具再无着力点：业务错误（活局已不在），不是协议错。
    let (is_err, text) = rpc.call_tool("read", json!({"path": "/game/board"})).await;
    assert!(is_err, "拆局后 read 应回业务错: {text}");
    assert!(text.contains("no live game"), "拆局后文案: {text}");

    server.stop();
}
