//! goptop-transport — 平台 IO 绑定层（仅 wasm32；native 空壳）。
//!
//! 架构（通道泵模型，避免闭包借用环）：
//! - IO 回调（WS onmessage / RTC 事件 / BC onmessage）只往事件通道塞 [`Event`]；
//! - `drain()`（50ms 定时泵 + 每次命令后同步调用）取空队列逐条喂
//!   `goptop_net::reduce`，产出的 Effect 立即在本层执行（Effect::Emit 触发
//!   JS 钩子 `window.goptopOnChange` → React 读 snapshot）；
//! - 状态机与 IO 层零相互借用，共享经通道，wasm 单线程下无锁。

#![recursion_limit = "256"]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use goptop_net::links::AnswerIntent;
use goptop_net::session::{Event, ReduceCtx, Session, UiCommand};
use wasm_bindgen::prelude::*;

mod bridge;
mod io;

/// 全局会话引用（JS 只持 WasmSession 句柄；IO 回调经 SESSION.with 访问）。
thread_local! {
    static SESSION: RefCell<Option<Rc<RefCell<Core>>>> = const { RefCell::new(None) };
}

/// 核心：状态机 + 待处理事件队列 + IO 句柄（Rc 包装：IO 回调与 effects 共享）。
pub(crate) struct Core {
    pub session: Session,
    pub queue: VecDeque<Event>,
    /// RTC 连接句柄（tag → peer）。
    pub peers: Vec<(String, io::rtc::RtcPeer)>,
    /// 同源 game channel / presence channel。
    pub game_bc: Option<Rc<io::bc::Bc>>,
    pub presence_bc: Option<Rc<io::bc::Bc>>,
    /// 服务器 WS。
    pub ws: Option<Rc<io::ws::ServerSocket>>,
}

