//! P2P 占位 — Phase 3 将在此持有 `iroh::Endpoint` 与 `GoPtop` 的 `GameState`，
//! 并通过 `AppHandle::emit("game://msg", payload)` 将 `GameMsg` 推送到前端。
//!
//! 当前为空模块，仅保证 `mod p2p` 编译通过，避免 Phase 0 引入未就绪的 iroh 依赖。
