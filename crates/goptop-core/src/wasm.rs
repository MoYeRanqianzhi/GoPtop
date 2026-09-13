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

/// SyncState.history 条目：落子坐标对象，或字符串 "pass"（停一手）。
/// 历史契约必须可表达 Pass，否则含 Pass 的对局在 undo 重放/adopt 重建时
/// 轮转必然漂移（2026-09-14 审查 #5 P1-2）。
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum HistoryEntry {
    Place(Coord),
    Pass(String),
}

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
    /// kind_json 见模块注释。非法尺寸组合返回 None 而非触达核心层断言——
    /// release 下 panic="abort"，断言即 wasm trap（白屏），边界必须自己挡
    /// （2026-09-14 审查 #5 P2-3）。
    pub fn new_game(kind_json: &str) -> Option<WasmGame> {
        let kind: GameKind = match serde_json::from_str(kind_json) {
            Ok(k) => k,
            Err(_) => return None,
        };
        if !kind.is_valid() {
            return None;
        }
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

    /// 停一手：引擎翻转行棋方并把 Pass 记入历史，保证后续 undo/adopt 重放
    /// 与真实序列一致（收到 Move{Pass} 或未来本地停一手都走这里）。
    pub fn pass(&mut self) -> String {
        match self.state.try_play(Move::Pass) {
            Ok(effect) => ok_reply(&self.state, &effect.captured),
            Err(_) => err_reply("game_over"),
        }
    }

    /// 撤销最后一手：弹出一手并全量重放（≤361 步，开销可忽略），
    /// 返回回退后的权威棋盘。空历史返回 ok:false。
    pub fn undo_last(&mut self) -> String {
        let mut moves = self.state.history.clone();
        if moves.pop().is_none() {
            return err_reply("no_move");
        }
        let kind = self.state.kind.clone();
        let mut fresh = GameState::new(kind);
        for mv in moves {
            // 重放必成功：这些是历史上通过规则的手，且 adopt 已做格式/边界
            // 校验。万一失败（远端脏快照构造出互斥历史），保持原状态报错，
            // 绝不带病回退（2026-09-14 审查 #5 P2-2）。
            if fresh.try_play(mv).is_err() {
                return err_reply("replay_failed");
            }
        }
        self.state = fresh;
        ok_reply(&self.state, &[])
    }

    /// 重开（同种类）。
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// 采纳全量快照（SyncState）：棋盘/行棋方/胜者直接采用快照（不经规则——
    /// 快照可能来自任何合法序列），历史按坐标/"pass" 序列重建（黑白交替、
    /// 黑先，Pass 原样保留），供后续 undo_last 重放。TS 侧只在收到快照时调用。
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
        // winner 与 to_move 同款守卫："empty" 等非法值直接拒绝（#5 P3-2）。
        let winner = match winner {
            "null" | "" => None,
            "black" => Some(Stone::Black),
            "white" => Some(Stone::White),
            _ => return false,
        };
        let entries: Vec<HistoryEntry> = match serde_json::from_str(history_json) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let mut moves = Vec::with_capacity(entries.len());
        for e in entries {
            match e {
                HistoryEntry::Place(c) => {
                    // 边界校验：坐标对象只有 u8 约束，逻辑越界会污染 undo 重放
                    if c.x as usize >= n || c.y as usize >= n {
                        return false;
                    }
                    moves.push(Move::Place(c));
                }
                HistoryEntry::Pass(s) => {
                    if s != "pass" {
                        return false;
                    }
                    moves.push(Move::Pass);
                }
            }
        }
        self.state.board = variant;
        self.state.to_move = to_move;
        self.state.winner = winner;
        self.state.captures = (0, 0);
        // 历史重建：落子按黑白交替推定，Pass 原样保留（#5 P1-2）。
        self.state.history = moves;
        true
    }
}

