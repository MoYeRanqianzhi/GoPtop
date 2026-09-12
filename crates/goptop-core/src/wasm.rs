//! WASM 绑定 — 仅在 `feature = "wasm"` 时编译。
//!
//! 前端（Web 与 Tauri WebView 同一份）经本模块用 Rust 规则执行一切落子判定：
//! 五连、提子、禁自杀全部以 goptop-core 为唯一真源，TS 侧不再有规则副本
//! （历史教训：checkFive 曾同时存在三份且语义漂移；TS 版围棋更完全没有提子）。
//!
//! 数据面约定（与前端 net/protocol.ts 对齐，勿改）：
//! - 棋盘 JSON = 行优先二维数组，元素为小写字符串 `"empty"|"black"|"white"`；
//! - 围棋 9/13 路输出**逻辑尺寸**（kind.size()），不暴露 19×19 物理存储；
//! - 颜色 JSON 同上小写；kind 入参为 serde 外部标签格式 `{"Gomoku":{"size":15}}`。
//! 所有方法返回 JSON 字符串（wasm-bindgen 跨界传结构体成本高，且前端已有
//! JSON 解析习惯）；`ok:false` 时附 `error` 稳定字符串供上层提示/忽略。

use wasm_bindgen::prelude::*;

use crate::board::{BoardVariant, Coord, Stone};
use crate::game::{GameKind, GameState, Move};

/// TS 颜色字符串 → Stone（大小写不敏感；非法值返回 None）。
fn stone_from_str(s: &str) -> Option<Stone> {
    match s.to_ascii_lowercase().as_str() {
        "empty" => Some(Stone::Empty),
        "black" => Some(Stone::Black),
        "white" => Some(Stone::White),
        _ => None,
    }
}

fn stone_to_str(s: Stone) -> &'static str {
    match s {
        Stone::Empty => "empty",
        Stone::Black => "black",
        Stone::White => "white",
    }
}

/// 棋盘 → 逻辑尺寸的行优先二维数组 JSON（TS StoneColor[][] 同构）。
fn board_json(state: &GameState) -> String {
    let n = state.kind.size();
    let mut rows: Vec<String> = Vec::with_capacity(n);
    for y in 0..n {
        let mut row: Vec<String> = Vec::with_capacity(n);
        for x in 0..n {
            let c = Coord::new(x as u8, y as u8);
            let s = state.board.get(c).unwrap_or(Stone::Empty);
            row.push(format!("\"{}\"", stone_to_str(s)));
        }
        rows.push(format!("[{}]", row.join(",")));
    }
    format!("[{}]", rows.join(","))
}

fn winner_json(state: &GameState) -> String {
    match state.winner {
        Some(s) => format!("\"{}\"", stone_to_str(s)),
        None => "null".to_string(),
    }
}

fn to_move_json(state: &GameState) -> String {
    format!("\"{}\"", stone_to_str(state.to_move))
}

/// ok 回复公共字段：棋盘（权威）、行棋方、胜者、本手提子。
fn ok_reply(state: &GameState, captured: &[Coord]) -> String {
    let caps: Vec<String> = captured
        .iter()
        .map(|c| format!("{{\"x\":{},\"y\":{}}}", c.x, c.y))
        .collect();
    format!(
        "{{\"ok\":true,\"board\":{},\"captured\":[{}],\"toMove\":{},\"winner\":{}}}",
        board_json(state),
        caps.join(","),
        to_move_json(state),
        winner_json(state),
    )
}

fn err_reply(err: &str) -> String {
    // error 字符串是稳定契约（occupied/out_of_bounds/suicide/game_over/no_move），
    // 前端据此决定静默忽略还是提示。
    format!("{{\"ok\":false,\"error\":\"{err}\"}}")
}

/// 行为化绑定：一个实例持一局 `GameState`。前端每个对局页面/本地页各持一个。
#[wasm_bindgen]
pub struct WasmGame {
    state: GameState,
}

