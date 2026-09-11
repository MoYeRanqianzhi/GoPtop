//! GoPtop 信令服务器（官服）。
//!
//! 定位（RustDesk 模式）：只做「信令 + 名册 + 短码 + 兜底中转」，对局数据面
//! 尽量走双方 WebRTC 直连；直连失败时客户端自动降级用 `relay`（WS 转发）。
//! 服务器不落盘、无持久化：全部状态在内存，连接断即清。
//!
//! 能力：
//! - `hello`/`announce`：注册与状态上报，服务器向全员广播名册（在线用户）。
//! - `invite-create`/`invite-resolve`：短码邀请（链接从 ~700 字符缩到 ~40）。
//!   邀请者把 kind/size/pwd/offer 存服务器换短码；受邀者凭短码+pwd 直接拿到
//!   offer 并回 answer（经服务器转发）——免回执。
//! - `challenge`/`accept-challenge`/`reject-challenge`：大厅手动挑战（pwd 空，
//!   被挑战者点接受后由发起者走 invite-create 生成短码，双方免回执完成直连）。
//! - `watch-create`/`watch-resolve`：观战短码，观战者点开即看，免回执。
//! - `offer`/`answer`/`ice`：SDP 与 trickle ICE 候选转发（服务器模式启用的能力）。
//! - `relay`：GameMsg 兜底中转（P2P 未建立时客户端用它，sender+seq 去重防重放）。
//!
//! 部署：监听 `--listen`（默认 127.0.0.1:9527），前面由 openresty/caddy 做
//! TLS 反代（wss://goptopserver.meowoo.org/ws → ws://127.0.0.1:9527/ws）。
//! 官服跨服务器不可对战：客户端一次只连一个服务器，名册/邀请天然隔离。

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};

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

/// 名册里单个在线用户。
#[derive(Clone)]
struct Peer {
    name: String,
    status: String,
    game_id: Option<String>,
    tx: mpsc::UnboundedSender<Message>,
}

/// 短码邀请：邀请者开局时创建，受邀者凭 code+pwd 兑换 offer（一次性，用掉即删）。
struct Invite {
    inviter: String,
    kind: String,
    size: u8,
    pwd: String,
    offer: String,
    game_id: String,
}

/// 观战短码：房主创建，观战者凭 code 直接连房主（免回执）。
struct Watch {
    owner: String,
    game_id: String,
}

/// 服务器全局状态。std RwLock 即可：临界区内只做 Map 读写与 channel send，
/// 绝不跨 await 持锁。
#[derive(Default)]
struct StateInner {
    peers: HashMap<String, Peer>,
    invites: HashMap<String, Invite>,
    watches: HashMap<String, Watch>,
    /// 每连接限速计数：conn_id -> (窗口起点, 已收条数)。
    rates: HashMap<String, (Instant, u32)>,
}
type AppState = Arc<RwLock<StateInner>>;

/// 客户端上行消息（`t` 为判别标签，字段名 camelCase）。
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case", rename_all_fields = "camelCase")]
enum C2S {
    Hello { name: String },
    Announce { status: String, game_id: Option<String> },
    InviteCreate { kind: String, size: u8, pwd: String, offer: String, game_id: String },
    InviteResolve { code: String, pwd: String },
    Answer { to: String, game_id: String, answer: String },
    Offer { to: String, game_id: String, role: String, offer: String },
    Ice { to: String, game_id: String, candidate: String },
    WatchCreate { game_id: String },
    WatchResolve { code: String },
    Challenge { to: String, kind: String, size: u8, code: String, pwd: String },
    ChallengeReject { to: String },
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

/// 单连接生命周期：首条消息必须是 hello（换取 s- 短 ID），之后循环处理上行。
async fn handle_conn(socket: WebSocket, state: AppState) {
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // 未完成 hello 前收到任何其他消息/超时都直接断开，防止匿名连接占位。
    let hello = tokio::time::timeout(Duration::from_secs(10), source.next()).await;
    let Ok(Some(Ok(Message::Text(text)))) = hello else { return };
    let Ok(C2S::Hello { name }) = serde_json::from_str::<C2S>(&text) else { return };

    let user_id = alloc_id(&state);
    {
        let mut st = state.write().unwrap();
        st.peers.insert(
            user_id.clone(),
            Peer { name: sanitize_name(&name, &user_id), status: "idle".into(), game_id: None, tx: tx.clone() },
        );
        broadcast_peers_locked(&st);
    }
    // welcome 告知客户端自己被分配的短 ID（大厅显示、链接目标都用它）。
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
    // 客户端心跳间隔，否则空闲连接会被误杀、其短码邀请随之失效。
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
        if let Err(msg) = handle_event(&state, &user_id, &tx, event) {
            let _ = tx.send(Message::text(json!({ "t": "error", "msg": msg }).to_string()));
        }
    }

