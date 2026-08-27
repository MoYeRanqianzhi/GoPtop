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

/// 统一游戏状态，唯一真源。所有可序列化，便于存档、重放与网络同步。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameState {
    /// 游戏种类与尺寸。
    pub kind: GameKind,
    /// 棋盘。
    pub board: BoardVariant,
    /// 轮到谁走。
    pub to_move: Stone,
    /// 历史（用于悔棋/复盘），按落子顺序。
    pub history: Vec<Move>,
    /// 提子数 (黑, 白) — 围棋统计，五子棋恒为 (0,0)。
    pub captures: (u32, u32),
    /// 胜者（`None` 表示进行中）。
    pub winner: Option<Stone>,
}

impl GameState {
    /// 以指定种类创建新对局，黑先。
    #[must_use]
    pub fn new(kind: GameKind) -> Self {
        let size = kind.size();
        Self {
            board: BoardVariant::new(size),
            kind,
            to_move: Stone::Black,
            history: Vec::new(),
            captures: (0, 0),
            winner: None,
        }
    }

    /// 尝试执行 `mv`，成功返回副作用，失败返回 `RuleError` 且状态不变。
    pub fn try_play(&mut self, mv: Move) -> Result<PlayEffect, RuleError> {
        if self.winner.is_some() {
            return Err(RuleError::GameOver);
        }

        match mv.clone() {
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
                return Ok(PlayEffect {
                    captured: Vec::new(),
                    winner: None,
                });
            }
            Move::Place(coord) => {
                // 边界与占用由各规则模块再次检查，此处先做快速拒绝。
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

        // 按围棋规则落子与提子；底层 `try_place` 已处理自杀回滚。
        let captured = match &mut self.board {
            BoardVariant::B15(b) => crate::go::try_place(b, coord, stone),
            BoardVariant::B19(b) => crate::go::try_place(b, coord, stone),
        }
        .map_err(|e| {
            if e.contains("suicide") {
                RuleError::Suicide
            } else if e.contains("occupied") {
                RuleError::Occupied
            } else if e.contains("bounds") {
                RuleError::OutOfBounds
            } else {
                RuleError::Other(e)
            }
        })?;

        // 更新提子统计：己方提掉的是对手的棋子。
        let n = captured.len() as u32;
        if stone == Stone::Black {
            self.captures.0 += n;
        } else {
            self.captures.1 += n;
        }

        self.history.push(Move::Place(coord));
        self.to_move = stone.opponent();

        // 围棋的胜负不在落子时判定（需终局目数），此处 winner 保持 None。
        Ok(PlayEffect {
            captured,
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
