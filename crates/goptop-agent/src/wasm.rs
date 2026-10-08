//! wasm32 侧的平台实现 —— B 席宿主（[`WebHost`]）与时间/随机原语的浏览器臂。
//!
//! 只在 `target_arch="wasm32"` 编译；native 臂在 [`crate::time_compat`] 与
//! transport-native。`WebHost` 是 [`PlatformHost`] 的 web 实现：B 席（Agent 的
//! 无头会话，由 goptop-transport 的 agent 出口创建）以它为宿主基座，再被
//! [`crate::player::HookHost`] 装饰（临时 userId / stun 空 / emit 推 watch）。
//! 钩子通道与 `goptop-transport` 的 `window.goptop*` 同款：缺钩子回落
//! localStorage（前端 store 门面未安装/初始化前的兜底，五行同款）。

use wasm_bindgen::prelude::*;

use crate::player::PlatformHost;

/// 当前 Unix 毫秒（`Date.now`；非单调已知并接受，见 [`crate::time_compat`]）。
#[must_use]
pub(crate) fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

/// 单个 u32 随机数（crypto.getRandomValues；真熵）。
///
/// # Panics
/// 浏览器环境缺失 `window.crypto`（正常浏览器不存在；wasm 无「降级运行」可言）。
#[must_use]
pub(crate) fn rand_u32() -> u32 {
    let window = web_sys::window().expect("no global window");
    let crypto = window.crypto().expect("no crypto");
    let mut buf = [0u8; 4];
    crypto.get_random_values_with_u8_array(&mut buf).expect("getRandomValues");
    u32::from_ne_bytes(buf)
}

/// `setTimeout` 的 Promise 化（spawn_local 微任务里的一拍；rtc 轮询同款手法）。
pub(crate) async fn sleep(ms: u64) {
    let p = js_sys::Promise::new(&mut |resolve, _reject| {
        let _ = web_sys::window()
            .expect("no global window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

/// 全局钩子取函数（`window.<name>`；未安装/非函数回 None）。
fn hook(name: &str) -> Option<js_sys::Function> {
    let f = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name)).ok()?;
    f.is_function().then(|| js_sys::Function::from(f))
}

/// 宿主存储钩子读（`window.goptopStorageGet` 由前端 store 门面安装；缺钩子回落
/// localStorage——与 goptop-transport 的 storage_get 同款五行，两端行为一致）。
fn storage_get(key: &str) -> Option<String> {
    if let Some(f) = hook("goptopStorageGet") {
        if let Ok(v) = f.call1(&JsValue::NULL, &JsValue::from_str(key)) {
            // 门面返回 null 表示「没有这个键」，返回字符串则是命中
            return v.as_string();
        }
        // 钩子抛错（平台存储在初始化前被读等）：回落 localStorage，不留空值
    }
    web_sys::window()?.local_storage().ok().flatten()?.get_item(key).ok().flatten()
}

/// 宿主存储钩子写（`None` = 删除；缺钩子回落 localStorage）。
fn storage_set(key: &str, value: Option<&str>) {
    if let Some(f) = hook("goptopStorageSet") {
        let v = match value {
            Some(v) => JsValue::from_str(v),
            None => JsValue::NULL,
        };
        if f.call2(&JsValue::NULL, &JsValue::from_str(key), &v).is_ok() {
            return;
        }
    }
    if let Ok(Some(ls)) = web_sys::window().expect("no global window").local_storage() {
        let _ = match value {
            Some(v) => ls.set_item(key, v),
            None => ls.remove_item(key),
        };
    }
}

/// B 席的宿主基座（web 臂）。六个动作全部面向 `window.goptop*` 全局钩子：
/// - storage 走 [`storage_get`]/[`storage_set`]（读侧被 [`crate::player::HookHost`]
///   拦截的 userId/stun 不经这里——那两个键在装饰层就已短路）；
/// - notice/copy 透传全局（B 的提示条与人同屏，与桌面 HookHost→TauriHost 的
///   透传同形）；
/// - **nav 恒吞**：B 是无头席，不该导航（Hub 场景下 Nav effect 若落到全局会把
///   人的页面拽走）；
/// - **emit no-op**：B 的快照推送由 [`crate::player::HookHost::emit`] 覆写推
///   watch（事件物化源头），内层无人消费——web 上没有「TauriHost 还要推前端」
///   那一层。
#[derive(Default)]
pub struct WebHost;

impl WebHost {
    /// 构造（无状态；钩子每次现取——与 transport 同款，不缓存 JS 函数引用）。
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PlatformHost for WebHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        storage_get(key)
    }

    fn storage_set(&self, key: &str, value: Option<&str>) {
        storage_set(key, value);
    }

    fn copy(&self, text: &str) {
        // B 席实际不触发复制（无 UI）；钩子在则透传，形状与 transport bridge 的
        // goptopCopy 调用同款（ok 置空——复制成功与否的提示条归 UI 侧）。
        if let Some(f) = hook("goptopCopy") {
            let arg = serde_json::json!({ "text": text, "ok": null });
            let _ = f.call1(&JsValue::NULL, &JsValue::from_str(&arg.to_string()));
        }
    }

    fn notice(&self, text: Option<&str>, ms: Option<u32>) {
        let Some(f) = hook("goptopNotice") else { return };
        let arg = serde_json::json!({ "text": text, "ms": ms });
        let _ = f.call1(&JsValue::NULL, &JsValue::from_str(&arg.to_string()));
    }

    fn nav(&self, _path: &str) {
        // 恒吞：B 不导航（见类型注）。
    }

    fn emit(&self, _snapshot_json: &str) {
        // no-op：B 的 emit 在 HookHost 覆写层推 watch，内层无人消费（见类型注）。
    }
}