    // 断线清理：名册、短码邀请、观战短码一并移除并广播。
    {
        let mut st = state.write().unwrap();
        st.peers.remove(&user_id);
        st.invites.retain(|_, i| i.inviter != user_id);
        st.watches.retain(|_, w| w.owner != user_id);
        st.rates.remove(&user_id);
        broadcast_peers_locked(&st);
    }
    tracing::info!("{user_id} left");
    drop(tx);
    let _ = pump.await;
}

/// 处理一条已解析的上行事件。返回 Err 时向该客户端回 error 消息。
fn handle_event(state: &AppState, me: &str, tx: &mpsc::UnboundedSender<Message>, ev: C2S) -> Result<(), String> {
    let mut st = state.write().unwrap();
    match ev {
        C2S::Announce { status, game_id } => {
            let Some(peer) = st.peers.get_mut(me) else { return Err("not registered".into()) };
            if !matches!(status.as_str(), "idle" | "waiting" | "in-game") {
                return Err("bad status".into());
            }
            peer.status = status;
            peer.game_id = game_id;
            broadcast_peers_locked(&st);
        }
        C2S::InviteCreate { kind, size, pwd, offer, game_id } => {
            if kind != "gomoku" && kind != "go" {
                return Err("bad kind".into());
            }
            if ![9, 13, 15, 19].contains(&size) {
                return Err("bad size".into());
            }
            let code = alloc_code(&st);
            st.invites.insert(code.clone(), Invite { inviter: me.to_string(), kind, size, pwd, offer, game_id });
            let _ = tx.send(Message::text(json!({ "t": "invite-created", "code": code }).to_string()));
        }
        C2S::InviteResolve { code, pwd } => {
            let Some(inv) = st.invites.get(&code) else { return Err(format!("邀请 {code} 不存在或已过期")) };
            if inv.pwd != pwd {
                return Err("邀请钥匙不正确".into());
            }
            // 一次性消费：resolve 成功即删（重放无效）。
            let Invite { inviter, kind, size, offer, game_id, .. } = st.invites.remove(&code).unwrap();
            let from_name = st.peers.get(me).map(|p| p.name.clone()).unwrap_or_default();
            send_to(&st, me, &json!({ "t": "invite-offer", "code": code, "kind": kind, "size": size, "offer": offer, "gameId": game_id, "from": inviter, "fromName": from_name }));
            send_to(&st, &inviter, &json!({ "t": "invitee-joined", "code": code, "from": me }));
        }
        C2S::Answer { to, game_id, answer } => forward(&st, me, &to, "answer", &json!({ "gameId": game_id, "answer": answer })),
        C2S::Offer { to, game_id, role, offer } => forward(&st, me, &to, "offer", &json!({ "gameId": game_id, "role": role, "offer": offer })),
        C2S::Ice { to, game_id, candidate } => forward(&st, me, &to, "ice", &json!({ "gameId": game_id, "candidate": candidate })),
        C2S::WatchCreate { game_id } => {
            let code = alloc_code(&st);
            st.watches.insert(code.clone(), Watch { owner: me.to_string(), game_id });
            let _ = tx.send(Message::text(json!({ "t": "watch-created", "code": code }).to_string()));
        }
        C2S::WatchResolve { code } => {
            let Some(w) = st.watches.get(&code) else { return Err(format!("观战 {code} 不存在或已失效")) };
            let owner = w.owner.clone();
            let game_id = w.game_id.clone();
            send_to(&st, me, &json!({ "t": "watch-accepted", "code": code, "from": owner, "gameId": game_id }));
            send_to(&st, &owner, &json!({ "t": "spectator-joined", "code": code, "from": me }));
        }
        C2S::Challenge { to, kind, size, code, pwd } => {
            if !st.peers.contains_key(&to) {
                return Err("对方不在线（或已在其他服务器）".into());
            }
            send_to(&st, &to, &json!({ "t": "challenge", "from": me, "kind": kind, "size": size, "code": code, "pwd": pwd }));
        }
        C2S::ChallengeReject { to } => {
            send_to(&st, &to, &json!({ "t": "challenge-rejected", "from": me }));
        }
        C2S::Relay { to, payload } => {
            if payload.as_object().map(|o| o.len() > 64).unwrap_or(true) {
                return Err("relay payload too large".into());
            }
            send_to(&st, &to, &json!({ "t": "relayed", "from": me, "payload": payload }));
        }
        C2S::Hello { .. } | C2S::Ping => unreachable!("handled by caller"),
    }
    Ok(())
}

/// 把 `extra` 包上 from 后转发给目标（offer/answer/ice 共用的转发骨架）。
fn forward(st: &StateInner, from: &str, to: &str, kind: &str, extra: &Value) {
    let mut obj = extra.as_object().cloned().unwrap_or_default();
    obj.insert("t".into(), json!(kind));
    obj.insert("from".into(), json!(from));
    send_to(st, to, &Value::Object(obj));
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

/// 分配 7 位 base36 短码（约 780 亿组合，冲突即重试）。邀请/观战共用一个码空间。
fn alloc_code(st: &StateInner) -> String {
    loop {
        let code = random_base36(7);
        if !st.invites.contains_key(&code) && !st.watches.contains_key(&code) {
            return code;
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
