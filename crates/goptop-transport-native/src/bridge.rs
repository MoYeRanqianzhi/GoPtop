//! Effect 执行 —— 把状态机吐出的抽象动作落到 native IO 上。
//!
//! 与 wasm 侧 `goptop-transport/src/bridge.rs` 逐条对应；两端的**行为必须一致**，
//! 差异只允许出现在「用什么实现」这一层（浏览器 API ↔ tokio / webrtc-rs）。
//!
//! 需要 IO 的 Effect（连服务器、建连接、发消息）在这里只**派发任务**，实际 IO
//! 在各自模块的 tokio 任务里跑；回调再把 `Event` 塞回队列由泵处理——这样状态机
//! 永远是单线程串行访问的，不需要考虑重入。

use std::sync::Arc;

use goptop_net::session::Effect;

use crate::host::Host;
use crate::{Core, SharedCore, io};

/// 执行一批 Effect。`core` 由调用方保证未持锁（本函数内部会再取）。
pub fn run_effects(core: &SharedCore, host: &Arc<dyn Host>, effects: Vec<Effect>) {
    for e in effects {
        match e {
            Effect::Emit => {
                let snap = match core.lock() {
                    Ok(c) => c.session.snapshot(),
                    Err(_) => continue,
                };
                host.emit(&snap);
            }
            Effect::Notice(text, ms) => host.notice(text.as_deref(), ms),
            Effect::Nav(path) => host.nav(&path),
            Effect::SetStorage { key, value } => host.storage_set(&key, value.as_deref()),
            Effect::Copy { text, ok_msg } => {
                host.copy(&text);
                host.notice(Some(&ok_msg), Some(2000));
            }

            Effect::Timer { id, ms } => {
                let core = core.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    queue(&core, goptop_net::session::Event::Timer(id));
                });
            }

            Effect::ServerConnect(url) => io::ws::connect(core, Some(url)),
            Effect::SendServer(v) => {
                if let Ok(c) = core.lock() {
                    if let Some(ws) = &c.ws {
                        ws.send(&v);
                    }
                }
            }
            Effect::ServerClose => {
                let ws = core.lock().ok().and_then(|mut c| c.ws.take());
                if let Some(ws) = ws {
                    ws.close();
                }
            }
            Effect::SendPresence(v) => {
                if let Ok(c) = core.lock() {
                    if let Some(bc) = &c.presence {
                        bc.send(v);
                    }
                }
            }
            Effect::JoinChannel(name) => io::bc::join(core, &name),
            Effect::LeaveChannel => {
                let bc = core.lock().ok().and_then(|mut c| c.presence.take());
                if let Some(bc) = bc {
                    bc.close();
                }
            }

            Effect::CreatePeer { tag, inviter, spectator } => {
                io::rtc::create(core, tag, inviter, spectator);
            }
            // 说明：offer/answer 的「包 JSON + pwd 加密」在 io::rtc 生成 SDP 之后由
            // 本层补齐（见 io::rtc 里回喂 RtcReady 前的 encode_payload），与 wasm 侧
            // bridge 的分工一致——rtc 层只管 SDP，编码不归它管。
            Effect::FeedOffer { tag, offer, encrypted } => {
                io::rtc::feed_offer(core, &tag, &offer, encrypted);
            }
            Effect::AcceptAnswer { tag, answer, encrypted } => {
                io::rtc::accept_answer(core, &tag, &answer, encrypted);
            }
            Effect::RenamePeer { from, to } => {
                if let Ok(mut c) = core.lock() {
                    if let Some(p) = c.peers.iter_mut().find(|(t, _)| *t == from) {
                        p.0 = to;
                    }
                }
            }
            Effect::ClosePeers => {
                let peers = core.lock().ok().map(|mut c| std::mem::take(&mut c.peers));
                if let Some(peers) = peers {
                    for (_, p) in peers {
                        p.close();
                    }
                }
            }

            Effect::Broadcast(msg) => {
                let bc = core.lock().ok().and_then(|c| c.presence.as_ref().map(|b| b.clone_handle()));
                if let Some(bc) = bc {
                    bc.send(serde_json::to_value(&msg).unwrap_or(serde_json::Value::Null));
                }
                io::rtc::broadcast(core, &msg);
            }
        }
    }
}

/// 事件入队（IO 回调统一走这里）。
pub fn queue(core: &SharedCore, ev: goptop_net::session::Event) {
    if let Ok(mut c) = core.lock() {
        c.queue(ev);
    }
}

/// 泵一次：drain 事件 → 执行 Effect。
///
/// **不要持锁执行 Effect**：Effect 里可能再次 lock（如 SendServer 要读 ws），
/// 持锁会死锁。所以先 drain（短临界区），出锁后再执行。
pub fn pump(core: &SharedCore, host: &Arc<dyn Host>) {
    let effects = match core.lock() {
        Ok(mut c) => c.drain(),
        Err(_) => return,
    };
    run_effects(core, host, effects);
}

/// 供 io 模块用的便捷读取：取会话里的某个字段。
pub fn with_core<T>(core: &SharedCore, f: impl FnOnce(&Core) -> T) -> Option<T> {
    core.lock().ok().map(|c| f(&c))
}