fn window() -> web_sys::Window {
    web_sys::window().expect("no global window")
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

fn rand_u32() -> u32 {
    let crypto = window().crypto().expect("no crypto");
    let mut buf = [0u8; 4];
    crypto.get_random_values_with_u8_array(&mut buf).expect("getRandomValues");
    u32::from_ne_bytes(buf)
}

pub(crate) fn storage_get(key: &str) -> Option<String> {
    window().local_storage().ok().flatten()?.get_item(key).ok().flatten()
}

pub(crate) fn storage_set(key: &str, value: Option<&str>) {
    if let Ok(Some(ls)) = window().local_storage() {
        let _ = match value {
            Some(v) => ls.set_item(key, v),
            None => ls.remove_item(key),
        };
    }
}

fn session_storage_get(key: &str) -> Option<String> {
    window().session_storage().ok().flatten()?.get_item(key).ok().flatten()
}

fn session_storage_set(key: &str, value: &str) {
    if let Ok(Some(ss)) = window().session_storage() {
        let _ = ss.set_item(key, value);
    }
}

/// 持久 userId（sessionStorage，每标签页唯一——对齐 C2「每页一身份」）。
fn ensure_user_id() -> String {
    if let Some(id) = session_storage_get("goptop:tabUser") {
        return id;
    }
    let id = goptop_net::identity::gen_user_id(now_ms(), rand_u32());
    session_storage_set("goptop:tabUser", &id);
    id
}

/// 页面级随机 sender ID（GameMsg.sender，每次加载重新生成）。
fn fresh_peer_id() -> String {
    let crypto = window().crypto().expect("no crypto");
    let mut buf = [0u8; 16];
    crypto.get_random_values_with_u8_array(&mut buf).expect("getRandomValues");
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    format!("p-{hex}")
}

/// wasm 入口：创建会话并接通全部 IO。
#[wasm_bindgen]
pub struct WasmSession {
    core: Rc<RefCell<Core>>,
}

#[wasm_bindgen]
impl WasmSession {
    /// 构造（cfg_json：{name, serverMode, shareOrigin, kind, size}）。
    #[wasm_bindgen(constructor)]
    pub fn new(cfg_json: &str) -> WasmSession {
        console_error_panic_hook::set_once();
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Config {
            name: String,
            server_mode: bool,
            share_origin: String,
            kind: String,
            size: u16,
        }
        let cfg: Config = serde_json::from_str(cfg_json).expect("bad config");
        let user_id = ensure_user_id();
        let peer_id = fresh_peer_id();
        let avatar = storage_get("goptop:avatar");
        let stun = io::load_stun_urls();
        let mut session = Session::new(user_id, peer_id, cfg.name, avatar, cfg.server_mode, stun, cfg.share_origin);
        session.kind = cfg.kind;
        session.size = cfg.size;
        session.engine = goptop_core::game::GameState::new(goptop_net::session::make_engine_kind(&session.kind, session.size));
        let core = Rc::new(RefCell::new(Core {
            session,
            queue: VecDeque::new(),
            peers: Vec::new(),
            game_bc: None,
            presence_bc: None,
            ws: None,
        }));
        SESSION.with(|s| *s.borrow_mut() = Some(core.clone()));
        let ws = WasmSession { core };
        let href = window().location().href().unwrap_or_default();
        ws.core.borrow_mut().queue.push_back(Event::Boot { href });
        io::start_presence(&ws.core);
        if ws.core.borrow().session.server_mode {
            io::connect_server(&ws.core);
        }
        ws.drain();
        ws
    }

    /// 当前状态快照（UI 渲染契约）。
    pub fn snapshot(&self) -> String {
        self.core.borrow().session.snapshot()
    }

    /// ICE 调试（E2E/诊断）：各连接的 tag/ICE 状态/本地与远端候选摘要。
    pub fn ice_debug(&self) -> String {
        let core = self.core.borrow();
        let list: Vec<serde_json::Value> = core
            .peers
            .iter()
            .map(|(tag, p)| {
                let cands = |sdp: Option<web_sys::RtcSessionDescription>| -> Vec<String> {
                    sdp.map(|d| {
                        d.sdp()
                            .lines()
                            .filter(|l| l.starts_with("a=candidate"))
                            .map(|l| l.split_whitespace().skip(4).take(2).collect::<Vec<_>>().join("@"))
                            .collect()
                    })
                    .unwrap_or_default()
                };
                let ice = match p.pc.ice_connection_state() {
                    web_sys::RtcIceConnectionState::New => "new",
                    web_sys::RtcIceConnectionState::Checking => "checking",
                    web_sys::RtcIceConnectionState::Connected => "connected",
                    web_sys::RtcIceConnectionState::Completed => "completed",
                    web_sys::RtcIceConnectionState::Failed => "failed",
                    web_sys::RtcIceConnectionState::Disconnected => "disconnected",
                    web_sys::RtcIceConnectionState::Closed => "closed",
                    _ => "unknown",
                };
                let dc = p.dc.borrow().as_ref().map(|c| match c.ready_state() {
                    web_sys::RtcDataChannelState::Connecting => "connecting",
                    web_sys::RtcDataChannelState::Open => "open",
                    web_sys::RtcDataChannelState::Closing => "closing",
                    web_sys::RtcDataChannelState::Closed => "closed",
                    _ => "unknown",
                });
                serde_json::json!({
                    "tag": tag,
                    "ice": ice,
                    "local": cands(p.pc.local_description()),
                    "remote": cands(p.pc.remote_description()),
                    "dcState": dc,
                })
            })
            .collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }

    /// UI 命令（方法集 → UiCommand 入队 → 泵一次）。
    fn cmd(&self, c: UiCommand) {
        self.core.borrow_mut().queue.push_back(Event::Ui(c));
        self.drain();
    }

    /* —— 对局/大厅 —— */
    pub fn create_invite(&self) {
        self.cmd(UiCommand::CreateInvite);
    }
    pub fn accept_invite(&self, inviter_id: String, pwd: Option<String>, kind: String, size: u16, rtc: Option<String>, spec: bool) {
        self.cmd(UiCommand::AcceptInvite { inviter_id, pwd, kind, size, rtc, spec });
    }
    pub fn accept_receipt(&self, receipt_json: &str) {
        if let Ok(ans) = serde_json::from_str::<AnswerIntent>(receipt_json) {
            self.cmd(UiCommand::AcceptReceipt(Box::new(ans)));
        }
    }
    pub fn accept_spec_receipt(&self, receipt_json: &str) {
        if let Ok(ans) = serde_json::from_str::<AnswerIntent>(receipt_json) {
            self.cmd(UiCommand::AcceptSpecReceipt(Box::new(ans)));
        }
    }
    pub fn accept_challenge(&self) {
        self.cmd(UiCommand::AcceptChallenge);
    }
    pub fn reject_challenge(&self) {
        self.cmd(UiCommand::RejectChallenge);
    }
    pub fn server_challenge(&self, to: String) {
        self.cmd(UiCommand::ServerChallenge(to));
    }
    pub fn server_accept_challenge(&self) {
        self.cmd(UiCommand::ServerAcceptChallenge);
    }
    pub fn server_reject_challenge(&self) {
        self.cmd(UiCommand::ServerRejectChallenge);
    }

    /* —— 落子与协商 —— */
    pub fn place(&self, x: u16, y: u16) {
        self.cmd(UiCommand::Place { x, y });
    }
    pub fn pass(&self) {
        self.cmd(UiCommand::Pass);
    }
    pub fn resign(&self) {
        self.cmd(UiCommand::Resign);
    }
    pub fn request_undo(&self) {
        self.cmd(UiCommand::RequestUndo);
    }
    pub fn request_reset(&self) {
        self.cmd(UiCommand::RequestReset);
    }
    pub fn request_swap(&self) {
        self.cmd(UiCommand::RequestSwap);
    }
    pub fn confirm_approve(&self) {
        self.cmd(UiCommand::ConfirmApprove);
    }
    pub fn confirm_decline(&self) {
        self.cmd(UiCommand::ConfirmDecline);
    }
    pub fn toggle_dead(&self, x: u16, y: u16) {
        self.cmd(UiCommand::ToggleDead { x, y });
    }
    pub fn confirm_score(&self) {
        self.cmd(UiCommand::ConfirmScore);
    }

    /* —— 聊天/主页/观战房间 —— */
    pub fn send_chat(&self, text: String) {
        self.cmd(UiCommand::SendChat(text));
    }
    pub fn set_name(&self, name: String) {
        self.cmd(UiCommand::SetName(name));
    }
    pub fn set_avatar(&self, data: Option<String>) {
        self.cmd(UiCommand::SetAvatar(data));
    }
    pub fn pick_kind(&self, k: String) {
        self.cmd(UiCommand::PickKind(k));
    }
    pub fn pick_size(&self, s: u16) {
        self.cmd(UiCommand::PickSize(s));
    }
    pub fn approve_spec(&self, id: String) {
        self.cmd(UiCommand::ApproveSpec(id));
    }
    pub fn reject_spec(&self, id: String) {
        self.cmd(UiCommand::RejectSpec(id));
    }
    pub fn kick_spec(&self, id: String) {
        self.cmd(UiCommand::KickSpec(id));
    }
    pub fn mute_spec(&self, id: String, muted: bool) {
        self.cmd(UiCommand::MuteSpec(id, muted));
    }
    pub fn disable_spectate(&self) {
        self.cmd(UiCommand::DisableSpectate);
    }
    pub fn request_spec_chat(&self) {
        self.cmd(UiCommand::RequestSpecChat);
    }
    pub fn back_home(&self) {
        self.cmd(UiCommand::BackHome);
    }

    /// 【调试探针】specrtc 解码逐层结果。
    pub fn spec_decode_probe(&self, token: String, pwd: String) -> String {
        let _ = self;
        let b64 = goptop_net::codec::b64url_decode(token.trim_start_matches("G1"));
        let enc_len = b64.as_ref().map(|v| v.len()).unwrap_or(0);
        let dec = goptop_net::codec::decode(&token, &pwd);
        let (ok, head) = match &dec {
            Ok(j) => (true, j.chars().take(60).collect()),
            Err(e) => (false, format!("{e:?}")),
        };
        serde_json::json!({ "encLen": enc_len, "ok": ok, "head": head }).to_string()
    }

    /// 链接解析（粘贴弹窗分派用；复用 goptop-net links 解析，跨端一致）。
    /// 返回 JSON：{ok:true, intent:{mode:"user"|..., ...}} 或 {ok:false}。
    pub fn parse_link(&self, text: String) -> String {
        let _ = self;
        match goptop_net::links::parse_pasted_link(&text) {
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

    /// 回执解析（粘贴弹窗用）。返回 {ok:true, answer:{...}} 或 {ok:false}。
    pub fn parse_answer(&self, text: String) -> String {
        let _ = self;
        match goptop_net::links::parse_pasted_answer(&text) {
            Some(a) => serde_json::json!({ "ok": true, "answer": a }).to_string(),
            None => serde_json::json!({ "ok": false }).to_string(),
        }
    }

    /// 事件泵：处理 IO 回调入队的全部事件（JS 以 50ms 定时调用）。
    pub fn drain(&self) {
        loop {
            let ev = self.core.borrow_mut().queue.pop_front();
            let Some(ev) = ev else { break };
            let ctx = ReduceCtx { now_ms: now_ms(), rand: [rand_u32(), rand_u32(), rand_u32(), rand_u32()] };
            let effects = goptop_net::session::reduce(&mut self.core.borrow_mut().session, ev, &ctx);
            bridge::run_effects(&self.core, effects);
        }
        bridge::notify_change();
    }

    /// 安装 50ms 定时泵（App 挂载时调用一次）。
    pub fn start_pump(&self) {
        let f = Closure::<dyn FnMut()>::new(|| {
            SESSION.with(|s| {
                if let Some(core) = s.borrow().clone() {
                    let ws = WasmSession { core };
                    ws.drain();
                }
            });
        });
        let _ = window().set_interval_with_callback_and_timeout_and_arguments_0(f.as_ref().unchecked_ref(), 50);
        f.forget();
    }
}

/// IO 回调入队（crate 内共享）。
pub(crate) fn queue_event(ev: Event) {
    SESSION.with(|s| {
        if let Some(core) = s.borrow().as_ref() {
            core.borrow_mut().queue.push_back(ev);
        }
    });
}
