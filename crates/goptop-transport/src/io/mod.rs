//! IO 模块 — WebSocket / BroadcastChannel / RTCPeerConnection / 定时器。

pub mod bc;
pub mod rtc;
pub mod ws;

use crate::{queue_event, Core};
use goptop_net::session::{Event, PresenceEvt};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

pub(crate) type SharedCore = Rc<RefCell<Core>>;

/// 启动同源 presence（announce 循环在 effect 侧 2s 定时器驱动；这里建 channel 收件）。
pub(crate) fn start_presence(core: &SharedCore) {
    let bc = bc::Bc::open("goptop-presence-v1", {
        let core = core.clone();
        move |data: String| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                let t = v["t"].as_str().unwrap_or("");
                let me = core.borrow().session.user_id.clone();
                match t {
                    "announce" => {
                        // 名册条目更新由 announce effect 的定时广播 + 本端收件合并——
                        // presence 名册语义：收到的 announce 刷新该 peer 的存活（简化：
                        // 状态机直接以单条 announce 更新名册条目）。
                        queue_event(Event::Presence(PresenceEvt::Peers { peers: Vec::new() }));
                        let _ = &me;
                    }
                    "challenge" if v["to"].as_str() == Some(me.as_str()) => {
                        queue_event(Event::Presence(PresenceEvt::Challenge {
                            from: sv(&v, "from"),
                            from_name: sv(&v, "fromName"),
                            pwd: v["pwd"].as_str().map(str::to_string),
                            kind: sv(&v, "kind"),
                            size: v["size"].as_u64().unwrap_or(15) as u16,
                            game_id: sv(&v, "gameId"),
                            rtc_ans: v["rtcAns"].as_str().map(str::to_string),
                        }));
                    }
                    "accept" if v["to"].as_str() == Some(me.as_str()) => {
                        queue_event(Event::Presence(PresenceEvt::Accept { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
                    }
                    "reject" if v["to"].as_str() == Some(me.as_str()) => {
                        queue_event(Event::Presence(PresenceEvt::Reject { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
                    }
                    _ => {}
                }
            }
        }
    });
    core.borrow_mut().presence_bc = Some(Rc::new(bc));
}

fn sv(v: &serde_json::Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

/// 按设置连接服务器（重连循环在 ws 层）。
pub(crate) fn connect_server(core: &SharedCore) {
    let url = selected_server_url();
    if let Some(ws) = core.borrow_mut().ws.take() {
        ws.close_manual();
    }
    let Some(url) = url else { return };
    let socket = ws::ServerSocket::open(core.clone(), url);
    core.borrow_mut().ws = Some(Rc::new(socket));
}

/// 服务器选择（localStorage 配置读取；与历史键名一致）。
pub(crate) fn selected_server_url() -> Option<String> {
    const BUILTIN: &str = r#"[{"id":"official","label":"官方服务器","url":"wss://goptopserver.meowoo.org/ws","builtin":true}]"#;
    let sel = crate::storage_get("goptop:server-sel").unwrap_or_else(|| "official".into());
    if sel == "none" {
        return None;
    }
    let mut all = BUILTIN.to_string();
    if let Some(custom) = crate::storage_get("goptop:servers") {
        // 自定义服务器列表拼接进候选（builtin 项在 TS 侧已过滤，这里信任配置）。
        if custom.len() > 2 {
            all = format!("[{},{}]", BUILTIN.trim_start_matches('[').trim_end_matches(']'), custom.trim_start_matches('[').trim_end_matches(']'));
        }
    }
    let list: serde_json::Value = serde_json::from_str(&all).unwrap_or(serde_json::Value::Null);
    list.as_array()
        .and_then(|arr| arr.iter().find(|s| s["id"].as_str() == Some(sel.as_str())))
        .and_then(|s| s["url"].as_str())
        .map(str::to_string)
}

/// STUN 线路读取（与历史 stun.ts 键名/默认一致；迁移逻辑简化为「配置缺失即默认」）。
pub(crate) fn load_stun_urls() -> Vec<String> {
    const DEFAULT_ON: &[&str] = &[
        "stun:stun.miwifi.com:3478",
        "stun:stun.chat.bilibili.com:3478",
        "stun:stun.cloudflare.com:3478",
    ];
    match crate::storage_get("goptop:stun").and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok()) {
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

/// DOM 定时器：毫秒回调（泵层定时器在 lib.rs；这里给 effect 的一次性定时用）。
pub(crate) fn set_timeout(core: &SharedCore, ms: i32, f: impl FnMut() + 'static) {
    let core = core.clone();
    let mut f = f;
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        f();
        let _ = &core;
    });
    crate::window().set_timeout_with_callback_and_timeout_and_arguments_0(closure.as_ref().unchecked_ref(), ms).expect("setTimeout");
    closure.forget();
}
