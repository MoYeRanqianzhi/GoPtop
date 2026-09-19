//! IO 模块（原生）—— WebSocket / WebRTC / 同源通道。
//!
//! 与 wasm 侧 `goptop-transport/src/io/` 的分工一一对应，只是实现换成 tokio 那套。

pub mod bc;
pub mod rtc;
pub mod ws;
