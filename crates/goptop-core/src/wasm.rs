//! WASM 绑定 — 仅在 `feature = "wasm"` 时编译。
//!
//! 设计意图是为纯 Web 前端暴露 `GameState` 的权威规则校验。**当前前端（App.tsx）
//! 使用 TS 内联规则实现，本模块尚未接线**；接入前两者并存，规则语义须保持一致。

use wasm_bindgen::prelude::*;

use crate::board::Coord;
use crate::game::{GameKind, GameState, Move};

/// 供 JS 调用的薄封装：创建对局并返回 JSON 序列化的 `GameState`。
#[wasm_bindgen]
pub fn wasm_new_game(kind_json: &str) -> String {
    let kind: GameKind =
        serde_json::from_str(kind_json).unwrap_or(GameKind::Gomoku { size: 15 });
    let state = GameState::new(kind);
    serde_json::to_string(&state).unwrap_or_else(|_| "{}".to_string())
}

/// 尝试落子，返回 JSON：`{ state, effect }` 或 `{ error }`。
#[wasm_bindgen]
pub fn wasm_try_play(state_json: &str, mv_json: &str) -> String {
    let mut state: GameState = match serde_json::from_str(state_json) {
        Ok(s) => s,
        Err(e) => return serde_json::json!({ "error": e.to_string() }).to_string(),
    };
    let mv: Move = match serde_json::from_str(mv_json) {
        Ok(m) => m,
        Err(e) => return serde_json::json!({ "error": e.to_string() }).to_string(),
    };
    // Coord 在 JSON 中为 { x, y }，与 TS 侧一致。
    match state.try_play(mv) {
        Ok(effect) => serde_json::json!({ "state": state, "effect": effect }).to_string(),
        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
    }
}

/// 便捷：构造 `Place` 操作的 JSON，供 TS 侧直接传给 `wasm_try_play`。
#[wasm_bindgen]
pub fn wasm_move_place(x: u8, y: u8) -> String {
    serde_json::to_string(&Move::Place(Coord::new(x, y))).unwrap_or_else(|_| "{}".to_string())
}
