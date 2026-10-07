//! WebSocket 服务器通道（原生）—— `tokio-tungstenite`。
//!
//! 与 wasm 侧 `goptop-transport/src/io/ws.rs` **行为逐条对齐**：
//! 连上即发 hello、25s 心跳、下行解析成同一组 `ServerEvt`、断开后固定 8s 重连
//! （不是指数退避——wasm 侧注释已说明 retry 恒为 3，换算下来就是固定 8s）。
//!
//! 与 wasm 的结构差异只在并发：那边是 DOM 回调 + 闭包持有；这边是一个 tokio 任务
//! 跑「连接 → 读循环 → 断线重连」，写走 mpsc 通道交给同一个任务。

use futures_util::{SinkExt, StreamExt};
use goptop_net::session::{Event, ServerEvt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{SharedCore, bridge};

/// 重连间隔（毫秒）。与 wasm 侧 `1000 * 2^3 = 8s` 一致。
const RECONNECT_MS: u64 = 8_000;
/// 心跳间隔（毫秒）。
const HEARTBEAT_MS: u64 = 25_000;

/// 上行句柄：把要发的文本丢给 IO 任务。
pub struct ServerSocket {
    tx: mpsc::UnboundedSender<String>,
}

impl ServerSocket {
    pub fn send(&self, v: &serde_json::Value) {
        let _ = self.tx.send(v.to_string());
    }

    /// 关闭：本方法无实际动作——发送端只能靠**整体析构**丢弃（`&self` 里 drop 不掉 tx）。
    /// 调用方（`bridge` 的 `ServerClose`）用 `Core::ws.take()` 把整个句柄移出 Core，
    /// 出块即析构：tx 一死，IO 任务的 `rx.is_closed()` 为真、`rx.recv()` 返回 None，
    /// 任务随即退出且不再重连（该检查在 `run` 的循环开头，见那里的说明）。
    pub fn close(&self) {
        // 真正的释放是 take() 之后的析构，见上。
    }
}

/// 连接服务器：建 IO 任务并把句柄挂进 Core。
pub fn connect(core: &SharedCore, url: Option<String>) {
    let Some(url) = url else { return };
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    if let Ok(mut c) = core.lock() {
        c.ws = Some(ServerSocket { tx });
    }
    let core = core.clone();
    tokio::spawn(async move { run(core, url, rx).await });
}

/// 服务器地址：按设置选（键名与 wasm 侧 `io::selected_server_url` 一致）。
///
/// 与 wasm 版的差异只在读存储的入口——那边走 `crate::storage_get`（宿主钩子 →
/// localStorage 兜底），这边走 `Host::storage_get`（桌面 `~/.goptop` 等）。
pub fn selected_server_url(host: &dyn crate::host::Host) -> Option<String> {
    const BUILTIN: &str = r#"[{"id":"official","label":"官方服务器","url":"wss://goptopserver.meowoo.org/ws","builtin":true}]"#;
    let sel = host.storage_get("goptop:server-sel").unwrap_or_else(|| "official".into());
    if sel == "none" {
        return None;
    }
    let mut all = BUILTIN.to_string();
    if let Some(custom) = host.storage_get("goptop:servers") {
        if custom.len() > 2 {
            all = format!(
                "[{},{}]",
                BUILTIN.trim_start_matches('[').trim_end_matches(']'),
                custom.trim_start_matches('[').trim_end_matches(']')
            );
        }
    }
    let list: serde_json::Value = serde_json::from_str(&all).unwrap_or(serde_json::Value::Null);
    list.as_array()
        .and_then(|arr| arr.iter().find(|s| s["id"].as_str() == Some(sel.as_str())))
        .and_then(|s| s["url"].as_str())
        .map(str::to_string)
}

async fn run(core: SharedCore, url: String, mut rx: mpsc::UnboundedReceiver<String>) {
    loop {
        // 主动关闭的退出点（**必须在循环开头**）：Core 侧 take 掉 ServerSocket 后 tx 被
        // drop，rx 随之关闭。下方的连接失败分支走 `continue` 直接回到这里，不经过循环
        // 尾部——只在尾部检查的话，服务器不可达时这个任务会永远重连下去，连同它持有的
        // `Arc<Core>`（以及 Core 里的 tx，所以 rx 永远不会自己关闭）一起泄漏。
        if rx.is_closed() {
            return;
        }
        // 会话已释放的退出点：`rx.is_closed()` 在 server_mode 下**永远为假**——tx 就在
        // 被本任务抱着的这份 Core 里，不靠 take 不可能关。停机标志（`NativeSession::drop`
        // 置位）是唯一能等到的那条腿：断线重连的下一轮在这里退出，不再去连。
        if core.lock().map(|c| c.stop.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(true) {
            return;
        }
        bridge::queue(&core, Event::Server(ServerEvt::State { s: "connecting".into(), detail: None }));

        let ws = match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _resp)) => ws,
            Err(_) => {
                bridge::queue(&core, Event::Server(ServerEvt::State { s: "connecting".into(), detail: None }));
                tokio::time::sleep(std::time::Duration::from_millis(RECONNECT_MS)).await;
                continue;
            }
        };
        let (mut sink, mut stream) = ws.split();

        // hello（camelCase 字段，服务器 rename_all_fields）
        let hello = {
            let c = match core.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            serde_json::json!({ "t": "hello", "name": c.session.name, "userId": c.session.user_id })
        };
        if sink.send(Message::Text(hello.to_string().into())).await.is_err() {
            continue;
        }

        let mut hb = tokio::time::interval(std::time::Duration::from_millis(HEARTBEAT_MS));
        hb.tick().await; // 第一次立即触发，跳过

        // 读循环：上行（rx）/ 下行（stream）/ 心跳 三路 select
        loop {
            tokio::select! {
                // 上行
                out = rx.recv() => {
                    match out {
                        Some(text) => {
                            if sink.send(Message::Text(text.into())).await.is_err() { break; }
                        }
                        // 发送端被 drop = 主动关闭：不再重连
                        None => return,
                    }
                }
                // 心跳
                _ = hb.tick() => {
                    if sink.send(Message::Text(r#"{"t":"ping"}"#.into())).await.is_err() { break; }
                }
                // 下行
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(txt))) => handle_text(&core, &txt),
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => { /* ping/pong/binary 忽略，与 wasm 侧一致 */ }
                        Some(Err(_)) => break,
                    }
                }
            }
        }

        // 断开：等重连间隔再进下一轮（主动关闭由下一轮开头的 rx 检查兜住）
        tokio::time::sleep(std::time::Duration::from_millis(RECONNECT_MS)).await;
    }
}

