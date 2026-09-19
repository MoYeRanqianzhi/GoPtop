//! 统一游戏状态机 — 屏蔽 Gomoku/Go 的差异，供上层（前端、传输、AI）统一调用。
//!
//! `GameState` 持有当前棋盘、轮到谁走、历史与提子数；所有落子通过 `try_play` 进入，
//! 内部按 `GameKind` 分发到 `gomoku`/`go` 的规则实现。

use crate::board::{BoardVariant, Coord, Stone};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 游戏种类与尺寸。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameKind {
    /// 五子棋，尺寸固定 15。
    Gomoku { size: u8 },
    /// 围棋，尺寸 9/13/19。
    Go { size: u8 },
}

impl GameKind {
    /// 默认五子棋 15 路。
    #[must_use]
    pub fn default_gomoku() -> Self {
        Self::Gomoku { size: 15 }
    }

    /// 默认围棋 19 路。
    #[must_use]
    pub fn default_go() -> Self {
        Self::Go { size: 19 }
    }

    /// 逻辑棋盘尺寸。
    #[must_use]
    pub fn size(&self) -> usize {
        match self {
            Self::Gomoku { size } | Self::Go { size } => *size as usize,
        }
    }
}

/// 落子/操作类型。`Pass`/`Resign` 对五子棋亦保留（UI 可禁用），便于统一协议。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Move {
    Place(Coord),
    Pass,
    Resign,
}

/// 规则错误，供上层展示给用户。
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleError {
    #[error("out of bounds")]
    OutOfBounds,
    #[error("point already occupied")]
    Occupied,
    #[error("suicide move not allowed")]
    Suicide,
    /// 简单劫：立即回提造成全局同形（围棋）。
    #[error("ko: immediate recapture forbidden")]
    Ko,
    /// 对局已进入终局计分阶段（双 Pass 后），不再接受落子/停一手。
    #[error("game is in scoring phase")]
    Scoring,
    #[error("game already over")]
    GameOver,
    #[error("{0}")]
    Other(String),
}

/// 落子后的副作用，供 UI 做动画/统计。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayEffect {
    /// 本手提掉的对手棋子（围棋），五子棋恒为空。
    pub captured: Vec<Coord>,
    /// 本手是否导致胜负已分（五子棋五连或某方认输）。
    pub winner: Option<Stone>,
}

/// 统一游戏状态，唯一真源。可序列化，便于存档与重放；**它不是线格式** ——
/// 联机走 `crates/goptop-net/src/protocol.rs` 的字符串标签格式（`"go"` + 独立 size），两套不可混用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameState {
    /// 游戏种类与尺寸。
    pub kind: GameKind,
    pub board: BoardVariant,
    /// 轮到谁走。
    pub to_move: Stone,
    /// 历史（用于悔棋/复盘），按落子顺序。
    pub history: Vec<Move>,
    /// 提子数 (黑, 白) — 围棋统计，五子棋恒为 (0,0)。
    pub captures: (u32, u32),
    /// 当前劫点（围棋简单劫）：对手不得立即在此回提。
    /// 不单独存档也可由 history 全量重放恢复，随状态携带便于直接查询。
    pub ko_point: Option<Coord>,
    /// 终局计分阶段（围棋连续双 Pass 后进入）：只允许死子标记与计分，不再接受落子。
    pub scoring: bool,
    /// 胜者（`None` 表示进行中）。
    pub winner: Option<Stone>,
}

impl GameKind {
    /// 尺寸组合合法性：五子棋仅 15，围棋仅 9/13/19。
    /// 核心层 `GameState::new` 对非法组合 panic（内部保证），wasm 边界
    /// 与其他外部输入入口应先经本方法校验，避免 panic 逃逸。
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            GameKind::Gomoku { size } => *size == 15,
            GameKind::Go { size } => matches!(*size, 9 | 13 | 19),
        }
    }
}

impl GameState {
    /// 以指定种类创建新对局，黑先。
    ///
    /// 尺寸不变量在边界校验：五子棋仅 15，围棋仅 9/13/19。非法组合 panic——
    /// 这是核心层的内部保证（URL 等外部输入应在反序列化/前端入口先校验），
    /// 防止"逻辑 9 路 + 物理 19 路盘"的混搭状态悄然通过。
    #[must_use]
    pub fn new(kind: GameKind) -> Self {
        assert!(kind.is_valid(), "invalid GameKind size combination: {kind:?}");
        let size = kind.size();
        Self {
            board: BoardVariant::new(size),
            kind,
            to_move: Stone::Black,
            history: Vec::new(),
            captures: (0, 0),
            ko_point: None,
            scoring: false,
            winner: None,
        }
    }

