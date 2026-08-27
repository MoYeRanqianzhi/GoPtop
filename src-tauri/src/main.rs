#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri 入口，转发到 lib::run，保持与官方模板结构一致，便于后续移动端入口复用。

fn main() {
    goptop_lib::run()
}