/// wasm.rs 的 JSON 契约单测（审查 #6 C10）——在 native 目标直接调用
/// `#[wasm_bindgen]` 导出的 pub fn（返回 String/bool），断言一律先解析成
/// `serde_json::Value`，锁定错误码/字段名等稳定契约而非裸字符串比对。
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn go9() -> WasmGame {
        WasmGame::new_game(r#"{"Go":{"size":9}}"#).unwrap()
    }

    fn gomoku15() -> WasmGame {
        WasmGame::new_game(r#"{"Gomoku":{"size":15}}"#).unwrap()
    }

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    /// board JSON 中 (x,y) 的颜色字符串。
    fn at(v: &Value, x: usize, y: usize) -> &str {
        v["board"][y][x].as_str().unwrap()
    }

    /// n×n 空盘 JSON（全 "empty"）。
    fn empty_board(n: usize) -> String {
        let row = format!("[{}]", vec!["\"empty\""; n].join(","));
        format!("[{}]", vec![row.as_str(); n].join(","))
    }

    /// 空盘上摆子后的棋盘 JSON。
    fn board_with(blacks: &[(usize, usize)], whites: &[(usize, usize)], n: usize) -> String {
        let mut v: Value = serde_json::from_str(&empty_board(n)).unwrap();
        for &(x, y) in blacks {
            v[y][x] = json!("black");
        }
        for &(x, y) in whites {
            v[y][x] = json!("white");
        }
        v.to_string()
    }

    /// 1) 提子后 undo：被提白子还原、行棋方回到提子前的黑。
    #[test]
    fn undo_restores_captured_stone() {
        let mut g = go9();
        // B(1,0) → W(0,0) → B(0,1) 提白 (0,0)。
        assert!(parse(&g.try_place(1, 0))["ok"].as_bool().unwrap());
        assert!(parse(&g.try_place(0, 0))["ok"].as_bool().unwrap());
        let cap = parse(&g.try_place(0, 1));
        assert_eq!(cap["captured"], json!([{"x":0,"y":0}]));
        assert_eq!(at(&cap, 0, 0), "empty");
        let undone = parse(&g.undo_last());
        assert_eq!(at(&undone, 0, 0), "white");
        assert_eq!(undone["toMove"], "black");
        // 被弹出的正是提子那手 B(0,1)（该点回空）；更早的黑子 (1,0) 仍在。
        assert_eq!(at(&undone, 0, 1), "empty");
        assert_eq!(at(&undone, 1, 0), "black");
    }

    /// 2) 空历史 undo：ok:false + 稳定错误码 no_move。
    #[test]
    fn undo_on_empty_history_is_no_move() {
        let mut g = go9();
        let r = parse(&g.undo_last());
        assert_eq!(r["ok"], json!(false));
        assert_eq!(r["error"], "no_move");
    }

    /// 3) 连续两次 undo：逐手回退直至空盘、黑先。
    #[test]
    fn undo_twice_returns_to_empty_board() {
        let mut g = go9();
        assert!(parse(&g.try_place(4, 4))["ok"].as_bool().unwrap());
        assert!(parse(&g.try_place(5, 5))["ok"].as_bool().unwrap());
        let r1 = parse(&g.undo_last());
        assert_eq!(at(&r1, 5, 5), "empty");
        assert_eq!(at(&r1, 4, 4), "black");
        assert_eq!(r1["toMove"], "white");
        let r2 = parse(&g.undo_last());
        assert_eq!(at(&r2, 4, 4), "empty");
        assert_eq!(r2["toMove"], "black");
        assert!(r2["winner"].is_null());
    }

    /// 4) adopt 后 undo：按重建历史全量重放，回到快照前一状态
    /// （而非简单保留快照棋盘——(5,5) 的白子必须因重放而消失）。
    #[test]
    fn adopt_then_undo_replays_rebuilt_history() {
        let mut g = go9();
        // 快照：黑 (4,4)、白 (5,5)，轮白；历史两条同序。
        let board = board_with(&[(4, 4), (5, 5)], &[], 9);
        assert!(g.adopt(&board, "white", "null", r#"[{"x":4,"y":4},{"x":5,"y":5}]"#));
        let after = parse(&g.undo_last());
        assert_eq!(at(&after, 4, 4), "black");
        assert_eq!(at(&after, 5, 5), "empty");
        assert_eq!(after["toMove"], "white");
    }

    /// 5) adopt 维度守卫：行数≠逻辑尺寸、行长不齐均拒绝。
    #[test]
    fn adopt_rejects_dimension_mismatch() {
        let mut g = gomoku15();
        // 14 行给 15 路局。
        let row15 = format!("[{}]", vec!["\"empty\""; 15].join(","));
        let board14 = format!("[{}]", vec![row15.as_str(); 14].join(","));
        assert!(!g.adopt(&board14, "black", "null", "[]"));
        // 行长度不齐。
        let row14 = format!("[{}]", vec!["\"empty\""; 14].join(","));
        let ragged = format!("[{},{},{}]", row15, row14, row15);
        assert!(!g.adopt(&ragged, "black", "null", "[]"));
    }

    /// 6) adopt 字段白名单：toMove 不接受 "empty"；winner 接受 "null"（→None）、
    /// 拒绝白名单外颜色。
    #[test]
    fn adopt_rejects_invalid_to_move_and_winner() {
        let mut g = go9();
        let board = empty_board(9);
        assert!(!g.adopt(&board, "empty", "null", "[]"));
        assert!(g.adopt(&board, "black", "null", "[]"));
        assert!(!g.adopt(&board, "black", "red", "[]"));
        assert!(g.adopt(&board, "white", "black", "[]"));
    }

    /// 7) adopt 历史校验：越界坐标拒绝；非 "pass" 字符串拒绝；
    /// "pass" 接受为 Move::Pass（含 pass 的历史 undo 重放轮转不漂移）。
    #[test]
    fn adopt_history_validation() {
        let mut g = go9();
        let board = empty_board(9);
        assert!(!g.adopt(&board, "black", "null", r#"[{"x":99,"y":0}]"#));
        assert!(!g.adopt(&board, "black", "null", r#"["resign"]"#));
        // 合法：黑落 (4,4)、白 pass → 快照轮黑。
        let board_after_pass = board_with(&[(4, 4)], &[], 9);
        assert!(g.adopt(&board_after_pass, "black", "null", r#"[{"x":4,"y":4},"pass"]"#));
        // undo 弹出 pass 后重放 [Place(4,4)]：黑子仍在、轮白。
        let undone = parse(&g.undo_last());
        assert_eq!(at(&undone, 4, 4), "black");
        assert_eq!(undone["toMove"], "white");
    }

    /// 8) try_place 错误码契约：out_of_bounds/occupied/suicide 锁定为稳定字符串
    /// （game_over 在五连用例中锁定）。
    #[test]
    fn try_place_error_codes() {
        let mut g = go9();
        // 9 路局 (9,0) 超出逻辑尺寸（物理 19×19 也在盘外）。
        assert_eq!(parse(&g.try_place(9, 0))["error"], "out_of_bounds");
        assert!(parse(&g.try_place(4, 4))["ok"].as_bool().unwrap());
        assert_eq!(parse(&g.try_place(4, 4))["error"], "occupied");
        // 自杀：白 (0,1)、(1,0) 分隔不相连各有外气，黑 (0,0) 无气且提不动 → suicide。
        assert!(parse(&g.try_place(0, 1))["ok"].as_bool().unwrap());
        assert!(parse(&g.try_place(5, 5))["ok"].as_bool().unwrap());
        assert!(parse(&g.try_place(1, 0))["ok"].as_bool().unwrap());
        assert_eq!(parse(&g.try_place(0, 0))["error"], "suicide");
    }

    /// 9) 五子棋第 5 子 → winner="black"；终局后再落子 → ok:false + game_over。
    #[test]
    fn gomoku_five_wins_then_game_over() {
        let mut g = gomoku15();
        for x in 0..4u8 {
            assert!(parse(&g.try_place(x, 0))["ok"].as_bool().unwrap()); // 黑连珠
            assert!(parse(&g.try_place(x, 7))["ok"].as_bool().unwrap()); // 白闲着
        }
        let fifth = parse(&g.try_place(4, 0));
        assert_eq!(fifth["winner"], "black");
        let later = parse(&g.try_place(7, 7));
        assert_eq!(later["ok"], json!(false));
        assert_eq!(later["error"], "game_over");
        // 终局态持续：再次落子仍拒绝（err_reply 只含 ok/error，棋盘不可见）。
        let again = parse(&g.try_place(7, 7));
        assert_eq!(again["error"], "game_over");
    }

    /// 10) pass：轮转翻转、棋盘不变；终局后 pass → ok:false + game_over。
    #[test]
    fn pass_flips_to_move_and_rejected_after_game_over() {
        let mut g = go9();
        assert!(parse(&g.try_place(4, 4))["ok"].as_bool().unwrap());
        let before = parse(&g.try_place(5, 5));
        let p1 = parse(&g.pass());
        assert_eq!(p1["toMove"], "white");
        assert_eq!(p1["board"], before["board"]);
        let p2 = parse(&g.pass());
        assert_eq!(p2["toMove"], "black");
        assert_eq!(p2["board"], before["board"]);
        // 终局后 pass 被拒。
        let mut gm = gomoku15();
        for x in 0..4u8 {
            assert!(parse(&gm.try_place(x, 0))["ok"].as_bool().unwrap());
            assert!(parse(&gm.try_place(x, 7))["ok"].as_bool().unwrap());
        }
        assert!(parse(&gm.try_place(4, 0))["ok"].as_bool().unwrap());
        let p3 = parse(&gm.pass());
        assert_eq!(p3["ok"], json!(false));
        assert_eq!(p3["error"], "game_over");
    }
}