    /// 尝试执行 `mv`，成功返回副作用，失败返回 `RuleError` 且状态不变。
    pub fn try_play(&mut self, mv: Move) -> Result<PlayEffect, RuleError> {
        if self.winner.is_some() {
            return Err(RuleError::GameOver);
        }
        if self.scoring {
            // 终局计分阶段：落子/停一手/认输都不再接受（悔棋可退出计分态，走 undo 重放）。
            return Err(RuleError::Scoring);
        }

        match mv.clone() {
            // 胜者由 to_move 反推（本函数不知道是谁点的认输，认输者也未必正在行棋）：
            // 调用方须按认输者执色重定 winner，否则会「我认输却判我赢」（见 goptop-net matchplay.rs 的 resign）。
            Move::Resign => {
                let w = self.to_move.opponent();
                self.winner = Some(w);
                self.history.push(mv);
                // 认输不换手，直接结束。
                return Ok(PlayEffect {
                    captured: Vec::new(),
                    winner: self.winner,
                });
            }
            Move::Pass => {
                self.history.push(mv);
                self.to_move = self.to_move.opponent();
                // 围棋连续双 Pass → 终局计分。倒数第二手也是 Pass 才构成双 Pass
                //（倒数第一手是刚推入的本手）。五子棋 Pass 只翻手（UI 不暴露）。
                if matches!(self.kind, GameKind::Go { .. })
                    && matches!(self.history.iter().rev().nth(1), Some(Move::Pass))
                {
                    self.scoring = true;
                }
                return Ok(PlayEffect {
                    captured: Vec::new(),
                    winner: None,
                });
            }
            Move::Place(coord) => {
                // 逻辑边界：围棋 9/13 路的物理棋盘仍为 19×19，超出 kind.size() 的坐标视为越界。
                // 此检查必须在 `board.get` 的物理检查之前，否则 (9,0) 在 B19 上会被误判为合法。
                let logical = self.kind.size();
                if (coord.x as usize) >= logical || (coord.y as usize) >= logical {
                    return Err(RuleError::OutOfBounds);
                }
                if self.board.get(coord).is_none() {
                    return Err(RuleError::OutOfBounds);
                }
                if !self.board.is_empty(coord) {
                    return Err(RuleError::Occupied);
                }

                match &self.kind {
                    GameKind::Gomoku { .. } => self.try_place_gomoku(coord),
                    GameKind::Go { .. } => self.try_place_go(coord),
                }
            }
        }
    }

    fn try_place_gomoku(&mut self, coord: Coord) -> Result<PlayEffect, RuleError> {
        let stone = self.to_move;
        // 五子棋无需提子/气，直接落子。
        self.board.set(coord, stone);
        self.history.push(Move::Place(coord));

        // 胜负判定：以最后一手为中心检查五连。
        let won = match &self.board {
            BoardVariant::B15(b) => crate::gomoku::is_five(b, coord, stone),
            BoardVariant::B19(b) => crate::gomoku::is_five(b, coord, stone),
        };
        if won {
            self.winner = Some(stone);
        } else {
            self.to_move = stone.opponent();
        }

        Ok(PlayEffect {
            captured: Vec::new(),
            winner: self.winner,
        })
    }

    fn try_place_go(&mut self, coord: Coord) -> Result<PlayEffect, RuleError> {
        let stone = self.to_move;
        let ko_point = self.ko_point;
        let logical = self.kind.size();

        // 按围棋规则落子与提子；底层 `try_place` 已处理劫检查/自杀回滚，
        // 且直接返回结构化 RuleError（审查 C4：不再做字符串匹配分类）
        let outcome = match &mut self.board {
            BoardVariant::B15(b) => crate::go::try_place(b, coord, stone, ko_point, logical),
            BoardVariant::B19(b) => crate::go::try_place(b, coord, stone, ko_point, logical),
        }?;

        // 更新提子统计：己方提掉的是对手的棋子。
        let n = outcome.captured.len() as u32;
        if stone == Stone::Black {
            self.captures.0 += n;
        } else {
            self.captures.1 += n;
        }

        // 劫点随每手更新：非劫形态清空，构成劫则记录（undo 全量重放会自然恢复）。
        self.ko_point = outcome.new_ko;

        self.history.push(Move::Place(coord));
        self.to_move = stone.opponent();

        // 围棋的胜负不在落子时判定（双 Pass 终局后区域计分），此处 winner 保持 None。
        Ok(PlayEffect {
            captured: outcome.captured,
            winner: None,
        })
    }

    /// 重置为同种类的新对局。
    pub fn reset(&mut self) {
        let kind = self.kind.clone();
        *self = Self::new(kind);
    }
}

