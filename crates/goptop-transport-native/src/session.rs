//! NativeSession —— 传输层对外的会话句柄（对标 wasm 的 `WasmSession`）。
//!
//! 与 wasm 版的三处有意差异：
//! 1. **身份持久化**：wasm 用 sessionStorage 做到「每标签页一身份」；native 是
//!    每设备一身份，所以 `user_id` 落宿主存储（桌面 `~/.goptop`、移动端私有目录）。
//! 2. **泵由 tokio 驱动**：wasm 靠 `setInterval(50ms)`；这边用 tokio interval，
//!    外加「每次命令后同步泵一次」，与 wasm 的行为一致。
//! 3. **平台动作走 `Host` trait**：wasm 侧调 `window.goptop*` 全局钩子，这边调
//!    宿主实现——无头测试用 `HeadlessHost` 即可跑通全部逻辑。
//!
//! # 宿主必须先进运行时
//!
//! 本模块到处 `tokio::spawn`（建连接、收消息、定时器）。宿主的调用点常常不在任何
//! 运行时上下文里——Tauri 的同步命令在主线程、NAPI 回调在 ArkWeb 线程——没有上下文
//! 就是 `there is no reactor running` **panic 在主线程上、整个进程退出**。
//! 碰会话之前先 `let _g = crate::enter_runtime();`，见该函数的说明。

use std::sync::Arc;
use std::time::Duration;

use goptop_core::game::GameState;
use goptop_net::session::{Event, ReduceCtx, Session, UiCommand};

use crate::host::Host;
use crate::{Core, SharedCore, SessionConfig, bridge, now_ms, rand4};

