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

use std::sync::OnceLock;

use tokio::sync::broadcast;

use crate::SharedCore;

/// 广播载荷：`(发送者 userId, 文本)`。
///
/// **必须带发送者**：浏览器的 `BroadcastChannel` 不回送给发送者自己，状态机因此
/// 从不自过滤。进程内广播若原样回送，发送端会把自己的消息再处理一遍、又发一次，
/// 形成回路——实测表现为「A 同时发出 accept 与 reject」，对端刚进对局就被踢回主页。
type Payload = (String, String);

/// **进程内共享**的广播通道。
///
/// 关键：不能每个 `Core` 各建一个——那样同进程里的两个会话（同机双窗口、无头测试的
/// 两端）永远收不到对方的消息，而 wasm 侧 BroadcastChannel 恰恰是同源页面互通的。
/// 用一个全局 channel 才能对等实现「同源广播」的语义。
fn hub() -> &'static broadcast::Sender<Payload> {
    static HUB: OnceLock<broadcast::Sender<Payload>> = OnceLock::new();
    HUB.get_or_init(|| broadcast::channel::<Payload>(256).0)
}

/// 进程内广播通道句柄。
pub struct Bc {
    name: String,
    /// 本会话的 userId：发送时带上，供订阅端过滤自己。
    me: String,
}

impl Bc {
    pub fn send(&self, v: serde_json::Value) {
        // 没有订阅者时 send 返回 Err——不是错误，忽略
        if std::env::var("GOPTOP_TRACE_BC").is_ok() {
            eprintln!("[bc send {}] {}", self.me, v.to_string().chars().take(160).collect::<String>());
        }
        let _ = hub().send((self.me.clone(), v.to_string()));
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 关闭通道。全局 hub 不因单个会话关闭而销毁（其他会话还在用），
    /// 订阅任务在自己的 `rx` 失效时自然退出。
    pub fn close(&self) {
        // 保留方法是为了与 wasm 侧 `Bc::close` 的调用面一致（bridge 里会调）
    }

    /// 克隆一个发送端句柄（`Effect::Broadcast` 要在不持 Core 锁的情况下发）。
    pub fn clone_handle(&self) -> BcHandle {
        BcHandle { me: self.me.clone() }
    }
}

/// 脱离 Core 生命周期的发送句柄。
#[derive(Clone)]
pub struct BcHandle {
    me: String,
}

impl BcHandle {
    pub fn send(&self, v: serde_json::Value) {
        let _ = hub().send((self.me.clone(), v.to_string()));
    }
}

/// 加入通道：订阅全局 hub，把收到的文本交给 presence 分类器。
///
/// **必须幂等**：`Effect::JoinChannel` 在对局开始/重开/受邀入局时都会触发，
/// 每次调用都挂一个新订阅者的话，同一条广播会被处理多遍——实测表现为
/// 「challenge 被受理 4 次」，而受理之后残留的重复处理又发 reject，
/// 把刚进对局的受邀者踢回主页。
///
/// `name` 目前只用于标识（native 单实例下无路由意义），保留是为了与 wasm 侧签名一致。
pub fn join(core: &SharedCore, name: &str) {
    if core.lock().map(|c| c.presence.is_some()).unwrap_or(false) {
        return;
    }

    let me = core.lock().map(|c| c.session.user_id.clone()).unwrap_or_default();
    let bc = Bc { name: name.to_string(), me: me.clone() };

    let mut rx = hub().subscribe();
    let sub = core.clone();
    tokio::spawn(async move {
        while let Ok((from, text)) = rx.recv().await {
            // 不回送发送者自己——对齐 BroadcastChannel 语义（见 Payload 的说明）
            if from == me {
                continue;
            }
            presence::on_presence_text(&sub, &me, &text);
        }
    });

    if let Ok(mut c) = core.lock() {
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
        if std::env::var("GOPTOP_TRACE_BC").is_ok() {
            eprintln!("[bc recv me={me}] {}", text.chars().take(160).collect::<String>());
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };

        // **先分对局消息**：广播通道送两类东西——presence（带 `t`）与对局 GameMsg
        //（带 `kind`/`sender`/`seq`）。只认 `t` 的话，GameMsg 会被整条丢掉，
        // 表现为「对手的连接是通的、广播也发出去了，但这边的棋盘不动」
        //（实测：`on_net` 从未被调用，因为消息在 presence 分类器里就被忽略了）。
        if v["kind"].is_object() && v["sender"].is_string() {
            if let Ok(msg) = serde_json::from_value::<goptop_net::protocol::GameMsg>(v) {
                bridge::queue(core, Event::Net(msg));
            }
            return;
        }

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
