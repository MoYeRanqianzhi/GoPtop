//! GoPtop 信令服务器（官服）。
//!
//! 定位（RustDesk 模式）：只做「名册 + 通用信令转发 + 数据兜底中转」，
//! 对局数据面尽量走双方 WebRTC 直连；直连失败时客户端自动降级用 `relay`。
//! **服务器不落地任何数据**：不存 pwd/offer/身份映射——所有信令原样转发，
//! pwd 校验、邀请/观战同意逻辑全在客户端；全部状态在内存，连接断即清。
//!
//! 消息面（JSON，`t` 为判别标签）：
//! - `hello`/`announce`：注册与状态上报，服务器向全员广播名册（在线用户）。
//! - `signal`：任意点对点信令原样转发（邀请 offer/answer、观战 join/offer/answer、
//!   大厅挑战、观战房间控制等，语义由客户端解释）。
//! - `relay`：GameMsg 兜底中转（P2P 未建立时客户端用它，按 sender+seq 去重）。
//! - `ping`：心跳。
//!
//! 部署：监听 `--listen`（默认 127.0.0.1:9527），前面由 openresty/caddy 做
//! TLS 反代（wss://goptopserver.meowoo.org/ws → ws://127.0.0.1:9527/ws）。
//! 官服跨服务器不可对战：客户端一次只连一个服务器，名册/信令天然隔离。

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use futures_util::{SinkExt, StreamExt};

/// 名册里单个在线用户。
#[derive(Clone)]
struct Peer {
    name: String,
    status: String,
    game_id: Option<String>,
    tx: mpsc::UnboundedSender<Message>,
}

/// 服务器全局状态。std RwLock 即可：临界区内只做 Map 读写与 channel send，
/// 绝不跨 await 持锁。**不含任何业务数据**（pwd/offer 等只过手，不落地）。
#[derive(Default)]
struct StateInner {
    peers: HashMap<String, Peer>,
    /// 每 ID 限速计数：user_id -> (窗口起点, 已收条数)（同 ID 顶替时沿用旧桶，重连不清零）。
    rates: HashMap<String, (Instant, u32)>,
}
type AppState = Arc<RwLock<StateInner>>;

/// 客户端上行消息（`t` 为判别标签，字段名 camelCase）。
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case", rename_all_fields = "camelCase")]
enum C2S {
    /// user_id：客户端持久 ID（URL 链接的目标就是它）——名册必须用同一套 ID，
    /// 否则链接指向的名字在信令路由时找不到人。同 ID 重连视为顶替（旧连接踢除）。
    Hello { name: String, user_id: Option<String> },
    Announce { status: String, game_id: Option<String> },
    /// 通用点对点信令：`kind` 标注语义（offer/answer/ice/spec/challenge/…），
    /// `payload` 原样转发，服务器不解析、不存储。
    Signal { to: String, kind: String, payload: Value },
    /// 对局数据兜底中转。
    Relay { to: String, payload: Value },
    Ping,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let listen: SocketAddr = std::env::args()
        .find(|a| a.starts_with("--listen="))
        .and_then(|a| a["--listen=".len()..].parse().ok())
        .unwrap_or_else(|| "127.0.0.1:9527".parse().expect("valid listen addr"));
    let state: AppState = Arc::default();
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen).await.expect("bind listen port");
    tracing::info!("goptop-server listening on {listen}");
    axum::serve(listener, app).await.expect("serve");
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_conn(socket, state))
}