/// 会话句柄。
pub struct NativeSession {
    core: SharedCore,
    host: Arc<dyn Host>,
    /// 后台泵的停机标志（[`Drop`] 置位）。
    ///
    /// 泵是个 `loop { tick; pump }` 的常驻任务，不显式叫停的话，会话被 drop 之后
    /// 它仍每 50ms 醒一次、抱着 `Arc<Core>` 与宿主不放——切一次页面漏一个，
    /// 而且宿主（Tauri 的 `AppHandle`）被常驻引用着，进程也退不干净。
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl NativeSession {
    /// 构造：建会话、起 presence、按配置连服务器，并泵一次（处理 `Event::Boot`）。
    pub fn new(cfg: SessionConfig, host: Arc<dyn Host>, href: &str) -> Self {
        let user_id = ensure_user_id(host.as_ref());
        let peer_id = fresh_peer_id();
        let avatar = host.storage_get("goptop:avatar");
        let stun = load_stun_urls(host.as_ref());
        let mut session = Session::new(
            user_id,
            peer_id,
            cfg.name,
            avatar,
            cfg.server_mode,
            stun,
            cfg.share_origin,
        );
        session.kind = cfg.kind.clone();
        session.size = cfg.size;
        session.engine = GameState::new(goptop_net::session::make_engine_kind(&session.kind, session.size));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let core: SharedCore = Arc::new(std::sync::Mutex::new(Core {
            session,
            queue: std::collections::VecDeque::new(),
            peers: Vec::new(),
            presence: None,
            bc_topics: std::sync::Mutex::new(std::collections::HashSet::new()),
            ws: None,
            // 与 self.stop 同一个 Arc：会话释放时不止后台泵，bc 订阅与 ws 重连
            // 任务也得跟着退（它们各自的旧退出条件在这进程里等不到，见 Core.stop）。
            stop: stop.clone(),
        }));

        let me = Self { core, host, stop };
        bridge::queue(&me.core, Event::Boot { href: href.to_string() });
        crate::io::bc::start_presence(&me.core);
        if me.core.lock().map(|c| c.session.server_mode).unwrap_or(false) {
            let url = crate::io::ws::selected_server_url(me.host.as_ref());
            crate::io::ws::connect(&me.core, url);
        }
        me.pump();
        me
    }

    /// 当前状态快照（UI 渲染契约，与 wasm 侧同一份 `Session::snapshot`）。
    pub fn snapshot(&self) -> String {
        self.core.lock().map(|c| c.session.snapshot()).unwrap_or_else(|_| "null".into())
    }

    /// 当前对局局面的完整序列化（AI 分析的输入）。
    pub fn state_json(&self) -> String {
        self.core
            .lock()
            .map(|c| goptop_core::json_api::state_json(&c.session.engine))
            .unwrap_or_else(|_| "null".into())
    }

    /// 泵一次：drain 事件 → 执行 Effect（`bridge::pump`）。
    pub fn pump(&self) {
        bridge::pump(&self.core, &self.host);
    }

    /// 起一个后台泵（50ms，与 wasm 侧同周期）。
    ///
    /// 定时器 Effect（`Effect::Timer`）由 `bridge` 各自 spawn，不依赖这个泵；
    /// 泵只负责把 IO 回调塞进队列的事件消化掉。
    pub fn start_pump(&self) {
        let core = self.core.clone();
        let host = self.host.clone();
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            loop {
                tick.tick().await;
                // 会话已释放就退出（见 `stop` 的说明）。检查放在 pump 之前：
                // 释放之后再去碰 Core 只会白跑。
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                bridge::pump(&core, &host);
            }
        });
    }

    /// UI 命令（对标 wasm 侧那一组 `#[wasm_bindgen]` 方法）：入队后同步泵一次。
    pub fn cmd(&self, c: UiCommand) {
        bridge::queue(&self.core, Event::Ui(c));
        self.pump();
    }

    /// 解析粘贴的链接（对标 wasm 的 `parse_link`）。
    pub fn parse_link(&self, text: &str) -> String {
        parse_link_json(text)
    }

    /// 解析粘贴的回执（对标 wasm 的 `parse_answer`）。
    pub fn parse_answer(&self, text: &str) -> String {
        parse_answer_json(text)
    }

    /// 连接诊断（E2E 用，与 wasm 的 `ice_debug` **同用途但不同深度**）。
    ///
    /// wasm 侧能同步读到 `RTCPeerConnection.iceConnectionState` 与候选列表；webrtc-rs
    /// 的对应 API 是 async，而本方法要与 wasm 保持同步签名（前端的诊断钩子是同步读）。
    /// 因此这里只报**能从账本直接读到的**：连接 tag 与是否已关闭。想要完整 ICE 状态
    /// 得等 webrtc-rs 侧把状态变化回写进句柄——那是另一件事，不假装有。
    pub fn ice_debug(&self) -> String {
        let Ok(c) = self.core.lock() else { return "[]".into() };
        let list: Vec<serde_json::Value> = c
            .peers
            .iter()
            .map(|(tag, p)| serde_json::json!({ "tag": tag, "closed": p.is_closed() }))
            .collect();
        serde_json::Value::Array(list).to_string()
    }

    /// 观战/服务器内部状态（E2E 诊断用，与 wasm 的 `state_debug` 同契约）。
    pub fn state_debug(&self) -> String {
        let Ok(c) = self.core.lock() else { return "{}".into() };
        let s = &c.session;
        serde_json::json!({
            "role": format!("{:?}", s.role),
            "phase": format!("{:?}", s.phase),
            "myHost": s.my_host,
            "serverState": s.server_state,
            "serverMode": s.server_mode,
            "spectators": s.spectators.len(),
            "opponent": s.opponent,
        })
        .to_string()
    }
}

/// 持久化身份：首次生成后落宿主存储（每设备一身份，区别于 wasm 的每标签页）。
fn ensure_user_id(host: &dyn Host) -> String {
    const KEY: &str = "goptop:userId";
    if let Some(id) = host.storage_get(KEY) {
        if !id.is_empty() {
            return id;
        }
    }
    let id = goptop_net::identity::gen_user_id(now_ms(), rand4()[0]);
    host.storage_set(KEY, Some(&id));
    id
}

/// 页面级随机 sender ID（与 wasm 侧 `fresh_peer_id` 同语义：每次会话重新生成）。
fn fresh_peer_id() -> String {
    let r = rand4();
    format!("p-{:08x}{:08x}{:08x}{:08x}", r[0], r[1], r[2], r[3])
}

