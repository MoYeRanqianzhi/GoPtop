//! WASM 绑定 — 仅在 `feature = "wasm"` 时编译。
//!
//! **这只是 Web 端的宿主封装**：契约逻辑全在平台无关的 [`crate::json_api`] 里，
//! 桌面/Android 端经 Tauri command 调用同一套（见 `src-tauri/src/rules.rs`）。
//! 两边必须逐字一致，否则同一份前端代码会在不同宿主上静默分叉——不报错，
//! 只是行为不同。
//!
//! 数据面约定与 `json_api` 相同（见该模块注释）；所有方法返回 JSON 字符串
//! （wasm-bindgen 跨界传结构体成本高，且前端已有 JSON 解析习惯）。

use wasm_bindgen::prelude::*;

use crate::game::GameState;
use crate::json_api;

/// 行为化绑定：一个实例持一局 `GameState`。前端每个对局页面/本地页各持一个。
#[wasm_bindgen]
pub struct WasmGame {
    state: GameState,
}

#[wasm_bindgen]
impl WasmGame {
    /// 创建对局（静态工厂；wasm-bindgen 不允许构造函数返回 Option）。
    /// 非法尺寸组合返回 `None` 而非触达核心层断言——release 下 panic="abort"，
    /// 断言即 wasm trap（白屏），边界必须自己挡（2026-09-14 审查 #5 P2-3）。
    pub fn new_game(kind_json: &str) -> Option<WasmGame> {
        json_api::new_game(kind_json).map(|state| WasmGame { state })
    }

    /// 当前逻辑尺寸。
    #[wasm_bindgen(js_name = boardSize)]
    pub fn board_size(&self) -> u8 {
        self.state.kind.size() as u8
    }

    /// 当前局面的完整序列化（`goptop-ai` 的分析输入）。
    pub fn state_json(&self) -> String {
        json_api::state_json(&self.state)
    }

    /// 落子（唯一规则入口）。
    pub fn try_place(&mut self, x: u8, y: u8) -> String {
        json_api::try_place(&mut self.state, x, y)
    }

    /// 停一手。
    pub fn pass(&mut self) -> String {
        json_api::pass(&mut self.state)
    }

    /// 终局区域计分（中国规则数子法）。
    pub fn score(&self, dead_json: &str) -> String {
        json_api::score(&self.state, dead_json)
    }

    /// 撤销最后一手。
    pub fn undo_last(&mut self) -> String {
        json_api::undo_last(&mut self.state)
    }

    /// 重开（同种类）。
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// 采纳全量快照（SyncState）。
    pub fn adopt(&mut self, board_json: &str, to_move: &str, winner: &str, history_json: &str) -> bool {
        json_api::adopt(&mut self.state, board_json, to_move, winner, history_json)
    }
}
