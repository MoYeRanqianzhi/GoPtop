//! WebSocket 服务器通道 — hello/心跳 25s/固定 8s 间隔重连/下行事件。

use super::SharedCore;
use std::rc::Rc;
use crate::queue_event;
use goptop_net::session::{Event, ServerEvt};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub(crate) struct ServerSocket {
    url: String,
    inner: web_sys::WebSocket,
    /// 手动关闭：本意不重连；但 manual_close 无写入点，当前实现仍会重连（见 onclose 分支）。
    manual_close: bool,
    // 闭包持有（防 drop 失效）。
    _closures: Vec<Closure<dyn FnMut(JsValue)>>,
    timers: Vec<i32>,
}

impl ServerSocket {
    /// 打开连接：onopen 发 hello + 启动心跳；onmessage 解析为 ServerEvt；
    /// onclose 后按固定 8s 间隔重连（retry 恒为 3，非指数退避——见该分支内的说明）。
    pub fn open(core: SharedCore, url: String) -> ServerSocket {
        let ws = web_sys::WebSocket::new(&url).expect("WebSocket::new");
        let mut closures: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();
        let timers: Vec<i32> = Vec::new();

        // —— onopen：hello（camelCase 字段，服务器 rename_all_fields）+ 心跳 ——
        {
            let core = core.clone();
            let ws_for_ping = ws.clone();
            let cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
                {
                    let c = core.borrow();
                    let hello = serde_json::json!({
                        "t": "hello", "name": c.session.name, "userId": c.session.user_id,
                    });
                    let _ = ws_for_ping.send_with_str(&hello.to_string());
                }
                let ws2 = ws_for_ping.clone();
                let ping = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
                    let _ = ws2.send_with_str(r#"{"t":"ping"}"#);
                });
                let id = crate::window().set_interval_with_callback_and_timeout_and_arguments_0(ping.as_ref().unchecked_ref(), 25_000).unwrap_or(0);
                ping.forget();
                let _ = id;
                queue_event(Event::Server(ServerEvt::State { s: "connecting".into(), detail: None }));
            });
            ws.set_onopen(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        // —— onmessage：welcome/peers/signal/relayed/error ——
        {
            let _core = core.clone();
            let cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
                let Ok(txt) = ev.dyn_into::<web_sys::MessageEvent>() else { return };
                let Some(data) = txt.data().as_string() else { return };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else { return };
                let t = v["t"].as_str().unwrap_or("").to_string();
                match t.as_str() {
                    "welcome" => {
                        queue_event(Event::Server(ServerEvt::State { s: "ready".into(), detail: None }));
                    }
                    "pong" => {}
                    "peers" => {
                        let users = v["users"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .map(|u| goptop_net::session::PeerInfo {
                                        id: u["id"].as_str().unwrap_or("").into(),
                                        name: u["name"].as_str().unwrap_or("").into(),
                                        status: u["status"].as_str().unwrap_or("idle").into(),
                                        game_id: u["gameId"].as_str().map(str::to_string),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        queue_event(Event::Server(ServerEvt::Peers { users }));
                    }
                    "signal" => {
                        queue_event(Event::Server(ServerEvt::Signal {
                            from: v["from"].as_str().unwrap_or("").into(),
                            kind: v["kind"].as_str().unwrap_or("").into(),
                            payload: v["payload"].clone(),
                        }));
                    }
                    "relayed" => {
                        if let Ok(msg) = serde_json::from_value::<goptop_net::protocol::GameMsg>(v["payload"].clone()) {
                            queue_event(Event::Server(ServerEvt::Relayed { from: v["from"].as_str().unwrap_or("").into(), msg }));
                        }
                    }
                    "error" => {
                        queue_event(Event::Server(ServerEvt::Error {
                            msg: v["msg"].as_str().unwrap_or("").into(),
                            code: v["code"].as_str().map(str::to_string),
                        }));
                    }
                    _ => {}
                }
            });
            ws.set_onmessage(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        // —— onclose：重连（固定 8s，见下方 delay 计算）。manual_close 无任何写入点（close_manual
        // 收 &self、字段非 Cell），Effect::ServerClose/ServerConnect 全仓也无构造点——close_manual
        // 后仍会重连，「手动关闭不重连」尚未接通 ——
        {
            let core = core.clone();
            let url2 = url.clone();
            let cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
                let manual = core.borrow().ws.as_ref().is_some_and(|w| w.manual_close);
                if manual {
                    return;
                }
                queue_event(Event::Server(ServerEvt::State { s: "connecting".into(), detail: None }));
                let core2 = core.clone();
                let url3 = url2.clone();
                let retry = 3u32; // retry 恒为 3 → 1000*2^3 = 8s 固定间隔；min(10_000) 上限永不生效（真退避需按连接次数递增 retry，当前无计数）
                let delay = (1000 * 2u32.pow(retry.min(4))).min(10_000);
                let reconnect = Closure::<dyn FnMut()>::new(move || {
                    let socket = ServerSocket::open(core2.clone(), url3.clone());
                    core2.borrow_mut().ws = Some(Rc::new(socket));
                });
                crate::window().set_timeout_with_callback_and_timeout_and_arguments_0(reconnect.as_ref().unchecked_ref(), delay as i32).ok();
                reconnect.forget();
            });
            ws.set_onclose(Some(cb.as_ref().unchecked_ref()));
            closures.push(cb);
        }

        ServerSocket { url, inner: ws, manual_close: false, _closures: closures, timers }
    }

    /// 上行 JSON。
    pub fn send(&self, v: &serde_json::Value) -> bool {
        self.inner.send_with_str(&v.to_string()).is_ok()
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn is_open(&self) -> bool {
        self.inner.ready_state() == web_sys::WebSocket::OPEN
    }

    /// 手动关闭：只关 socket，不置 manual_close；「不触发重连」尚未生效。
    pub fn close_manual(&self) {
        let _ = self.inner.close();
    }
}