#[wasm_bindgen]
impl WasmGame {
    /// 创建对局（静态工厂；wasm-bindgen 不允许构造函数返回 Option）。
    /// kind_json 见模块注释。尺寸不变量由 GameState::new 断言——前端
    /// kind/size 已在来源处（pickKind/pickSize/链接解析）收敛到合法集合。
    pub fn new_game(kind_json: &str) -> Option<WasmGame> {
        let kind: GameKind = match serde_json::from_str(kind_json) {
            Ok(k) => k,
            Err(_) => return None,
        };
        Some(WasmGame { state: GameState::new(kind) })
    }

    /// 当前逻辑尺寸。
    #[wasm_bindgen(js_name = boardSize)]
    pub fn board_size(&self) -> u8 {
        self.state.kind.size() as u8
    }

    /// 落子（唯一规则入口）：合法则更新内部棋盘并返回权威棋盘/提子/胜负；
    /// 非法（占据/越界/自杀/终局）返回 ok:false，内部状态不变。
    /// 越界/占据/终局判定都在 GameState::try_play 内。
    pub fn try_place(&mut self, x: u8, y: u8) -> String {
        match self.state.try_play(Move::Place(Coord::new(x, y))) {
            Ok(effect) => ok_reply(&self.state, &effect.captured),
            Err(e) => err_reply(match e {
                crate::game::RuleError::OutOfBounds => "out_of_bounds",
                crate::game::RuleError::Occupied => "occupied",
                crate::game::RuleError::Suicide => "suicide",
                crate::game::RuleError::GameOver => "game_over",
                crate::game::RuleError::Other(_) => "other",
            }),
        }
    }

    /// 撤销最后一手：弹出一手并全量重放（≤361 步，开销可忽略），
    /// 返回回退后的权威棋盘。空历史返回 ok:false。
    pub fn undo_last(&mut self) -> String {
        if self.state.history.pop().is_none() {
            return err_reply("no_move");
        }
        let kind = self.state.kind.clone();
        let moves = self.state.history.clone();
        let mut fresh = GameState::new(kind);
        for mv in moves {
            let _ = fresh.try_play(mv); // 重放必成功：这些是历史上通过规则的手
        }
        self.state = fresh;
        ok_reply(&self.state, &[])
    }

    /// 重开（同种类）。
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// 采纳全量快照（SyncState）：棋盘/行棋方/胜者直接采用快照（不经规则——
    /// 快照可能来自任何合法序列），历史用坐标序列重建（黑白交替、黑先），
    /// 供后续 undo_last 重放。TS 侧只在收到快照时调用。
    pub fn adopt(&mut self, board_json: &str, to_move: &str, winner: &str, history_json: &str) -> bool {
        let rows: Vec<Vec<String>> = match serde_json::from_str(board_json) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let n = self.state.kind.size();
        if rows.len() != n || rows.iter().any(|r| r.len() != n) {
            return false;
        }
        let mut variant = BoardVariant::new(n);
        for (y, row) in rows.iter().enumerate() {
            for (x, cell) in row.iter().enumerate() {
                let Some(s) = stone_from_str(cell) else { return false };
                variant.set(Coord::new(x as u8, y as u8), s);
            }
        }
        let Some(to_move) = stone_from_str(to_move) else { return false };
        if !matches!(to_move, Stone::Black | Stone::White) {
            return false;
        }
        let winner = if winner == "null" || winner.is_empty() { None } else { stone_from_str(winner) };
        let coords: Vec<Coord> = match serde_json::from_str(history_json) {
            Ok(v) => v,
            Err(_) => return false,
        };
        self.state.board = variant;
        self.state.to_move = to_move;
        self.state.winner = winner;
        self.state.captures = (0, 0);
        // 历史重建：颜色按黑白交替推定（与 TS history 的隐含约定一致）。
        self.state.history = coords.into_iter().map(Move::Place).collect();
        true
    }
}
