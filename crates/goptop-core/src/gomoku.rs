//! 五子棋规则 — 五连判定。
//!
//! 仅负责“给定棋盘与最后一手，是否形成五连”。禁手、长连等扩展后加，不影响核心判定。

use crate::board::{Board, Coord, Stone};

/// 四个方向的单位向量：水平、垂直、主对角、副对角。
const DIRS: [(i8, i8); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

/// 判断 `stone` 在 `coord` 落子后是否形成五连（恰好/至少五连均视为胜，符合常见无禁手规则）。
///
/// 调用方需保证 `coord` 上已放置 `stone`。
#[must_use]
pub fn is_five<const N: usize>(board: &Board<N>, coord: Coord, stone: Stone) -> bool {
    debug_assert!(stone != Stone::Empty);
    for (dx, dy) in DIRS {
        let count = 1 + count_dir(board, coord, stone, dx, dy) + count_dir(board, coord, stone, -dx, -dy);
        if count >= 5 {
            return true;
        }
    }
    false
}

/// 从 `origin` 出发沿 `(dx,dy)` 方向连续同色棋子的数量（不含起点）。
fn count_dir<const N: usize>(board: &Board<N>, origin: Coord, stone: Stone, dx: i8, dy: i8) -> usize {
    let mut count = 0;
    let mut x = origin.x as i16 + dx as i16;
    let mut y = origin.y as i16 + dy as i16;
    while x >= 0 && y >= 0 {
        let c = Coord::new(x as u8, y as u8);
        match board.get(c) {
            Some(s) if s == stone => count += 1,
            _ => break,
        }
        x += dx as i16;
        y += dy as i16;
    }
    count
}

/// 扫描全盘是否存在 `stone` 的五连（用于终局校验或回放）。
#[must_use]
pub fn has_five<const N: usize>(board: &Board<N>, stone: Stone) -> bool {
    for y in 0..N {
        for x in 0..N {
            let c = Coord::new(x as u8, y as u8);
            if board.get(c) == Some(stone) && is_five(board, c, stone) {
                return true;
            }
        }
    }
    false
}
