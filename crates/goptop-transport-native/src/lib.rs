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
pub mod session;

pub use host::{HeadlessHost, Host};
pub use session::NativeSession;

/// 会话配置（与 wasm 版 `WasmSession::new` 的 cfg_json 同字段）。
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfig {
    pub name: String,
    pub server_mode: bool,
    pub share_origin: String,
    pub kind: String,
    pub size: u16,
}

/// 核心：状态机 + 待处理事件队列 + IO 句柄。
///
/// 与 wasm 版的 `Core` 一一对应，只有两处差异：共享方式（`Arc<Mutex>` vs
/// `Rc<RefCell>`），以及同源通道——wasm 分「presence / 对局」两个 `Bc`，这边只有一个
/// 全局订阅（见 io/bc.rs 的说明）。
pub struct Core {
    pub session: Session,
    pub queue: VecDeque<Event>,
    /// RTC 连接句柄（tag → peer）。
    pub peers: Vec<(String, io::rtc::RtcPeer)>,
    /// 同源 presence 通道（native 单实例下同进程广播，见 io/bc.rs）。
    pub presence: Option<io::bc::Bc>,
    /// 本会话**允许收信的 BC 主题集**（presence 常驻 + JoinChannel 的对局频道）。
    /// 投递按集过滤——跨局串扰（R5/2026-10-08 重大 bug：旧局的认输广播被新局
    /// 当成自己的对局消息，空盘「黑胜」）的根治就是 per-game topic。
    pub bc_topics: std::sync::Mutex<std::collections::HashSet<String>>,
    /// 服务器 WS。
    pub ws: Option<io::ws::ServerSocket>,
    /// 会话停机标志，与 `NativeSession::drop` 置位的是**同一个** `Arc`。
    ///
    /// 常驻 IO 任务（bc 订阅、ws 重连）都抱着 `Arc<Core>`，而它们的旧退出条件
    /// ——bc 靠 `recv()` 返回 Closed、ws 靠 `rx.is_closed()`——在进程里**永远等
    /// 不到**：hub 的 sender 是 `'static` 的、ws 的 tx 就在被任务自己抱着的这份
    /// Core 里。于是每次会话释放（切页/重开对局）都漏一整份 Core（连带 RTC 连接
    /// 与 socket）。有这个标志后，各任务与后台泵同一套约定：看到置位即退。
    pub stop: Arc<std::sync::atomic::AtomicBool>,
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

/// 传输层的调度运行时。
///
/// **宿主碰会话之前必须先进来**：传输层到处 `tokio::spawn`（建连接、收消息、
/// 定时器），而宿主的调用点往往不在任何运行时上下文里——Tauri 的**同步**命令跑在
/// 主线程上，NAPI 的回调跑在 ArkWeb 的线程上。没有上下文就是
/// `there is no reactor running, must be called from the context of a Tokio 1.x
/// runtime` **panic 在主线程上**，整个应用进程随之退出（实测：桌面壳在开局的
/// 那一刻直接崩掉）。
///
/// 用法：`let _g = enter_runtime();` 然后照常调 `NativeSession::new` / `pump` /
/// `cmd`。守卫离开作用域自动还原线程上下文。
///
/// **不要指望宿主框架的运行时**：那是实现细节，会随版本变；自持一个还让传输层的
/// 生命周期与窗口框架解耦。无头测试用 `#[tokio::test]` 自带的运行时即可，
/// 嵌套 `enter()` 是安全的（守卫析构时还原上一层）。
pub fn enter_runtime() -> tokio::runtime::EnterGuard<'static> {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("goptop-net")
            .build()
            .expect("无法创建 tokio 运行时：传输层全靠它调度，起不来等于 P2P 不可用")
    })
    .enter()
}

/// 毫秒时间戳（**墙钟**，与 wasm 侧 `Date.now()` 同一时间轴）。
///
/// 注意它**不是单调时钟**：系统时间被回拨（NTP 校时）时会倒退，状态机的超时判定
/// 随之整体后移。这里刻意与 wasm 保持一致——换 `Instant` 会让两端对同一份状态机
/// 得出不同的超时结论，那才是真正难查的分叉。
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 4 个随机 u32（状态机的 ctx.rand）。
///
/// **必须有真熵，不能只用时间**：先前只拿 `now_ms()` 做 xorshift 种子，同一毫秒内
/// 建的两个会话会得到完全相同的序列——实测表现为两个会话拿到**同一个 userId**，
/// 于是 P2P 挑战被发给自己、配对彻底失败。`RandomState` 由标准库注入进程级熵
/// （每实例种子不同），再混入时间和自增计数，足以让任意两次调用都不同。
pub fn rand4() -> [u32; 4] {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut out = [0u32; 4];
    for slot in &mut out {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(now_ms());
        h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
        // `| 1` 保证非零：下游 `gen_game_id`/`gen_user_id` 直接把 u32 编进 ID，
        // 取到 0 会产出 `g-000000000000` 这种全零 ID（实测踩到）。
        *slot = (h.finish() as u32) | 1;
    }
    out
}

/// 本 crate 单测共用的最小构造（`#[cfg(test)]` 才编译）。
///
/// 生命周期类断言（订阅任务/泵任务是否随会话退出）要直接数 `Arc<Core>` 的
/// 引用计数，集成测试够不到 `NativeSession` 的私有 `core` 字段，只能放 crate 内。
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// 无 IO 句柄的最小会话核：presence/ws/peers 全空，server_mode 关（不起 WS）。
    pub fn core(user_id: &str) -> SharedCore {
        let session = goptop_net::session::Session::new(
            user_id.to_string(),
            format!("p-{user_id}"),
            "测试".to_string(),
            None,
            false,
            Vec::new(),
            "https://goptop.pages.dev".to_string(),
        );
        Arc::new(std::sync::Mutex::new(Core {
            session,
            queue: VecDeque::new(),
            peers: Vec::new(),
            presence: None,
            bc_topics: std::sync::Mutex::new(std::collections::HashSet::new()),
            ws: None,
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }))
    }

    /// 最小会话配置（无服务器、五子棋 15 路），供 `NativeSession::new` 用。
    pub fn config(name: &str) -> SessionConfig {
        SessionConfig {
            name: name.to_string(),
            server_mode: false,
            share_origin: "https://goptop.pages.dev".to_string(),
            kind: "gomoku".to_string(),
            size: 15,
        }
    }
}
