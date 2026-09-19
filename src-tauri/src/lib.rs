//! GoPtop Tauri backend — 窗口与命令的粘合层，外加**核心逻辑的原生宿主**。
//!
//! - `rules`：规则引擎。桌面与 Android 直接跑 native Rust，契约逻辑与 Web 端的
//!   wasm 绑定共用 `goptop_core::json_api` 同一份实现。
//! - `ai`：AI 引擎（五子棋 NNUE + 围棋 MCTS）。同样原生执行——noru 在 wasm 上
//!   只有标量路径，原生能吃 NEON/AVX2。
//! - `session`：P2P 会话（传输层状态机 + WebRTC/WS）。**原生平台不在 WebView 里跑
//!   传输层的 wasm**——同一个 `goptop-transport-native` 直接链接进来。
//! - `store`：平台本地存储。桌面 `~/.goptop/store.json`，移动端平台私有数据目录
//!   （Web 端不走这里，前端门面直接用 localStorage）。
//! - `titlebar`：Windows Snap Layouts 透明覆盖层。
//! - `p2p` 为预留空模块：早期 iroh 路线已弃。
//!
//! **架构口径（2026-09-19 修正）**：`wasm 只是 Web 端的编译目标`——浏览器跑不了
//! 原生代码，所以 Web 必须编 wasm。但桌面/Android 有完整的 Rust native 宿主
//! （`goptop.exe` / `libgoptop_lib.so`），没有理由绕道 WebView 去跑 wasm：
//! 那既丢 SIMD，又多一层无谓的边界。原生平台一律直接链接 crate。

mod ai;
mod commands;
mod p2p;
mod rules;
mod session;
mod store;
mod titlebar;

/// 供 `src-tauri/src/main.rs` 调用的入口，保持与官方模板一致的 `run()` 签名。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(rules::Games::default())
        .manage(session::Sessions::default())
        .setup(|app| {
            titlebar::init(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            titlebar::snap_overlay_set_rect,
            store::store_load,
            store::store_set,
            store::store_remove,
            // —— 规则引擎（原生） ——
            rules::game_new,
            rules::game_drop,
            rules::game_state_json,
            rules::game_place,
            rules::game_pass,
            rules::game_undo,
            rules::game_reset,
            rules::game_score,
            rules::game_adopt,
            rules::game_board_size,
            // —— AI 引擎（原生） ——
            ai::ai_analyze,
            ai::ai_warmup,
            // —— P2P 会话（原生传输层） ——
            session::session_new,
            session::session_drop,
            session::session_poll,
            session::session_cmd,
            session::session_state_json,
            session::session_state_debug,
            session::session_parse_link,
            session::session_parse_answer
        ])
        .run(tauri::generate_context!())
        .expect("error while running GoPtop tauri application");
}