/// 单连接生命周期：首条消息必须是 hello（优先采用客户端持久 ID；缺失才分配
/// s- 短 ID 兜底），之后循环处理上行。
async fn handle_conn(socket: WebSocket, state: AppState) {
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // 未完成 hello 前收到任何其他消息/超时都直接断开，防止匿名连接占位。
    let hello = tokio::time::timeout(Duration::from_secs(10), source.next()).await;
    let Ok(Some(Ok(Message::Text(text)))) = hello else { return };
    let Ok(C2S::Hello { name, user_id }) = serde_json::from_str::<C2S>(&text) else { return };

    // 注册 ID：优先用客户端持久 ID（与邀请链接一致）；同 ID 旧连接顶替。
    // code=taken-over：客户端收到后必须主动断开旧 socket，否则旧连接的下行
    // 事件仍会以该 user_id 路由（顶替后名册里已是新连接）。
    let user_id = match user_id {
        Some(id) if is_valid_peer_id(&id) => {
            let mut st = state.write().unwrap();
            if let Some(old) = st.peers.remove(&id) {
                let _ = old.tx.send(Message::text(json!({ "t": "error", "code": "taken-over", "msg": "本 ID 在别处重新连接，当前连接已被顶替" }).to_string()));
            }
            drop(st);
            id
        }
        _ => alloc_id(&state),
    };
    {
        let mut st = state.write().unwrap();
        st.peers.insert(
            user_id.clone(),
            Peer { name: sanitize_name(&name, &user_id), status: "idle".into(), game_id: None, tx: tx.clone() },
        );
        broadcast_peers_locked(&st);
    }
    // welcome 告知客户端其在名册中的 ID（客户端持久 ID，或缺失时的 s- 兜底短 ID）。
    let _ = tx.send(Message::text(json!({ "t": "welcome", "id": user_id }).to_string()));
    tracing::info!("{user_id} joined (name={name})");

    // 下行泵：业务循环只往 tx 里塞，由本任务统一写 socket。
    let pump = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 收发循环：60s 无任何消息（含 25s 心跳）才视为断线——超时必须远大于
    // 客户端心跳间隔，否则空闲连接会被误杀。
    loop {
        let next = tokio::time::timeout(Duration::from_secs(60), source.next()).await;
        let msg = match next {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };
        let Message::Text(text) = msg else { continue };
        let event = match serde_json::from_str::<C2S>(&text) {
            Ok(ev) => ev,
            Err(_) => {
                let _ = tx.send(Message::text(json!({ "t": "error", "msg": "bad message" }).to_string()));
                break;
            }
        };
        if !rate_ok(&state, &user_id) {
            tracing::warn!("{user_id} exceeded rate limit, dropping connection");
            break;
        }
        // Ping 无需触碰状态，直接回 pong。
        if matches!(event, C2S::Ping) {
            let _ = tx.send(Message::text(json!({ "t": "pong" }).to_string()));
            continue;
        }
        if let Err(msg) = handle_event(&state, &user_id, event) {
            let _ = tx.send(Message::text(json!({ "t": "error", "msg": msg }).to_string()));
        }
    }

    // 断线清理：名册移除并广播（无其他状态可清——服务器不存业务数据）。
    // 同 ID 顶替后这里可能晚于新连接执行：只有名册里仍是本连接的 tx 才清，
    // 否则会把顶替者（新连接）的注册误删成「在线却不可达」。
    {
        let mut st = state.write().unwrap();
        if st.peers.get(&user_id).is_some_and(|p| p.tx.same_channel(&tx)) {
            st.peers.remove(&user_id);
            st.rates.remove(&user_id);
            broadcast_peers_locked(&st);
        }
    }
    tracing::info!("{user_id} left");
    drop(tx);
    let _ = pump.await;
}

