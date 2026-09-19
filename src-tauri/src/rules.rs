//! 规则引擎的原生宿主 —— 桌面与 Android 直接跑 native Rust。
//!
//! 契约逻辑全在 `goptop_core::json_api`（与 Web 端的 wasm 绑定共用同一份），
//! 这里只做「Tauri command ↔ 多局实例管理」的适配。
//!
//! **为什么用 id 索引而不是单例**：本地页与 P2P 页可能同时存在对局，且前端
//! 每个页面各持一个引擎实例（与 Web 端 `WasmGame` 一一对应）。id 由 `game_new`
//! 分配，用完调 `game_drop` 释放。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use goptop_core::game::GameState;
use goptop_core::json_api;
use tauri::State;

/// 多局实例表。
#[derive(Default)]
pub struct Games(pub Mutex<HashMap<u32, GameState>>);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

#[tauri::command]
pub fn game_new(games: State<'_, Games>, kind_json: String) -> Option<u32> {
    let state = json_api::new_game(&kind_json)?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    games.0.lock().ok()?.insert(id, state);
    Some(id)
}

#[tauri::command]
pub fn game_drop(games: State<'_, Games>, id: u32) {
    if let Ok(mut m) = games.0.lock() {
        m.remove(&id);
    }
}

#[tauri::command]
pub fn game_state_json(games: State<'_, Games>, id: u32) -> String {
    with_game(&games, id, json_api::state_json).unwrap_or_else(|| "null".into())
}

#[tauri::command]
pub fn game_place(games: State<'_, Games>, id: u32, x: u8, y: u8) -> String {
    with_game_mut(&games, id, |s| json_api::try_place(s, x, y))
        .unwrap_or_else(|| json_api::err_reply("no_game"))
}

#[tauri::command]
pub fn game_pass(games: State<'_, Games>, id: u32) -> String {
    with_game_mut(&games, id, json_api::pass).unwrap_or_else(|| json_api::err_reply("no_game"))
}

#[tauri::command]
pub fn game_undo(games: State<'_, Games>, id: u32) -> String {
    with_game_mut(&games, id, json_api::undo_last).unwrap_or_else(|| json_api::err_reply("no_game"))
}

#[tauri::command]
pub fn game_reset(games: State<'_, Games>, id: u32) {
    let _ = with_game_mut(&games, id, |s| {
        s.reset();
    });
}

#[tauri::command]
pub fn game_score(games: State<'_, Games>, id: u32, dead_json: String) -> String {
    with_game(&games, id, |s| json_api::score(s, &dead_json)).unwrap_or_else(|| json_api::err_reply("no_game"))
}

#[tauri::command]
pub fn game_adopt(
    games: State<'_, Games>,
    id: u32,
    board_json: String,
    to_move: String,
    winner: String,
    history_json: String,
) -> bool {
    with_game_mut(&games, id, |s| {
        json_api::adopt(s, &board_json, &to_move, &winner, &history_json)
    })
    .unwrap_or(false)
}

#[tauri::command]
pub fn game_board_size(games: State<'_, Games>, id: u32) -> u8 {
    with_game(&games, id, |s| s.kind.size() as u8).unwrap_or(0)
}

fn with_game<T>(games: &State<'_, Games>, id: u32, f: impl FnOnce(&GameState) -> T) -> Option<T> {
    let m = games.0.lock().ok()?;
    m.get(&id).map(f)
}

fn with_game_mut<T>(games: &State<'_, Games>, id: u32, f: impl FnOnce(&mut GameState) -> T) -> Option<T> {
    let mut m = games.0.lock().ok()?;
    m.get_mut(&id).map(f)
}
