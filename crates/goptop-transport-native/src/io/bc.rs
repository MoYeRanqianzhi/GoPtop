//! 同源通道（原生）—— 进程内广播。
//!
//! wasm 侧的 BroadcastChannel 解决的是「同一浏览器里两个标签页互通」；native
//! 单实例下没有这个场景，但 `Effect::Broadcast` / `SendPresence` 仍需要一个分发点，
//! 所以这里用 `tokio::sync::broadcast` 做进程内广播：订阅者可以是未来的第二个窗口，
//! 也可以什么都不是（没人订阅时发送是无害的）。
//!
//! **行为差异记录**：wasm 端 BC 的 announce 语义在本层不成立（没有「另一个页面」
//! 可宣告），因此 presence 的跨实例发现能力在 native 端目前是空的。这不影响
//! 服务器模式与 P2P 直连（那两条路走 WS 与 RTC），只影响「同机双窗口自动发现」。

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::SharedCore;

/// 进程内广播通道。
pub struct Bc {
    tx: broadcast::Sender<String>,
    name: String,
}

impl Bc {
    pub fn send(&self, v: serde_json::Value) {
        // 没有订阅者时 send 返回 Err——不是错误，忽略
        let _ = self.tx.send(v.to_string());
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 关闭通道：丢掉发送端，订阅任务随之退出。
    pub fn close(&self) {
        // `broadcast::Sender` 没有显式 close；Core 侧 take() 后整体析构即等价于关闭。
        // 保留这个方法是为了与 wasm 侧 `Bc::close` 的调用面一致（bridge 里会调）。
    }

    /// 克隆一个发送端句柄（`Effect::Broadcast` 要在不持 Core 锁的情况下发）。
    pub fn clone_handle(&self) -> BcHandle {
        BcHandle { tx: self.tx.clone() }
    }
}

/// 脱离 Core 生命周期的发送句柄。
#[derive(Clone)]
pub struct BcHandle {
    tx: broadcast::Sender<String>,
}

impl BcHandle {
    pub fn send(&self, v: serde_json::Value) {
        let _ = self.tx.send(v.to_string());
    }
}

/// 加入通道：建广播并起订阅任务，把收到的文本交给 `on_message` 解析。
///
/// `name` 目前只用于标识（单实例下无路由意义），保留它是为了与 wasm 侧签名一致。
pub fn join(core: &SharedCore, name: &str) {
    let (tx, mut rx) = broadcast::channel::<String>(64);
    let bc = Bc { tx, name: name.to_string() };

    // 订阅任务：只入队事件、不碰状态机（与 wasm 侧回调同款约束）
    let sub = core.clone();
    let user_id = core.lock().ok().map(|c| c.session.user_id.clone()).unwrap_or_default();
    tokio::spawn(async move {
        while let Ok(text) = rx.recv().await {
            presence::on_presence_text(&sub, &user_id, &text);
        }
    });

    if let Ok(mut c) = core.lock() {
        // presence 通道与对局通道共用一个广播（native 单实例下没有「同源双页」要区分）
        if c.presence.is_none() {
            c.presence = Some(bc);
        }
    }
}

/// 启动 presence（与 wasm 侧 `start_presence` 对应）。
pub fn start_presence(core: &Arc<std::sync::Mutex<crate::Core>>) {
    join(core, "goptop-presence-v1");
}

/// presence 文本 → 事件（与 wasm 侧 `io/mod.rs::start_presence` 的分类逐条对应）。
pub mod presence {
    use goptop_net::session::{Event, PresenceEvt};

    use crate::{SharedCore, bridge};

    fn sv(v: &serde_json::Value, k: &str) -> String {
        v[k].as_str().unwrap_or("").to_string()
    }

    pub fn on_presence_text(core: &SharedCore, me: &str, text: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };
        let t = v["t"].as_str().unwrap_or("");
        match t {
            "announce" => {
                // 与 wasm 侧同款简化：收到的 announce 仅当存活信号，下发空名册
                bridge::queue(core, Event::Presence(PresenceEvt::Peers { peers: Vec::new() }));
            }
            "challenge" if v["to"].as_str() == Some(me) => {
                bridge::queue(
                    core,
                    Event::Presence(PresenceEvt::Challenge {
                        from: sv(&v, "from"),
                        from_name: sv(&v, "fromName"),
                        pwd: v["pwd"].as_str().map(str::to_string),
                        kind: sv(&v, "kind"),
                        size: v["size"].as_u64().unwrap_or(15) as u16,
                        game_id: sv(&v, "gameId"),
                        rtc_ans: v["rtcAns"].as_str().map(str::to_string),
                    }),
                );
            }
            "accept" if v["to"].as_str() == Some(me) => {
                bridge::queue(core, Event::Presence(PresenceEvt::Accept { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
            }
            "reject" if v["to"].as_str() == Some(me) => {
                bridge::queue(core, Event::Presence(PresenceEvt::Reject { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
            }
            _ => {}
        }
    }
}
