//! Tauri commands — 前端 `invoke` 的 Rust 侧实现。
//! 当前仅 `greet` 探针，且前端尚未调用它（grep 证实零引用）；P2P/房间逻辑全在
//! crates/goptop-net + crates/goptop-transport（wasm），Tauri 壳只承载窗口。

/// 模板探针：验证 `frontend ↔ src-tauri` invoke 链路通畅（未被前端调用）。
#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}
