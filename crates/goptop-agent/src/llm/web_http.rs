//! [`HttpChannel`] 的 wasm 实现 —— 浏览器 `fetch`（阶段⑤契约 §3.4）。
//!
//! 通道不做任何协议解释：POST JSON、原样回 `(状态码, 体)`，分类判断归适配器
//!（见 [`super`] 模块注）——与 native 的 reqwest 通道同一分工。受端点 CORS 约束
//! （浏览器同源策略），这是 web 内置模式的部署前提，计划 AgentPage 节已注明。
//!
//! **60s 超时 = fetch 与 `delay(60_000)` 竞速**：竞速败侧的 future 被 drop，但
//! drop 不会 abort 底层 fetch，连接自然终结（契约 R-w3：60s 一次、退避重试 2 次
//! 封顶，量级无害；不引 AbortController 免加 web-sys feature 面）。非 2xx 原样
//! 回 `(状态码, 体)`——`post_with_retry`/`classify_status` 两端共享零改动。

use std::task::Poll;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use super::HttpChannel;
use crate::time_compat::delay;

/// 单次请求超时（与 native_http 的 REQUEST_TIMEOUT 同值：非流式 LLM 回复的正常
/// 上界；更长的生成属于配置失误——max_output_tokens 该调小）。
const REQUEST_TIMEOUT_MS: u64 = 60_000;

/// fetch 通道壳（无状态：每次请求现取 `window`，连接池归浏览器）。
#[derive(Default)]
pub struct WebHttpChannel;

impl WebHttpChannel {
    /// 建客户端（native 版要装 TLS provider，web 版无装配动作；保留构造器让
    /// 装配层两端同形——`WebHttpChannel::new()` 与 `NativeHttp::new()` 调用点一致）。
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

// ?Send 界的取舍见 trait 侧注（llm/mod.rs HttpChannel）。
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl HttpChannel for WebHttpChannel {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String), String> {
        let window = web_sys::window().ok_or("http post failed: no global window")?;
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        // 头装成普通对象（Headers/对象两可；对象最少 feature 面）。Content-Type
        // 与 native 通道同款显式补齐（适配器给的鉴权头之外的那一个）。
        let header_obj = js_sys::Object::new();
        for (k, v) in headers {
            js_sys::Reflect::set(&header_obj, &JsValue::from_str(k), &JsValue::from_str(v))
                .map_err(|e| format!("http post failed: bad header {k:?}: {e:?}"))?;
        }
        js_sys::Reflect::set(
            &header_obj,
            &JsValue::from_str("Content-Type"),
            &JsValue::from_str("application/json"),
        )
        .map_err(|e| format!("http post failed: bad content-type: {e:?}"))?;
        init.set_headers(&header_obj);
        init.set_body(&JsValue::from_str(&body));

        let promise = window.fetch_with_str_and_init(url, &init);
        // fetch 与超时竞速（poll_fn 手写 select——wasm 不引 tokio macros）。竞速败侧
        // 的 future 被 drop（不 abort 底层 fetch，连接自然终结——契约 R-w3 接受）。
        let resp_value: JsValue = {
            let mut fetch = Box::pin(wasm_bindgen_futures::JsFuture::from(promise));
            let mut timer = Box::pin(delay(REQUEST_TIMEOUT_MS));
            std::future::poll_fn(|cx| {
                if let Poll::Ready(v) = fetch.as_mut().poll(cx) {
                    // v: Result<JsValue, JsValue>——fetch 的 resolve/reject 原样透传。
                    return Poll::Ready(Ok(v));
                }
                if timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(format!(
                        "http post failed: timeout after {}s",
                        REQUEST_TIMEOUT_MS / 1000
                    )));
                }
                Poll::Pending
            })
            .await
            .map_err(|e| format!("http post failed: {e:?}"))? // 超时臂
            .map_err(|e| format!("http post failed: {e:?}"))? // fetch 拒绝臂
        };
        let resp: web_sys::Response = resp_value
            .dyn_into()
            .map_err(|_| "http post failed: response is not a Response".to_string())?;
        // 非 2xx 不算 Err：状态码+体原样带回，分类是适配器的事（与 native 同一口径）。
        let status = resp.status();
        let text_promise = resp.text().map_err(|e| format!("http body read failed: {e:?}"))?;
        let text = wasm_bindgen_futures::JsFuture::from(text_promise)
            .await
            .map_err(|e| format!("http body read failed: {e:?}"))?
            .as_string()
            .ok_or("http body read failed: body is not text")?;
        Ok((status, text))
    }
}
