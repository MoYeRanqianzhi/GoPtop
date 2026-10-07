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
use goptop_core::game::{GameKind, GameState, Move};
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

impl AnalyzeRequest {
    /// 宿主边界校验。serde 对 GameState 零校验：`kind` 与 history 坐标都是自由值，
    /// 而下游围棋的 `Board::with_size` 对 0/19+ 尺寸是 assert、figrid 的 zobrist
    /// 表按 15 路索引——workspace 的 panic="abort" 会让这类 panic 跨鸿蒙 FFI /
    /// Tauri 命令直接掀翻进程。入口先过这里，把失败变成明确的错误文案。
    ///
    /// # Errors
    /// `kind` 非法（五子棋仅 15，围棋仅 9/13/19），或 history 坐标越出逻辑盘。
    pub fn validate(&self) -> Result<(), String> {
        let st = &self.state;
        if !st.kind.is_valid() {
            return Err(format!("invalid kind: {:?}（五子棋仅 15，围棋仅 9/13/19）", st.kind));
        }
        let size = st.kind.size() as u8;
        for mv in &st.history {
            if let Move::Place(c) = mv
                && (c.x >= size || c.y >= size)
            {
                return Err(format!("history 坐标 ({},{}) 越出 {} 路盘", c.x, c.y, size));
            }
        }
        Ok(())
    }
}

/// 分析结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeResult {
    /// 最佳着法；`want_move=false` 或无着可下（终局/计分态）时为 `None`。
    pub best_move: Option<(u8, u8)>,
    /// 视角方（`my_color`）的胜率，0.0..=1.0。
    pub win_rate: f64,
    /// 搜索深度。五子棋=α-β 搜到的深度；围棋=MCTS 树展开到的最大层数
    /// （见 `go::Search::max_depth`），空盘上只有 2~3 层——围棋树很宽，同预算下
    /// 铺开比扎深划算，不要把这两个数放在一起比较。
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
    // 非法请求在这里降级成中性结果而不是 panic：ohos dispatch 与 Tauri 命令拿到的
    // 返回类型是 AnalyzeResult，表达不了错误，而 panic 跨 FFI 是整个进程 abort。
    // 要拿到明确的错误文案走 [`AnalyzeRequest::validate`]——wasm 边界的
    // `analyze_json` 先校验再分析，非法请求回 `{"error":...}`。
    if req.validate().is_err() {
        return AnalyzeResult {
            best_move: None,
            win_rate: 0.5,
            depth: 0,
            nodes: 0,
            elapsed_ms: 0,
        };
    }
    let st = &req.state;
    if st.winner.is_some() || st.scoring {
        return AnalyzeResult::decided(st, req.my_color);
    }
    match st.kind {
        GameKind::Gomoku { size } => gomoku::analyze(st, size, req.my_color, req.budget_ms, req.want_move),
        GameKind::Go { size } => go::analyze(st, size, req.my_color, req.budget_ms, req.want_move),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goptop_core::board::Coord;

    /// 合法请求必须通过校验——守卫不能误伤正常对局（两种棋、含中盘历史）。
    #[test]
    fn validate_accepts_real_states() {
        let mut go = GameState::new(GameKind::Go { size: 9 });
        go.try_play(Move::Place(Coord::new(4, 4))).unwrap();
        let req = AnalyzeRequest {
            state: go,
            my_color: Stone::White,
            budget_ms: 100,
            want_move: false,
        };
        assert_eq!(req.validate(), Ok(()));

        let mut gomoku = GameState::new(GameKind::default_gomoku());
        gomoku.try_play(Move::Place(Coord::new(7, 7))).unwrap();
        let req = AnalyzeRequest {
            state: gomoku,
            my_color: Stone::Black,
            budget_ms: 100,
            want_move: false,
        };
        assert_eq!(req.validate(), Ok(()));
    }

    /// serde 对 GameState 零校验：非法 kind 与越界坐标都能原样反序列化出来，
    /// 边界必须自己拦。围棋 `Board::with_size(25,25)` 是 assert，panic 跨 FFI
    /// 是整个进程 abort。
    #[test]
    fn validate_rejects_invalid_kind_and_coords() {
        // kind：围棋只允许 9/13/19，25 会让 with_size 直接 assert。
        let mut v = serde_json::to_value(GameState::new(GameKind::default_go())).unwrap();
        v["kind"] = serde_json::json!({ "Go": { "size": 25 } });
        let st: GameState = serde_json::from_value(v).unwrap();
        let req = AnalyzeRequest {
            state: st,
            my_color: Stone::Black,
            budget_ms: 1,
            want_move: false,
        };
        let err = req.validate().expect_err("围棋 size=25 必须被拒");
        assert!(err.contains("25"), "错误文案应带上非法尺寸：{err}");

        // history：坐标必须落在逻辑盘内，错误文案要能定位到具体坐标。
        let mut v = serde_json::to_value(GameState::new(GameKind::default_gomoku())).unwrap();
        v["history"] =
            serde_json::json!([{ "Place": { "x": 7, "y": 7 } }, { "Place": { "x": 20, "y": 0 } }]);
        let st: GameState = serde_json::from_value(v).unwrap();
        let req = AnalyzeRequest {
            state: st,
            my_color: Stone::Black,
            budget_ms: 1,
            want_move: false,
        };
        let err = req.validate().expect_err("越界坐标必须被拒");
        assert!(err.contains("(20,0)"), "错误文案应带上坐标：{err}");
    }

    /// 兜底语义：`analyze` 的签名表达不了错误，非法请求降级成中性结果——绝不
    /// panic 跨 FFI，也不把半分析的数字当真值交出去。
    #[test]
    fn analyze_neutralizes_invalid_request() {
        let mut v = serde_json::to_value(GameState::new(GameKind::Go { size: 9 })).unwrap();
        v["kind"] = serde_json::json!({ "Go": { "size": 25 } });
        let st: GameState = serde_json::from_value(v).unwrap();
        let req = AnalyzeRequest {
            state: st,
            my_color: Stone::Black,
            budget_ms: 50,
            want_move: true,
        };
        assert!(req.validate().is_err());
        let r = analyze(&req); // 修复前：Board::with_size(25,25) assert → 进程 abort
        assert_eq!(r.best_move, None);
        assert_eq!(r.win_rate, 0.5);
        assert_eq!(r.nodes, 0);
    }
}
