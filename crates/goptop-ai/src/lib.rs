//! goptop-ai — 本地离线 AI：五子棋 α-β/NNUE 引擎、围棋 MCTS、实时胜率评估。
//!
//! 定位：与 `goptop-core`（规则真源）分离的**分析层**。规则永远由 core 判定，
//! 本 crate 只负责「下一步下哪里」与「当前形势谁的胜率多少」。
//!
//! 两条引擎线：
//! - **五子棋**：`gomoku` 模块，接 figrid-board（Gomocup 参赛引擎）的 α-β + NNUE；
//! - **围棋**：`go` 模块，自建 MCTS + RAVE，走子用 go_game_board（libEGo）的
//!   模式化 playout。
//!
//! 统一的输入输出见 [`analyze`]：一个局面 + 时间预算 → 最佳着法 + 胜率。
//! 两条线都**不阻塞调用方**的责任边界：本 crate 是同步阻塞的纯计算，隔离到
//! Web Worker 由前端负责（wasm 单线程下 1 秒搜索会冻结主线程）。

pub mod go;
pub mod gomoku;
pub mod odds;

#[cfg(feature = "wasm")]
pub mod wasm;

use goptop_core::board::Stone;
use goptop_core::game::{GameKind, GameState};
use serde::{Deserialize, Serialize};

/// 分析请求（wasm 边界与 Worker 消息共用；字段名走 camelCase 与前端一致）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeRequest {
    /// 局面（serde 形态与 goptop-core 一致，直接反序列化 GameState）。
    pub state: GameState,
    /// 视角颜色：胜率以该方为准。本地双人/观战无「我方」，由前端指定黑方。
    pub my_color: Stone,
    /// 思考预算（毫秒）。围棋按 playout 数换算，五子棋直接进 α-β 的 deadline。
    pub budget_ms: u32,
    /// 是否需要最佳着法（人机对战 true；纯胜率分析 false 可省下选点开销）。
    pub want_move: bool,
}

/// 分析结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeResult {
    /// 最佳着法；`want_move=false` 或无着可下（终局/计分态）时为 `None`。
    pub best_move: Option<(u8, u8)>,
    /// 视角方（`my_color`）的胜率，0.0..=1.0。
    pub win_rate: f64,
    /// 搜索深度（五子棋=α-β 深度；围棋=按 playout 数折算的等价信息，见各模块注释）。
    pub depth: u32,
    /// 搜索规模（五子棋=节点数；围棋=playout 数）。
    pub nodes: u64,
    /// 实际耗时（毫秒）。
    pub elapsed_ms: u64,
}

impl AnalyzeResult {
    /// 终局/无需分析时的退化结果：胜负已定，胜率只能是 0 或 1。
    pub fn decided(state: &GameState, my_color: Stone) -> Self {
        let win_rate = match state.winner {
            Some(w) if w == my_color => 1.0,
            Some(_) => 0.0,
            None => 0.5,
        };
        Self { best_move: None, win_rate, depth: 0, nodes: 0, elapsed_ms: 0 }
    }
}

/// 统一分析入口：按棋种分派到对应引擎。
///
/// 终局（`winner.is_some()`）与围棋计分态直接返回确定结果，不进搜索——这两个
/// 状态下 core 已拒绝落子，再搜索既无意义也会给出误导性胜率。
pub fn analyze(req: &AnalyzeRequest) -> AnalyzeResult {
    let st = &req.state;
    if st.winner.is_some() || st.scoring {
        return AnalyzeResult::decided(st, req.my_color);
    }
    match st.kind {
        GameKind::Gomoku { size } => gomoku::analyze(st, size, req.my_color, req.budget_ms, req.want_move),
        GameKind::Go { size } => go::analyze(st, size, req.my_color, req.budget_ms, req.want_move),
    }
}
