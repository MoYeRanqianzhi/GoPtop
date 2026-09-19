//! Effect 执行器 — 状态机的 IO 意图在这里落地（WS 上行 / BC 广播 / RTC 操作 /
//! 存储 / 导航 / 定时器 / 剪贴板）。

use crate::io::{self, bc, rtc, ws};
use crate::{queue_event, Core};
use goptop_net::session::{Effect, Event};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// JS 侧变更通知钩子（App 挂载时设 window.goptopOnChange = () => setState(...)）。
pub(crate) fn notify_change() {
    let f = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("goptopOnChange")).ok();
    if let Some(f) = f {
        if f.is_function() {
            let _ = js_sys::Function::from(f).call0(&JsValue::NULL);
        }
    }
}

/// 执行一批 Effect。
pub(crate) fn run_effects(core: &Rc<RefCell<Core>>, effects: Vec<Effect>) {
    for e in effects {
        run_effect(core, e);
    }
}

fn run_effect(core: &Rc<RefCell<Core>>, e: Effect) {
    match e {
        Effect::Emit => notify_change(),
        Effect::Notice(text, ms) => {
            // 提示条本体在快照里没有——历史上 notice 是 React 状态；这里以
            // window.goptopNotice(text, ms) 钩子透出（App 侧转 React state）。
            let arg = serde_json::json!({ "text": text, "ms": ms });
            call_hook("goptopNotice", &arg);
        }
        Effect::Nav(path) => {
            // SPA 导航：pushState + popstate 事件（对齐 net/links nav）。
            let w = crate::window();
            let _ = w.history().map(|h| h.push_state_with_url(&JsValue::NULL, "", Some(&path)));
            let _ = w.dispatch_event(&web_sys::Event::new("popstate").unwrap());
        }
        Effect::SendServer(v) => {
            let socket = core.borrow().ws.clone();
            if let Some(ws) = socket {
                let connected = ws.is_open();
                if connected {
                    ws.send(&v);
                }
            }
        }
        Effect::ServerConnect(url) => {
            let socket = ws::ServerSocket::open(core.clone(), url);
            core.borrow_mut().ws = Some(Rc::new(socket));
        }
        Effect::ServerClose => {
            if let Some(w) = core.borrow_mut().ws.take() {
                w.close_manual();
            }
        }
        Effect::SendPresence(v) => {
            let t = v["t"].as_str().unwrap_or("").to_string();
            let bc = core.borrow().presence_bc.clone();
            if let Some(bc) = bc {
                if t == "announce" {
                    // 无 2s 重放循环：announce 只由状态机在阶段切换时发一次，且走 SendServer
                    // 信令侧（不经本通道），这里只做转发。
                    core.borrow_mut().session.server_url = core.borrow().session.server_url.clone();
                }
                bc.post(&v);
            }
        }
        Effect::Broadcast(msg) => {
            // 三链路：BC + 全部 open 的 DC + 服务器 relay 兜底。
            let json = serde_json::to_string(&msg).unwrap_or_default();
            let core_ref = core.borrow();
            if let Some(bc) = &core_ref.game_bc {
                bc.post(&serde_json::json!(msg));
            }
            let _ = json.clone();
            for (tag, peer) in &core_ref.peers {
                let _ = tag;
                peer.send(&json);
            }
            if core_ref.session.server_mode && core_ref.session.server_state == "ready" {
                if let Some(ws) = &core_ref.ws {
                    if ws.is_open() {
                        for to in &core_ref.session.relay_targets {
                            ws.send(&serde_json::json!({ "t": "relay", "to": to, "payload": msg }));
                        }
                    }
                }
            }
        }
        Effect::CreatePeer { tag, inviter, spectator: _ } => {
            let stun = core.borrow().session.stun_urls.clone();
            let peer = rtc::RtcPeer::new(core, tag.clone(), inviter, &stun);
            let rtc_ready_tag = tag.clone();
            // offer 生成（inviter 方向）：完成后回喂 RtcReady。
            if inviter {
                let core2 = core.clone();
                let server_mode = core.borrow().session.server_mode;
                peer.create_offer(Box::new(move |result| {
                    let (offer_plain, offer_enc) = match result {
                        Ok((sdp, typ)) => {
                            let payload = serde_json::json!({ "s": sdp, "t": typ, "r": "player" }).to_string();
                            let enc = if server_mode {
                                None
                            } else {
                                // 无服务器：pwd 混淆编码（当前局 pwd；观战 offer 由
                                // 状态机以 specPwd 二次编码后进链接）。
                                core2
                                    .borrow()
                                    .session
                                    .pwd
                                    .as_deref()
                                    .and_then(|p| goptop_net::codec::encode(&payload, p).ok())
                            };
                            (Some(payload), enc)
                        }
                        Err(_) => (None, None),
                    };
                    queue_event(Event::RtcReady { tag: rtc_ready_tag, offer_plain, answer_plain: None, offer_enc, answer_enc: None });
                }));
            }
            core.borrow_mut().peers.retain(|(t, _)| *t != tag);
            core.borrow_mut().peers.push((tag, peer));
        }
        Effect::FeedOffer { tag, offer, encrypted } => {
            let (sdp, typ, _r) = decode_sdp(&offer, encrypted, core);
            if let Some((sdp, typ)) = sdp.zip(typ) {
                if let Some((_, peer)) = core.borrow().peers.iter().find(|(t, _)| *t == tag) {
                    let core2 = core.clone();
                    let role_spectator = core.borrow().session.role == goptop_net::session::Role::Spectator;
                    let tag2 = tag.clone();
                    peer.accept_offer(&sdp, &typ, Box::new(move |result| {
                        let (ans_plain, ans_enc) = match result {
                            Ok((sdp, typ)) => {
                                let payload = serde_json::json!({ "s": sdp, "t": typ, "r": if role_spectator { "spectator" } else { "player" } }).to_string();
                                let enc = if encrypted {
                                    let pwd = core2.borrow().session.pwd.clone().or_else(|| core2.borrow().session.spec_pwd.clone());
                                    pwd.as_deref().and_then(|p| goptop_net::codec::encode(&payload, p).ok())
                                } else {
                                    None
                                };
                                (Some(payload), enc)
                            }
                            Err(_) => (None, None),
                        };
                        queue_event(Event::RtcReady { tag: tag2, offer_plain: None, answer_plain: ans_plain, offer_enc: None, answer_enc: ans_enc });
                    }));
                }
            }
        }
        Effect::RenamePeer { from, to } => {
            let mut core = core.borrow_mut();
            if let Some(pos) = core.peers.iter().position(|(t, _)| *t == from) {
                let entry = core.peers.remove(pos);
                core.peers.push((to, entry.1));
            }
        }
        Effect::AcceptAnswer { tag, answer, encrypted } => {
            let (sdp, typ, _) = decode_sdp(&answer, encrypted, core);
            if let (Some(sdp), Some(typ)) = (sdp, typ) {
                if let Some((_, peer)) = core.borrow().peers.iter().find(|(t, _)| *t == tag) {
                    peer.accept_answer(&sdp, &typ);
                }
            }
        }
        Effect::ClosePeers => {
            let peers: Vec<(String, rtc::RtcPeer)> = core.borrow_mut().peers.drain(..).collect();
            for (_, p) in peers {
                p.close();
            }
            if let Some(bc) = core.borrow_mut().game_bc.take() {
                bc.close();
            }
        }
        Effect::JoinChannel(gid) => {
            let core2 = core.clone();
            let name = format!("goptop-game-{gid}");
            let bc = bc::Bc::open(&name, move |data: String| {
                if let Ok(msg) = serde_json::from_str::<goptop_net::protocol::GameMsg>(&data) {
                    queue_event(Event::Net(msg));
                }
                let _ = &core2;
            });
            core.borrow_mut().game_bc = Some(Rc::new(bc));
        }
        Effect::LeaveChannel => {
            if let Some(bc) = core.borrow_mut().game_bc.take() {
                bc.close();
            }
        }
        Effect::Timer { id, ms } => {
            io::set_timeout(core, ms as i32, move || {
                queue_event(Event::Timer(id));
            });
        }
        Effect::SetStorage { key, value } => crate::storage_set(&key, value.as_deref()),
        Effect::Copy { text, ok_msg } => {
            let arg = serde_json::json!({ "text": text, "ok": ok_msg });
            call_hook("goptopCopy", &arg);
        }
    }
}

