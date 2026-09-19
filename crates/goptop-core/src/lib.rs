#![allow(missing_docs)]
//! goptop-core — P2P 围棋/五子棋的唯一真源（Single Source of Truth）。
//!
//! - `board`  — 棋盘、棋子、坐标、尺寸相关的纯数据与工具。
//! - `gomoku` — 五子棋规则（五连判定等）。
//! - `go`     — 围棋规则（落子、提子、气等，最小可玩子集先行）。
//! - `game`   — 统一的 `GameState`/`GameKind`/`Move` 状态机，屏蔽 Gomoku/Go 差异。
//! - `protocol` — 早期参考实现，**已被 `crates/goptop-net/src/protocol.rs` 取代**
//!   （GameMsg/MsgKind 线格式的唯一真源在那里，字段名对齐历史 TS 线格式；
//!   本模块除本文件的 re-export 外无消费者）。
//! - `wasm`   — 仅在 `feature = "wasm"` 时编译，为前端暴露 `WasmGame` 绑定。
//!   **已接线（2026-09-13 起）**：Web 与 Tauri WebView 的所有落子/悔棋判定都
//!   经 `frontend/src/game/rules.ts` 走本模块；改规则后重跑 scripts/build-wasm.sh
//!   并提交 frontend/src/wasm/ 产物。
//!
//! 设计约束：
//! - 本 crate 不依赖任何平台/网络/前端库，可在原生、Tauri、WASM 三端复用。
//! - 五子棋与围棋的棋盘/棋子在数据层完全一致，差异仅在规则层。

pub mod board;
pub mod game;
pub mod go;
pub mod gomoku;
pub mod protocol;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use board::{Board, Coord, Stone};
pub use game::{GameKind, GameState, Move, RuleError, RuleSet};
pub use protocol::GameMsg;