/// 对局规则的统一接口，便于 AI/校验等按 `GameKind` 多态调用。
pub trait RuleSet {
    fn try_play(&mut self, mv: Move) -> Result<PlayEffect, RuleError>;
    fn winner(&self) -> Option<Stone>;
}

impl RuleSet for GameState {
    fn try_play(&mut self, mv: Move) -> Result<PlayEffect, RuleError> {
        GameState::try_play(self, mv)
    }

    fn winner(&self) -> Option<Stone> {
        self.winner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Coord;

    /// 9 路围棋超出逻辑边界的落子应被拒绝，即使物理棋盘为 19×19。
    #[test]
    fn go_9_out_of_logical_bounds_rejected() {
        let mut s = GameState::new(GameKind::Go { size: 9 });
        // (9,0) 与 (0,9) 恰好超出 9 路有效区（有效索引 0..8）。
        assert_eq!(
            s.try_play(Move::Place(Coord::new(9, 0))),
            Err(RuleError::OutOfBounds)
        );
        assert_eq!(
            s.try_play(Move::Place(Coord::new(0, 9))),
            Err(RuleError::OutOfBounds)
        );
        // 边界内仍可落子。
        assert!(s.try_play(Move::Place(Coord::new(8, 8))).is_ok());
    }

    /// 13 路同理。
    #[test]
    fn go_13_out_of_logical_bounds_rejected() {
        let mut s = GameState::new(GameKind::Go { size: 13 });
        assert_eq!(
            s.try_play(Move::Place(Coord::new(13, 0))),
            Err(RuleError::OutOfBounds)
        );
        assert_eq!(
            s.try_play(Move::Place(Coord::new(0, 13))),
            Err(RuleError::OutOfBounds)
        );
        assert!(s.try_play(Move::Place(Coord::new(12, 12))).is_ok());
    }

    /// 19 路物理边界与逻辑边界一致，19 为越界、18 可落子。
    #[test]
    fn go_19_physical_bounds() {
        let mut s = GameState::new(GameKind::Go { size: 19 });
        assert_eq!(
            s.try_play(Move::Place(Coord::new(19, 0))),
            Err(RuleError::OutOfBounds)
        );
        assert!(s.try_play(Move::Place(Coord::new(18, 18))).is_ok());
    }

    /// 五子棋 15 路边界：15 越界、14 可落子。
    #[test]
    fn gomoku_15_bounds() {
        let mut s = GameState::new(GameKind::Gomoku { size: 15 });
        assert_eq!(
            s.try_play(Move::Place(Coord::new(15, 0))),
            Err(RuleError::OutOfBounds)
        );
        assert!(s.try_play(Move::Place(Coord::new(14, 14))).is_ok());
    }

    /// 围棋落子后提子与轮手切换是否正确（最小闭气提子场景）。
    #[test]
    fn go_capture_and_turn() {
        let mut s = GameState::new(GameKind::Go { size: 9 });
        // 围住 (1,1) 的白子：黑在四周落子后提白。
        //  1) 黑 (0,1)  2) 白 (1,1)  3) 黑 (1,0)  4) 白 pass  5) 黑 (2,1)  6) 白 pass  7) 黑 (1,2) -> 提白
        assert!(s.try_play(Move::Place(Coord::new(0, 1))).is_ok()); // B
        assert!(s.try_play(Move::Place(Coord::new(1, 1))).is_ok()); // W
        assert!(s.try_play(Move::Place(Coord::new(1, 0))).is_ok()); // B
        assert!(s.try_play(Move::Pass).is_ok()); // W pass
        assert!(s.try_play(Move::Place(Coord::new(2, 1))).is_ok()); // B
        assert!(s.try_play(Move::Pass).is_ok()); // W pass
        let eff = s.try_play(Move::Place(Coord::new(1, 2))).unwrap(); // B 提子
        assert_eq!(eff.captured, vec![Coord::new(1, 1)]);
        // 提子计数：黑提 1。
        assert_eq!(s.captures.0, 1);
        assert_eq!(s.captures.1, 0);
    }

    /// 自杀手应被拒绝且不改变轮手。
    #[test]
    fn go_suicide_rejected() {
        let mut s = GameState::new(GameKind::Go { size: 9 });
        // 形成白子包围黑自杀点的形状：白在 (0,1) 与 (1,0)，黑若在 (0,0) 落子即自杀。
        assert!(s.try_play(Move::Place(Coord::new(5, 5))).is_ok()); // B 随便一手
        assert!(s.try_play(Move::Place(Coord::new(0, 1))).is_ok()); // W
        assert!(s.try_play(Move::Place(Coord::new(6, 6))).is_ok()); // B
        assert!(s.try_play(Move::Place(Coord::new(1, 0))).is_ok()); // W
        // 轮到黑，(0,0) 此时四邻仅有白子与边界，无气且不能提子 -> 自杀
        let err = s.try_play(Move::Place(Coord::new(0, 0))).unwrap_err();
        assert_eq!(err, RuleError::Suicide);
        // 轮手未变，仍为黑。
        assert_eq!(s.to_move, Stone::Black);
    }

    /// 劫争全流程：黑提子成劫 → 白立即回提被拒（Ko）→ 白他处落子后劫点清空
    /// → 白隔一手回提成功且再次成劫。
    #[test]
    fn go_ko_forbids_immediate_recapture() {
        let mut s = GameState::new(GameKind::Go { size: 9 });
        // 黑白交错「风车」劫形：黑固定 (0,2),(2,2),(1,3)（白 Q=(1,2) 的三邻）；
        // 白固定 (0,1),(2,1),(1,0)（黑 P=(1,1) 的三邻）；黑外子凑轮手。
        // 白 Q 入住后仅一气 (1,1)，黑填 P 提 Q 成劫；P 单子一气。
        let seq: [(u8, u8); 13] = [
            (0, 2), // 1  B
            (0, 1), // 2  W
            (2, 2), // 3  B
            (2, 1), // 4  W
            (1, 3), // 5  B
            (1, 0), // 6  W
            (7, 7), // 7  B 外子
            (7, 8), // 8  W 外子
            (8, 7), // 9  B 外子
            (8, 8), // 10 W 外子
            (6, 7), // 11 B 外子
            (1, 2), // 12 W Q 入住：气=(1,1) 一口（合法非自杀）
            (1, 1), // 13 B P 填入：提白 Q=(1,2)，P 单子一气 → 成劫
        ];
        for (x, y) in seq {
            assert!(s.try_play(Move::Place(Coord::new(x, y))).is_ok(), "({x},{y})");
        }
        // 构成劫：劫点 = 被提子位置 (1,2)，轮白。
        assert_eq!(s.ko_point, Some(Coord::new(1, 2)));
        assert_eq!(s.to_move, Stone::White);
        // 白立即回提 (1,1)：黑 P 仅一气 (1,2)，正常提子成立但正是劫点 → 拒。
        assert_eq!(
            s.try_play(Move::Place(Coord::new(1, 2))),
            Err(RuleError::Ko)
        );
        // 白改下他处，劫点清空。
        assert!(s.try_play(Move::Place(Coord::new(6, 8))).is_ok());
        assert_eq!(s.ko_point, None);
        // 轮黑随手一手后，白回提成功：白 (1,2) 提黑 P=(1,1)，再成劫（劫点换为 (1,1)）。
        assert!(s.try_play(Move::Place(Coord::new(5, 7))).is_ok()); // B
        let eff = s.try_play(Move::Place(Coord::new(1, 2))).unwrap(); // W 回提
        assert_eq!(eff.captured, vec![Coord::new(1, 1)]);
        assert_eq!(s.ko_point, Some(Coord::new(1, 1)));
    }

    /// 双 Pass → 终局计分：落子/停一手均被拒（Scoring）。
    #[test]
    fn go_double_pass_enters_scoring() {
        let mut s = GameState::new(GameKind::Go { size: 9 });
        assert!(s.try_play(Move::Place(Coord::new(4, 4))).is_ok()); // B
        assert!(s.try_play(Move::Pass).is_ok()); // W pass（单 Pass 不终局）
        assert!(!s.scoring);
        assert!(s.try_play(Move::Place(Coord::new(5, 5))).is_ok()); // B 继续
        assert!(s.try_play(Move::Pass).is_ok()); // W
        assert!(s.try_play(Move::Pass).is_ok()); // B pass → 双 Pass 终局
        assert!(s.scoring);
        // 计分阶段：落子与停一手都被拒。
        assert_eq!(
            s.try_play(Move::Place(Coord::new(6, 6))),
            Err(RuleError::Scoring)
        );
        assert_eq!(s.try_play(Move::Pass), Err(RuleError::Scoring));
        assert_eq!(s.to_move, Stone::White);
    }

    /// 五子棋双 Pass 不进入计分（Pass 对五子棋只是翻手，无终局语义）。
    #[test]
    fn gomoku_double_pass_not_scoring() {
        let mut s = GameState::new(GameKind::Gomoku { size: 15 });
        assert!(s.try_play(Move::Pass).is_ok());
        assert!(s.try_play(Move::Pass).is_ok());
        assert!(!s.scoring);
        // 仍可落子。
        assert!(s.try_play(Move::Place(Coord::new(7, 7))).is_ok());
    }
}