/// STUN 线路（与 wasm 侧 `io::load_stun_urls` 同键名/同默认）。
pub fn load_stun_urls(host: &dyn Host) -> Vec<String> {
    const DEFAULT_ON: &[&str] = &[
        "stun:stun.miwifi.com:3478",
        "stun:stun.chat.bilibili.com:3478",
        "stun:stun.cloudflare.com:3478",
    ];
    match host
        .storage_get("goptop:stun")
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        Some(v) if v.is_array() => v
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["enabled"].as_bool() == Some(true))
            .filter_map(|l| l["urls"].as_str())
            .map(str::to_string)
            .collect(),
        _ => DEFAULT_ON.iter().map(|s| s.to_string()).collect(),
    }
}

/// 链接解析（复用 goptop-net，跨端一致；输出 JSON 与 wasm 侧逐字相同）。
pub fn parse_link_json(text: &str) -> String {
    match goptop_net::links::parse_pasted_link(text) {
        Some(it) => {
            let intent = match it {
                goptop_net::links::UrlIntent::User { user_id, pwd, kind, size, rtc, spec } => serde_json::json!({
                    "mode": "user", "userId": user_id, "pwd": pwd, "kind": kind, "size": size, "rtc": rtc, "spec": spec,
                }),
                goptop_net::links::UrlIntent::Watch { game_id } => serde_json::json!({ "mode": "watch", "gameId": game_id }),
                goptop_net::links::UrlIntent::Local => serde_json::json!({ "mode": "local" }),
                goptop_net::links::UrlIntent::P2p => serde_json::json!({ "mode": "p2p" }),
                goptop_net::links::UrlIntent::Users => serde_json::json!({ "mode": "users" }),
                goptop_net::links::UrlIntent::Settings => serde_json::json!({ "mode": "settings" }),
                goptop_net::links::UrlIntent::Menu => serde_json::json!({ "mode": "menu" }),
            };
            serde_json::json!({ "ok": true, "intent": intent }).to_string()
        }
        None => serde_json::json!({ "ok": false }).to_string(),
    }
}

/// 回执解析（输出 JSON 与 wasm 侧逐字相同）。
pub fn parse_answer_json(text: &str) -> String {
    match goptop_net::links::parse_pasted_answer(text) {
        Some(a) => serde_json::json!({ "ok": true, "answer": a }).to_string(),
        None => serde_json::json!({ "ok": false }).to_string(),
    }
}

/// 供桥接层复用的 `ReduceCtx` 构造。
pub fn reduce_ctx() -> ReduceCtx {
    ReduceCtx { now_ms: now_ms(), rand: rand4() }
}

/// 会话生命周期的回归（要数私有 `core` 字段的 Arc 计数，只能放 crate 内）。
#[cfg(test)]
mod tests {
    use super::*;

    use crate::{HeadlessHost, test_support};

    /// `start_pump` + [`Drop`] 的完整链路：会话释放后，后台泵与 bc 订阅任务必须
    /// 交还 `Arc<Core>`。集成测试（tests/headless.rs）从不 drop 会话，这条路径
    /// 曾完全没有覆盖——而宿主的生命周期契约（切页/重开都靠 drop 释放）恰恰
    /// 建在它上面。
    #[tokio::test]
    async fn 会话释放后_泵与订阅任务交还核心() {
        let host: Arc<dyn Host> = Arc::new(HeadlessHost::default());
        let s = NativeSession::new(test_support::config("甲"), host, "http://localhost/life");
        s.start_pump();
        let weak = Arc::downgrade(&s.core);
        drop(s); // Drop 置位 stop（与 Core.stop 同一个 Arc）

        // 后台泵 50ms 一拍、bc 订阅也是 50ms 停机轮询：给足几拍
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(60)).await;
            if weak.upgrade().is_none() {
                break;
            }
        }
        assert!(
            weak.upgrade().is_none(),
            "会话释放后所有常驻任务必须退出并交还 Core，否则每切一次页面漏一份"
        );
    }
}
