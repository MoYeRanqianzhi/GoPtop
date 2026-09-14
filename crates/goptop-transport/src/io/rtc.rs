//! RTCPeerConnection 封装 — offer/answer（全量 gathering）、DataChannel、状态事件。
//!
//! offer/answer 都是 gathering 完成后才产出（无 trickle ICE，与历史实现一致），
//! 完成后以 `Event::RtcReady` 回喂状态机；远端消息（GameMsg JSON）以 `Event::Net` 回喂。
//! 等待 gathering 用轮询（100ms 步进、8s 上限）——避免 Promise/Closure 生命周期纠缠。

use super::SharedCore;
use crate::queue_event;
use goptop_net::session::Event;
use std::cell::{Cell, RefCell};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub(crate) struct RtcPeer {
    pub pc: web_sys::RtcPeerConnection,
    pub dc: RefCell<Option<web_sys::RtcDataChannel>>,
    pub closed: Cell<bool>,
    // 事件回调保活（防 GC；drop 时一并释放）。
    _closures: RefCell<Vec<Closure<dyn FnMut(JsValue)>>>,
}

impl RtcPeer {
    /// 建连：inviter=true 先建 DataChannel 并生成 offer；false 等 ondatachannel。
    /// `stun_urls` 来自会话配置（仅 NAT 发现，无 TURN）。
    pub fn new(core: &SharedCore, tag: String, inviter: bool, stun_urls: &[String]) -> RtcPeer {
        let mut cfg = web_sys::RtcConfiguration::new();
        let servers: Vec<web_sys::RtcIceServer> = stun_urls
            .iter()
            .map(|u| {
                let mut s = web_sys::RtcIceServer::new();
                s.urls(&js_sys::Array::of1(&JsValue::from_str(u)));
                s
            })
            .collect();
        cfg.ice_servers(&servers.into());
        let pc = web_sys::RtcPeerConnection::new_with_configuration(&cfg).expect("RTCPeerConnection");
        let peer = RtcPeer { pc: pc.clone(), dc: RefCell::new(None), closed: Cell::new(false), _closures: RefCell::new(Vec::new()) };
        let mut closures: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();

        // ICE connectionState 变化 → open/failed 事件（disconnected 常可自愈，不报）。
        {
            let _core2 = core.clone();
            let tag2 = tag.clone();
            let pc2 = pc.clone();
            let cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
                use web_sys::RtcIceConnectionState as S;
                let (opened, closed, failed) = match pc2.ice_connection_state() {
                    S::Connected | S::Completed => (true, false, false),
                    S::Failed => (false, true, true),
                    S::Closed => (false, true, false),
                    _ => (false, false, false),
                };
                if opened || closed {
                    queue_event(Event::PeerState { tag: tag2.clone(), opened, closed, failed });
                }
            });
            pc.set_onconnectionstatechange(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        // ondatachannel：受邀方向收到对端 DC。
        {
            let core2 = core.clone();
            let tag2 = tag.clone();
            let store = peer.dc.clone();
            let cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
                let Ok(evt) = ev.dyn_into::<web_sys::RtcDataChannelEvent>() else { return };
                let ch = evt.channel();
                attach_channel_handlers(&core2, &tag2, &ch);
                *store.borrow_mut() = Some(ch);
            });
            pc.set_ondatachannel(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        // 全量 gathering 模型：候选已含在 SDP 里，onicecandidate 仅等待 null（完成信号）。
        {
            let cb = Closure::<dyn FnMut(JsValue)>::new(|_ev: JsValue| {});
            pc.set_onicecandidate(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        if inviter {
            let ch = pc.create_data_channel("goptop");
            attach_channel_handlers(core, &tag, &ch);
            *peer.dc.borrow_mut() = Some(ch);
        }

        peer._closures.borrow_mut().extend(closures);
        peer
    }

    /// 生成 offer（全量 gathering 后回调 `(sdp, type)`）。
    pub fn create_offer(&self, done: Box<dyn FnOnce(Result<(String, String), String>)>) {
        let pc = self.pc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = async {
                let promise: js_sys::Promise = pc.create_offer().into();
                let offer: web_sys::RtcSessionDescriptionInit = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| "createOffer failed".to_string())?.into();
                await_void(pc.set_local_description(&offer), "setLocalDescription failed").await?;
                wait_gathering(&pc).await;
                let desc = pc.local_description().ok_or("no local description".to_string())?;
                Ok::<(String, String), String>((desc.sdp(), "offer".into()))
            };
            match r.await {
                Ok(pair) => done(Ok(pair)),
                Err(e) => done(Err(e)),
            }
        });
    }

    /// 应用远端 offer 并生成 answer（全量 gathering 后回调 `(sdp, type)`）。
    pub fn accept_offer(&self, sdp: &str, sdp_type: &str, done: Box<dyn FnOnce(Result<(String, String), String>)>) {
        let pc = self.pc.clone();
        let sdp = sdp.to_string();
        let sdp_type = sdp_type.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let r = async {
                let mut init = web_sys::RtcSessionDescriptionInit::new(sdp_type_enum(&sdp_type));
                init.sdp(&sdp);
                await_void(pc.set_remote_description(&init), "setRemoteDescription failed").await?;
                let promise: js_sys::Promise = pc.create_answer().into();
                let answer: web_sys::RtcSessionDescriptionInit = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| "createAnswer failed".to_string())?.into();
                await_void(pc.set_local_description(&answer), "setLocalDescription failed").await?;
                wait_gathering(&pc).await;
                let desc = pc.local_description().ok_or("no local description".to_string())?;
                Ok::<(String, String), String>((desc.sdp(), "answer".into()))
            };
            match r.await {
                Ok(pair) => done(Ok(pair)),
                Err(e) => done(Err(e)),
            }
        });
    }

    /// 应用远端 answer（幂等位由状态机管理；浏览器对重复应用抛错，这里静默）。
    pub fn accept_answer(&self, sdp: &str, sdp_type: &str) {
        let pc = self.pc.clone();
        let mut init = web_sys::RtcSessionDescriptionInit::new(sdp_type_enum(sdp_type));
        init.sdp(sdp);
        let init = init;
        wasm_bindgen_futures::spawn_local(async move {
            let _ = pc.set_remote_description(&init);
        });
    }

    /// 发送 GameMsg JSON（DC open 才发）。
    pub fn send(&self, json: &str) -> bool {
        if let Some(dc) = self.dc.borrow().as_ref() {
            if dc.ready_state() == web_sys::RtcDataChannelState::Open {
                return dc.send_with_str(json).is_ok();
            }
        }
        false
    }

    pub fn close(&self) {
        if self.closed.get() {
            return;
        }
        self.closed.set(true);
        if let Some(dc) = self.dc.borrow().as_ref() {
            let _ = dc.close();
        }
        let _ = self.pc.close();
    }
}

