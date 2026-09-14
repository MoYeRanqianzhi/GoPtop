//! BroadcastChannel 封装 — 同源信令（presence / 对局数据 channel）。

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

pub(crate) struct Bc {
    inner: web_sys::BroadcastChannel,
    _onmsg: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Bc {
    /// 打开 channel 并挂消息回调（JSON 文本）。
    pub fn open(name: &str, on_message: impl Fn(String) + 'static) -> Bc {
        let inner = web_sys::BroadcastChannel::new(name).expect("BroadcastChannel");
        let cb = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            if let Some(txt) = ev.data().as_string() {
                on_message(txt);
            }
        });
        inner.set_onmessage(Some(cb.as_ref().unchecked_ref()));
        Bc { inner, _onmsg: cb }
    }

    /// 发送 JSON（序列化失败静默忽略——BC 本地直传，失败只影响本页）。
    pub fn post(&self, v: &serde_json::Value) {
        let _ = self.inner.post_message(&JsValue::from_str(&v.to_string()));
    }

    pub fn close(&self) {
        let _ = self.inner.close();
    }
}
