//! goptop-transport-native — 传输层的**原生宿主**（桌面 / Android / 鸿蒙）。
//!
//! 与 `goptop-transport`（wasm 专用）的分工：两者都是 `goptop-net` 那套
//! `Event`/`Effect` 状态机的**执行器**，区别只在 IO 用什么实现。
//!
//! | IO | wasm 侧 | 本 crate |
//! |---|---|---|
//! | WebSocket | 浏览器 `WebSocket` | `tokio-tungstenite` |
//! | WebRTC | `RTCPeerConnection` | `webrtc` (webrtc-rs) |
//! | 同源通道 | `BroadcastChannel` | 进程内广播（native 单实例，见 io/bc.rs） |
//! | 定时器 | `setTimeout` | `tokio::time` |
//! | 存储 / 剪贴板 | 宿主钩子 / `navigator.clipboard` | 宿主回调（Tauri command / NAPI） |
//!
//! **为什么另起一个 crate 而不是给 goptop-transport 加 cfg**：两者的并发模型
//! 根本不同——wasm 是单线程 `Rc<RefCell<Core>>` + `setInterval` 泵；native 要跨
//! tokio 任务共享，必须 `Arc<Mutex<Core>>`。塞进一个 crate 会满屏 cfg，且共享状态
//! 类型没法统一。
//!
//! **状态机是同步的，IO 是异步的**：所有 IO 回调只把 `Event` 塞进队列
//! （[`Core::queue`]），由泵统一 `reduce`；产出的 `Effect` 立刻在本层执行。
//! 这与 wasm 侧同构，保证两端行为一致。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use goptop_net::session::{Event, ReduceCtx, Session};

pub mod bridge;
pub mod host;
pub mod io;

/// 核心：状态机 + 待处理事件队列 + IO 句柄。
///
/// 与 wasm 版的 `Core` 逐字对应，只有共享方式不同（`Arc<Mutex>` vs `Rc<RefCell>`）。
pub struct Core {
    pub session: Session,
    pub queue: VecDeque<Event>,
    /// RTC 连接句柄（tag → peer）。
    pub peers: Vec<(String, io::rtc::RtcPeer)>,
    /// 同源 presence 通道（native 单实例下同进程广播，见 io/bc.rs）。
    pub presence: Option<io::bc::Bc>,
    /// 服务器 WS。
    pub ws: Option<io::ws::ServerSocket>,
}

/// 共享核心（IO 任务与泵之间）。
pub type SharedCore = Arc<Mutex<Core>>;

impl Core {
    /// 事件入队。**只入队不处理**——IO 回调在各自的 tokio 任务里跑，
    /// 直接 reduce 会与泵竞争状态机；统一交给泵串行处理。
    pub fn queue(&mut self, ev: Event) {
        self.queue.push_back(ev);
    }

    /// 处理队列中的全部事件，返回本次产生的 Effect。
    pub fn drain(&mut self) -> Vec<goptop_net::session::Effect> {
        let mut out = Vec::new();
        while let Some(ev) = self.queue.pop_front() {
            let ctx = ReduceCtx {
                now_ms: now_ms(),
                rand: rand4(),
            };
            out.extend(goptop_net::session::reduce(&mut self.session, ev, &ctx));
        }
        out
    }
}

/// 单调毫秒。
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 4 个随机 u32（状态机的 ctx.rand）。xorshift64 够用——状态机要的是
/// 「每次不同」，不是密码学强度。
pub fn rand4() -> [u32; 4] {
    let mut x = now_ms() ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = [0u32; 4];
    for slot in &mut out {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *slot = (x >> 32) as u32;
    }
    out
}
