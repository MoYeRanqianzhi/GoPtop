//! GoPtop Tauri backend — 窗口与命令的粘合层。
//!
//! - `commands` 暴露给前端的 `invoke` 接口（当前只有 `greet` 模板探针）。
//! - `p2p` 为预留空模块：早期 iroh 路线已弃，现行 P2P 在 crates/goptop-net + crates/goptop-transport
//!   （wasm），前端 TS 只做 UI 绑定。
//! - `store` 平台本地存储：桌面 `~/.goptop/store.json`，移动端平台私有数据目录
//!   （Web 端不走这里，前端门面直接用 localStorage）。

mod commands;
mod p2p;
mod store;
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
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            titlebar::snap_overlay_set_rect,
            store::store_load,
            store::store_set,
            store::store_remove
        ])
        .run(tauri::generate_context!())
        .expect("error while running GoPtop tauri application");
}
