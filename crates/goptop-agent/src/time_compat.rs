//! 时间与随机的平台双臂 —— wasm32 上「禁 tokio time/net」的落点（阶段⑤契约 §1.3/§3.2）。
//!
//! 三个原语，两端各一份实现、调用点零 cfg：
//! - [`delay`]：native=`tokio::time::sleep`；wasm=`setTimeout` 的 Promise（由
//!   `wasm-bindgen-futures::spawn_local` 的微任务执行器驱动）。tokio `time` 在
//!   wasm 上是运行时 panic（`Instant::now` 即 trap，tokio 自家 time_wasm 测试钉死），
//!   所以循环/工具/配对的全部 sleep 都必须走这里。
//! - [`now_ms`]：native=transport-native 的进程级口径；wasm=`js_sys::Date::now()`。
//!   非单调（系统回拨影响 deadline）是已知且接受的取舍（契约 R-w2：40s 配对 /
//!   600ms 定拍量级容忍秒级回拨，不引 performance.now 省一个 web-sys feature 面）。
//! - [`rand_u32`]：B 席临时 userId 的熵源。native=`rand4()[0]`（transport 既有）；
//!   wasm=crypto.getRandomValues（[`crate::wasm`] 同款）。

/// 睡一拍。**wasm 上绝不许出现 `tokio::time::sleep`**——编译期在白名单外直接
/// compile_error，运行期则是 panic=abort 的整页 trap；全部走本函数。
pub(crate) async fn delay(ms: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    #[cfg(target_arch = "wasm32")]
    crate::wasm::sleep(ms).await;
}

/// 当前 Unix 毫秒（配对 deadline / 日志时间戳用）。
#[must_use]
pub(crate) fn now_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    return goptop_transport_native::now_ms();
    #[cfg(target_arch = "wasm32")]
    return crate::wasm::now_ms();
}

/// 单个 u32 随机数（HookHost 临时 userId 的熵源；同毫秒防撞车靠它与
/// `gen_user_id` 的进程级自增）。
#[must_use]
pub(crate) fn rand_u32() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    return goptop_transport_native::rand4()[0];
    #[cfg(target_arch = "wasm32")]
    return crate::wasm::rand_u32();
}
