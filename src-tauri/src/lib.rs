//! GoPtop Tauri backend — 窗口、命令与 P2P 的粘合层。
//!
//! - `commands` 暴露给前端的 `invoke` 接口（greet 为模板探针，后续扩展为 create_room/join/send）。
//! - `p2p` 持有 iroh Endpoint 与 GameState，通过 `app.emit("game://msg", ...)` 推给前端。

mod commands;
mod p2p;
mod titlebar;

/// 供 `src-tauri/src/main.rs` 调用的入口，保持与官方模板一致的 `run()` 签名。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            titlebar::init(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::greet, titlebar::snap_overlay_set_rect])
        .run(tauri::generate_context!())
        .expect("error while running GoPtop tauri application");
}