/// 解出 SDP 对：明文 JSON（服务器模式）或 G1 token（无服务器，pwd 解码）。
fn decode_sdp(payload: &str, encrypted: bool, core: &Rc<RefCell<Core>>) -> (Option<String>, Option<String>, Option<String>) {
    if encrypted {
        let pwd = {
            let c = core.borrow();
            c.session.pwd.clone().or_else(|| c.session.spec_pwd.clone()).unwrap_or_default()
        };
        match goptop_net::codec::decode(payload, &pwd).ok().and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok()) {
            Some(v) => (
                v["s"].as_str().map(str::to_string),
                v["t"].as_str().map(str::to_string),
                v["r"].as_str().map(str::to_string),
            ),
            None => (None, None, None),
        }
    } else {
        match serde_json::from_str::<serde_json::Value>(payload).ok() {
            Some(v) => (
                v["s"].as_str().map(str::to_string),
                v["t"].as_str().map(str::to_string),
                v["r"].as_str().map(str::to_string),
            ),
            None => (None, None, None),
        }
    }
}

fn call_hook(name: &str, arg: &serde_json::Value) {
    if let Ok(f) = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name)) {
        if f.is_function() {
            let _ = js_sys::Function::from(f).call1(&JsValue::NULL, &JsValue::from_str(&arg.to_string()));
        }
    }
}
