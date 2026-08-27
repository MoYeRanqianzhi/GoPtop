//! 围棋规则 — 最小可玩子集。
//!
//! 本模块实现围棋的核心落子语义：
//! - 落子后对四邻对手块计算气，若气为 0 则提子；
//! - 最后检查己方块是否有气，无气则为自杀（非法手）；
//! - 提子数计入 `captures`，供上层统计。
//!
//! 暂不实现：劫（ko）、打劫循环、终局目数。它们在 Phase 4 补齐，不阻塞 Web 验证主线。

use crate::board::{Board, Coord, Stone};
use std::collections::{HashSet, VecDeque};

/// 四邻方向。
const NEIGHBORS: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// 对 `coord` 的同色连通块做 BFS，返回 `(块内所有坐标, 气的数量)`。
fn block_and_liberties<const N: usize>(board: &Board<N>, start: Coord) -> (Vec<Coord>, usize) {
    let color = match board.get(start) {
        Some(s) if s != Stone::Empty => s,
        _ => return (Vec::new(), 0),
    };
    let mut visited: HashSet<Coord> = HashSet::new();
    let mut libs: HashSet<Coord> = HashSet::new();
    let mut queue = VecDeque::new();
    let mut block = Vec::new();

    visited.insert(start);
    queue.push_back(start);

    while let Some(cur) = queue.pop_front() {
        block.push(cur);
        for (dx, dy) in NEIGHBORS {
            let nx = cur.x as i16 + dx as i16;
            let ny = cur.y as i16 + dy as i16;
            if nx < 0 || ny < 0 {
                continue;
            }
            let nc = Coord::new(nx as u8, ny as u8);
            match board.get(nc) {
                None => continue, // 越界
                Some(Stone::Empty) => {
                    libs.insert(nc);
                }
                Some(s) if s == color => {
                    if visited.insert(nc) {
                        queue.push_back(nc);
                    }
                }
                _ => {} // 对手棋子，不计入气也不扩展
            }
        }
    }

    (block, libs.len())
}

/// 尝试在 `board` 的 `coord` 落 `stone`，按围棋规则执行提子与自杀检查。
///
/// - 成功：返回被提走的对手棋子坐标列表（可能为空），棋盘已更新。
/// - 失败：返回错误字符串，棋盘保持不变（调用方需在外层回滚或先克隆）。
pub fn try_place<const N: usize>(
    board: &mut Board<N>,
    coord: Coord,
    stone: Stone,
) -> Result<Vec<Coord>, String> {
    if stone == Stone::Empty {
        return Err("stone must be Black or White".to_string());
    }
    if board.get(coord).is_none() {
        return Err("out of bounds".to_string());
    }
    if !board.is_empty(coord) {
        return Err("point already occupied".to_string());
    }

    // 先落子，再检查提子与自杀；若非法则回滚。
    board.set(coord, stone);

    let opponent = stone.opponent();
    let mut captured: Vec<Coord> = Vec::new();
    let mut seen_blocks: HashSet<Coord> = HashSet::new();

    // 检查四邻对手块，气为 0 则提子。
    for (dx, dy) in NEIGHBORS {
        let nx = coord.x as i16 + dx as i16;
        let ny = coord.y as i16 + dy as i16;
        if nx < 0 || ny < 0 {
            continue;
        }
        let nc = Coord::new(nx as u8, ny as u8);
        if board.get(nc) != Some(opponent) {
            continue;
        }
        if seen_blocks.contains(&nc) {
            continue;
        }
        let (block, libs) = block_and_liberties(board, nc);
        for b in &block {
            seen_blocks.insert(*b);
        }
        if libs == 0 {
            captured.extend(block);
        }
    }

    // 移除被提子（先收集再移除，避免 BFS 期间棋盘变化影响其他块的气计算——此处已在落子后统一计算，安全）。
    for c in &captured {
        board.set(*c, Stone::Empty);
    }

    // 检查己方是否有气（自杀），无气且未提子则为非法手，回滚。
    let (_, libs) = block_and_liberties(board, coord);
    if libs == 0 {
        // 回滚：恢复落子点 + 被提子
        board.set(coord, Stone::Empty);
        for c in &captured {
            board.set(*c, opponent);
        }
        return Err("suicide move not allowed".to_string());
    }

    Ok(captured)
}
