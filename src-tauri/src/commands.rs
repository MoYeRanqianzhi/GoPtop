//! Tauri commands — 前端 `invoke` 的 Rust 侧实现。
//! 当前仅保留 `greet` 作为 Phase 0 探针，Phase 3 起扩展为房间与对弈指令。

/// 模板探针：验证 `frontend ↔ src-tauri` invoke 链路通畅。
#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}