/// SDP 类型字符串 → web-sys 枚举（web-sys 0.3.104 的 from_str 私有）。
fn sdp_type_enum(s: &str) -> web_sys::RtcSdpType {
    match s {
        "answer" => web_sys::RtcSdpType::Answer,
        "pranswer" => web_sys::RtcSdpType::Pranswer,
        "rollback" => web_sys::RtcSdpType::Rollback,
        _ => web_sys::RtcSdpType::Offer,
    }
}

/// set(X)Description 在本 web-sys 版本返回 Promise（非 Result）：await 后转错误。
async fn await_void(p: js_sys::Promise, err: &str) -> Result<(), String> {
    wasm_bindgen_futures::JsFuture::from(p).await.map(|_| ()).map_err(|_| err.to_string())
}

/// DC 事件挂接（open/close/message）。回调保活：闭包 forget（与页面同寿命——
/// 单页应用会话即页面生命周期，peer.close 后不再触发属预期）。
fn attach_channel_handlers(core: &SharedCore, tag: &str, ch: &web_sys::RtcDataChannel) {
    {
        let core2 = core.clone();
        let tag2 = tag.to_string();
        let cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
            queue_event(Event::PeerState { tag: tag2.clone(), opened: true, closed: false, failed: false });
            let _ = &core2;
        });
        ch.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
    {
        let core2 = core.clone();
        let tag2 = tag.to_string();
        let cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
            queue_event(Event::PeerState { tag: tag2.clone(), opened: false, closed: true, failed: false });
            let _ = &core2;
        });
        ch.set_onclose(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
    {
        let core2 = core.clone();
        let cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            let Ok(evt) = ev.dyn_into::<web_sys::MessageEvent>() else { return };
            let Some(txt) = evt.data().as_string() else { return };
            if let Ok(msg) = serde_json::from_str::<goptop_net::protocol::GameMsg>(&txt) {
                queue_event(Event::Net(msg));
            }
            let _ = &core2;
        });
        ch.set_onmessage(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
}

/// 等待 ICE gathering 完成（100ms 轮询、8s 上限，对齐历史 waitGathering 超时）。
async fn wait_gathering(pc: &web_sys::RtcPeerConnection) {
    let start = js_sys::Date::now();
    while pc.ice_gathering_state() != web_sys::RtcIceGatheringState::Complete && js_sys::Date::now() - start < 8_000.0 {
        sleep(100).await;
    }
}

/// setTimeout 的 Promise 化（轮询步进用）。
async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _reject| {
        let _ = crate::window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}
