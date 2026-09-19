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

    /// 关闭：丢掉发送端，IO 任务的 `rx.recv()` 返回 None 后自然退出（不重连）。
    pub fn close(&self) {
        // 显式 drop 不了 &self 里的 tx，交给 Core 侧 take() 后整体析构
    }
}

/// 连接服务器：建 IO 任务并把句柄挂进 Core。
pub fn connect(core: &SharedCore, url: String) {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    if let Ok(mut c) = core.lock() {
        c.ws = Some(ServerSocket { tx });
    }
    let core = core.clone();
    tokio::spawn(async move { run(core, url, rx).await });
}

async fn run(core: SharedCore, url: String, mut rx: mpsc::UnboundedReceiver<String>) {
    loop {
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

        // 断开：等重连间隔再进下一轮
        tokio::time::sleep(std::time::Duration::from_millis(RECONNECT_MS)).await;
        // 期间若被主动关闭（rx 关闭），下一轮 loop 会因 recv 返回 None 退出
        if rx.is_closed() {
            return;
        }
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
