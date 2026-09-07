//! P2P 占位 — 空模块，仅保证 `mod p2p` 编译通过。
//!
//! 早期规划在此持有 iroh Endpoint，该路线已弃：现行 P2P 全部在前端 TS
//! （WebRTC STUN-only 直连，见 .agents/docs/p2p-protocol.md），Tauri 壳只承载窗口。
//! 若未来 Rust 侧需要参与传输，再在此落地。
