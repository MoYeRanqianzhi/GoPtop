//! P2P 占位 — 空模块，仅保证 `mod p2p` 编译通过。
//!
//! 早期规划在此持有 iroh Endpoint，该路线已弃：现行 P2P 在 crates/goptop-net +
//! crates/goptop-transport（编译为 frontend/src/wasm/transport 的 wasm，前端 TS 只做 UI
//! 绑定），Tauri 壳只承载窗口。
//! 若未来 Rust 侧需要参与传输，再在此落地。