/// 处理一条已解析的上行事件。返回 Err 时向该客户端回 error 消息。
fn handle_event(state: &AppState, me: &str, ev: C2S) -> Result<(), String> {
    let st = state.write().unwrap();
    match ev {
        C2S::Announce { status, game_id } => {
            // 白名单必须与客户端 announce 的取值逐字一致（goptop-net session/server.rs 的 announce）：
            // 多一个少一个都会在此被拒，客户端只看到一条 3s 提示，名册状态停在旧值。
            if !matches!(status.as_str(), "idle" | "waiting" | "in-game") {
                return Err("bad status".into());
            }
            let mut st = st;
            let Some(peer) = st.peers.get_mut(me) else { return Err("not registered".into()) };
            peer.status = status;
            peer.game_id = game_id;
            broadcast_peers_locked(&st);
        }
        C2S::Signal { to, kind, payload } => {
            if !payload_size_ok(&payload, SIGNAL_MAX_BYTES) {
                return Err("signal payload too large".into());
            }
            // 客户端按 t:"signal" 分发；kind/payload 独立成字段（语义由客户端解释）
            send_to(&st, &to, &json!({ "t": "signal", "kind": kind, "from": me, "payload": payload }));
        }
        C2S::Relay { to, payload } => {
            if !payload_size_ok(&payload, RELAY_MAX_BYTES) {
                return Err("relay payload too large".into());
            }
            send_to(&st, &to, &json!({ "t": "relayed", "from": me, "payload": payload }));
        }
        // hello 在握手层已消费；循环内再收到说明客户端状态异常（或探测），温和拒绝
        // （只回 error，不断开；真正的断开交给限速/解析失败路径）——绝不 panic（release panic=abort 会拖垮整个进程）。
        C2S::Hello { .. } => return Err("already registered".into()),
        C2S::Ping => unreachable!("handled by caller before dispatch"),
    }
    Ok(())
}

/// signal 单条上限：须容得下 128px 圆形头像 dataURL（约 20KB）加信令余量。
const SIGNAL_MAX_BYTES: usize = 64 * 1024;
/// relay 单条上限：19 路全量快照（board + 数百手 history）实测 < 16KB。
const RELAY_MAX_BYTES: usize = 16 * 1024;

/// payload 字节上限：序列化后长度（顶层键数限制挡不住单键巨串）。
fn payload_size_ok(payload: &Value, max: usize) -> bool {
    payload.to_string().len() <= max
}

fn send_to(st: &StateInner, to: &str, msg: &Value) {
    if let Some(peer) = st.peers.get(to) {
        let _ = peer.tx.send(Message::text(msg.to_string()));
    }
}

/// 名册全量广播：人数规模（几十人）下全量下发足够，避免增量同步复杂度。
fn broadcast_peers_locked(st: &StateInner) {
    let users: Vec<Value> = st
        .peers
        .iter()
        .map(|(id, p)| json!({ "id": id, "name": p.name, "status": p.status, "gameId": p.game_id }))
        .collect();
    let msg = Message::text(json!({ "t": "peers", "users": users }).to_string());
    for peer in st.peers.values() {
        let _ = peer.tx.send(msg.clone());
    }
}

/// 合法 peer ID：u- 前缀（客户端持久 ID）或 s- 前缀（服务器分配），总长 3..=40。
fn is_valid_peer_id(id: &str) -> bool {
    (id.starts_with("u-") || id.starts_with("s-")) && (3..=40).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' )
}

/// 分配 `s-XXXX` 短 ID（base36 4 位，冲突重试）。
fn alloc_id(state: &AppState) -> String {
    let st = state.write().unwrap();
    loop {
        let id = format!("s-{}", random_base36(4));
        if !st.peers.contains_key(&id) {
            return id;
        }
    }
}

fn random_base36(len: usize) -> String {
    const CH: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..len).map(|_| CH[rand::rng().random_range(0..CH.len())] as char).collect()
}

/// 昵称清理：截断到 24 字符且不得为空（空则用短 ID 兜底显示）。
fn sanitize_name(raw: &str, fallback_id: &str) -> String {
    let trimmed = raw.trim().chars().take(24).collect::<String>();
    if trimmed.is_empty() { fallback_id.to_string() } else { trimmed }
}

