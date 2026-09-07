#![allow(missing_docs)]
//! goptop-core — P2P 围棋/五子棋的唯一真源（Single Source of Truth）。
//!
//! - `board`  — 棋盘、棋子、坐标、尺寸相关的纯数据与工具。
//! - `gomoku` — 五子棋规则（五连判定等）。
//! - `go`     — 围棋规则（落子、提子、气等，最小可玩子集先行）。
//! - `game`   — 统一的 `GameState`/`GameKind`/`Move` 状态机，屏蔽 Gomoku/Go 差异。
//! - `protocol` — 联机消息 `GameMsg`（serde）参考实现。**注意：现行联机协议的
//!   唯一真源是前端 `frontend/src/net/transport.ts`**（TS 线格式含 sender/userId/
//!   SyncState 等，与本模块不同）；本模块尚未接线，对接 Rust 传输前须先对齐。
//! - `wasm`   — 仅在 `feature = "wasm"` 时编译，为纯 Web 前端暴露 `wasm-bindgen` 绑定。
//!   **前端当前使用 TS 内联规则实现，WASM 尚未接线**。
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