/// 下行文本 → ServerEvt（与 wasm 侧 `handle` 逐条对应）。
fn handle_text(core: &SharedCore, txt: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(txt) else { return };
    let t = v["t"].as_str().unwrap_or("").to_string();
    match t.as_str() {
        "welcome" => bridge::queue(core, Event::Server(ServerEvt::State { s: "ready".into(), detail: None })),
        "pong" => {}
        "peers" => {
            let users = v["users"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|u| goptop_net::session::PeerInfo {
                            id: u["id"].as_str().unwrap_or("").into(),
                            name: u["name"].as_str().unwrap_or("").into(),
                            status: u["status"].as_str().unwrap_or("idle").into(),
                            game_id: u["gameId"].as_str().map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default();
            bridge::queue(core, Event::Server(ServerEvt::Peers { users }));
        }
        "signal" => bridge::queue(
            core,
            Event::Server(ServerEvt::Signal {
                from: v["from"].as_str().unwrap_or("").into(),
                kind: v["kind"].as_str().unwrap_or("").into(),
                payload: v["payload"].clone(),
            }),
        ),
        "relayed" => {
            if let Ok(msg) = serde_json::from_value::<goptop_net::protocol::GameMsg>(v["payload"].clone()) {
                bridge::queue(
                    core,
                    Event::Server(ServerEvt::Relayed { from: v["from"].as_str().unwrap_or("").into(), msg }),
                );
            }
        }
        "error" => bridge::queue(
            core,
            Event::Server(ServerEvt::Error {
                msg: v["msg"].as_str().unwrap_or("").into(),
                code: v["code"].as_str().map(str::to_string),
            }),
        ),
        _ => {}
    }
}