/// 每连接限速：10 秒窗口最多 500 条（打洞期 ice 风暴也在阈值内）。
fn rate_ok(state: &AppState, me: &str) -> bool {
    let mut st = state.write().unwrap();
    let now = Instant::now();
    let entry = st.rates.entry(me.to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) > Duration::from_secs(10) {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= 500
}

/// 信令服务器纯函数单测（审查 #6 C11）：ID 白名单、昵称清理、限速窗口。
/// 网络路径（ws 握手/广播）不在单测覆盖面，由 scripts/e2e 官服冒烟脚本回归。
#[cfg(test)]
mod tests {
    use super::*;

    /// is_valid_peer_id：u-/s- 前缀、总长 3..=40、仅 ASCII 字母数字与 '-'。
    #[test]
    fn peer_id_validation() {
        assert!(is_valid_peer_id("u-abc"));
        assert!(is_valid_peer_id("s-1a2b"));
        // 前缀白名单：其余一律拒绝（大小写敏感）。
        assert!(!is_valid_peer_id("x-abc"));
        assert!(!is_valid_peer_id("U-abc"));
        assert!(!is_valid_peer_id("abc"));
        // 长度边界 3..=40。
        assert!(is_valid_peer_id("u-a"));
        assert!(!is_valid_peer_id("u-"));
        assert!(is_valid_peer_id(&format!("u-{}", "a".repeat(38)))); // 恰 40
        assert!(!is_valid_peer_id(&format!("u-{}", "a".repeat(39)))); // 41
        // 字符集：下划线/空格/非 ASCII 拒绝。
        assert!(!is_valid_peer_id("u-a_b"));
        assert!(!is_valid_peer_id("u-a b"));
        assert!(!is_valid_peer_id("u-围"));
    }

    /// sanitize_name：按字符截断到 24、空/纯空白名回退短 ID、其余 trim 保留。
    #[test]
    fn name_sanitization() {
        assert_eq!(sanitize_name(&"a".repeat(30), "u-x"), "a".repeat(24));
        // 多字节字符按"字符"而非字节截断：30 个汉字 → 24 个。
        assert_eq!(sanitize_name(&"围".repeat(30), "u-x"), "围".repeat(24));
        assert_eq!(sanitize_name("", "u-x"), "u-x");
        assert_eq!(sanitize_name("   ", "u-x"), "u-x");
        assert_eq!(sanitize_name("  alice  ", "u-x"), "alice");
        assert_eq!(sanitize_name("bob", "u-x"), "bob");
    }

    /// rate_ok：500 条/10s 窗口。Instant 不可注入，窗口重置直接把计数窗口
    /// 起点拨回 11s 前（等价真实流逝），不写真实 sleep。
    #[test]
    fn rate_limit_threshold_and_window_reset() {
        let state: AppState = Arc::default();
        // 阈值内全部放行。
        for _ in 0..500 {
            assert!(rate_ok(&state, "u-r"));
        }
        // 第 501 条被拒。
        assert!(!rate_ok(&state, "u-r"));
        // 连接间计数独立：另一连接不受影响。
        assert!(rate_ok(&state, "u-other"));
        // 窗口重置：起点拨回 11s 前 → 计数清零、重新放行。
        state.write().unwrap().rates.get_mut("u-r").unwrap().0 = Instant::now() - Duration::from_secs(11);
        assert!(rate_ok(&state, "u-r"));
    }

    /// payload_size_ok：按序列化字节限长——少量键的巨串也要拦，大量小键也拦。
    #[test]
    fn payload_size_limit_by_bytes() {
        // 少量键 + 巨串：3 个键各 20KB → 超 16KB relay 上限。
        let big = json!({ "a": "x".repeat(20 * 1024), "b": "y".repeat(20 * 1024), "c": "z".repeat(20 * 1024) });
        assert!(!payload_size_ok(&big, RELAY_MAX_BYTES));
        // 大量小键：200 个键各 1 字节值 → 序列化约 1.6KB，signal（64KB）放行、1KB 阈值拒。
        let mut m = serde_json::Map::new();
        for i in 0..200 {
            m.insert(format!("k{i}"), json!("v"));
        }
        let many = Value::Object(m);
        assert!(payload_size_ok(&many, SIGNAL_MAX_BYTES));
        assert!(!payload_size_ok(&many, 1024));
        // 正常量级信令/头像（20KB dataURL）在 signal 上限内。
        let avatar = json!({ "dataUrl": format!("data:image/png;base64,{}", "A".repeat(20 * 1024)) });
        assert!(payload_size_ok(&avatar, SIGNAL_MAX_BYTES));
        assert!(payload_size_ok(&json!({ "kind": "ice" }), RELAY_MAX_BYTES));
    }
}
