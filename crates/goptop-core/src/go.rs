//! 围棋规则 — 落子/提子/劫/自杀/区域计分。
//!
//! 本模块实现围棋的核心规则：
//! - 落子后对四邻对手块计算气，若气为 0 则提子；
//! - 最后检查己方块是否有气，无气则为自杀（非法手）；
//! - 简单劫（ko）：禁止立即回提单子造成全局同形——落子恰提一子、落子块为
//!   单子且仅一气时，记 `new_ko = Some(被提子坐标)`，对手下一手不得在该点落子；
//! - 区域计分（中国规则数子法）：移除死子后按空点连通域归属计分，贴目 7.5。
//!
//! 劫状态不单独存储于棋盘：由调用方（`GameState`）持 `ko_point` 并随每手更新，
//! undo 全量重放 / adopt 历史重建都会自然恢复出正确劫点。

use crate::board::{Board, Coord, Stone};
use crate::game::RuleError;
use std::collections::{HashSet, VecDeque};

/// 四邻方向。
const NEIGHBORS: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// 中国规则贴目（目数，加给白方）。区域法下 7.5 保证无和棋。
pub const KOMI: f32 = 7.5;

/// 落子的完整结果：被提子列表 + 新劫点（无劫则 `None`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureOutcome {
    /// 本手提掉的对手棋子坐标（可能为空）。
    pub captured: Vec<Coord>,
    /// 若本手构成劫争形态，记录被提子坐标作为劫点；对手下一手在此落子将被拒。
    pub new_ko: Option<Coord>,
}

/// 对 `coord` 的同色连通块做 BFS，返回 `(块内所有坐标, 气的数量)`。
///
/// `logical`：逻辑棋盘边长。围棋 9/13 路物理盘为 19×19，逻辑外的空点
/// （如 9 路的 (9,0)）既不算气也不扩展——否则边角块会被物理盘外假气保命。
fn block_and_liberties<const N: usize>(
    board: &Board<N>,
    start: Coord,
    logical: usize,
) -> (Vec<Coord>, usize) {
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
            if nc.x as usize >= logical || nc.y as usize >= logical {
                continue; // 逻辑盘外：不算气也不扩展
            }
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

/// 尝试在 `board` 的 `coord` 落 `stone`，按围棋规则执行劫检查、提子与自杀检查。
///
/// - `ko_point`：当前劫点（来自 `GameState`，随历史重放恢复）。落子在劫点直接拒。
/// - `logical`：逻辑棋盘边长（围棋 9/13 路物理盘 19×19，只统计 0..logical）。
/// - 成功：返回 [`CaptureOutcome`]（被提子 + 新劫点），棋盘已更新。
/// - 失败：返回结构化 [`RuleError`]，棋盘保持不变（调用方需在外层回滚或先克隆）。
pub fn try_place<const N: usize>(
    board: &mut Board<N>,
    coord: Coord,
    stone: Stone,
    ko_point: Option<Coord>,
    logical: usize,
) -> Result<CaptureOutcome, RuleError> {
    if stone == Stone::Empty {
        return Err(RuleError::Other("stone must be Black or White".into()));
    }
    if board.get(coord).is_none() {
        return Err(RuleError::OutOfBounds);
    }
    if !board.is_empty(coord) {
        return Err(RuleError::Occupied);
    }
    // 劫：立即回提造成同形，禁止。
    if ko_point == Some(coord) {
        return Err(RuleError::Ko);
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
        let (block, libs) = block_and_liberties(board, nc, logical);
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
    let (_, libs) = block_and_liberties(board, coord, logical);
    if libs == 0 {
        // 回滚：恢复落子点 + 被提子
        board.set(coord, Stone::Empty);
        for c in &captured {
            board.set(*c, opponent);
        }
        return Err(RuleError::Suicide);
    }

    // 劫形态判定：恰提一子、落子块为单子、落子后仅一气——对手立即回提即全局同形。
    // 提多子或块大于一子都不构成简单劫（打劫循环由禁止"立即"回提覆盖，多劫循环不在最小可玩子集内）。
    let new_ko = if captured.len() == 1 {
        let (block, own_libs) = block_and_liberties(board, coord, logical);
        if block.len() == 1 && own_libs == 1 {
            Some(captured[0])
        } else {
            None
        }
    } else {
        None
    };

    Ok(CaptureOutcome { captured, new_ko })
}

/// 区域计分结果（中国规则数子法）。
#[derive(Clone, Debug, PartialEq)]
pub struct AreaScore {
    /// 黑方得分：黑子数 + 黑地。
    pub black: f32,
    /// 白方得分：白子数 + 白地 + 贴目 [`KOMI`]。
    pub white: f32,
    /// 黑地（仅空点归属，不含子）。
    pub black_territory: u32,
    /// 白地（仅空点归属，不含子）。
    pub white_territory: u32,
    /// 计分时移除的死子数。
    pub dead_removed: u32,
    /// 胜方；区域法 + 7.5 贴目下无和棋，恒有胜者。
    pub winner: Stone,
}

/// 区域计分：把 `dead` 视为死子从临时盘面移除后，按空点连通域归属计分。
///
/// - `logical`：逻辑棋盘边长（围棋 9/13 路物理盘为 19×19，只统计 0..logical）；
/// - 死子只参与计分，不修改传入棋盘；
/// - 空点连通域只接触一种颜色 → 归该色；同时接触黑白（公气/damé）→ 双方均不计；
/// - `dead` 中坐标必须在盘内且有子，否则返回错误（防远端脏数据）。
pub fn score_area<const N: usize>(
    board: &Board<N>,
    dead: &[Coord],
    komi: f32,
    logical: usize,
) -> Result<AreaScore, RuleError> {
    for c in dead {
        if c.x as usize >= logical || c.y as usize >= logical {
            return Err(RuleError::OutOfBounds);
        }
        match board.get(*c) {
            None => return Err(RuleError::OutOfBounds),
            Some(Stone::Empty) => return Err(RuleError::Other("dead point is empty".into())),
            _ => {}
        }
    }

    // 临时盘面：移除死子。
    let mut work = board.clone();
    for c in dead {
        work.set(*c, Stone::Empty);
    }

    let mut black_stones: u32 = 0;
    let mut white_stones: u32 = 0;
    let mut black_territory: u32 = 0;
    let mut white_territory: u32 = 0;

    // 统计活子数（仅逻辑区域）。
    for y in 0..logical {
        for x in 0..logical {
            match work.get(Coord::new(x as u8, y as u8)) {
                Some(Stone::Black) => black_stones += 1,
                Some(Stone::White) => white_stones += 1,
                _ => {}
            }
        }
    }

    // 空点连通域归属：BFS 每片空域，记录接触的颜色集合（仅逻辑区域）。
    let mut visited: HashSet<Coord> = HashSet::new();
    for y in 0..logical {
        for x in 0..logical {
            let start = Coord::new(x as u8, y as u8);
            if work.get(start) != Some(Stone::Empty) || !visited.insert(start) {
                continue;
            }
            let mut queue = VecDeque::new();
            let mut region = Vec::new();
            let mut touches: HashSet<Stone> = HashSet::new();
            queue.push_back(start);
            while let Some(cur) = queue.pop_front() {
                region.push(cur);
                for (dx, dy) in NEIGHBORS {
                    let nx = cur.x as i16 + dx as i16;
                    let ny = cur.y as i16 + dy as i16;
                    if nx < 0 || ny < 0 {
                        continue;
                    }
                    let nc = Coord::new(nx as u8, ny as u8);
                    if nc.x as usize >= logical || nc.y as usize >= logical {
                        continue; // 逻辑盘外不参与空域（9/13 路物理盘 19×19）
                    }
                    match work.get(nc) {
                        None => continue,
                        Some(Stone::Empty) => {
                            if visited.insert(nc) {
                                queue.push_back(nc);
                            }
                        }
                        Some(s) => {
                            touches.insert(s);
                        }
                    }
                }
            }
            // 只接触一种颜色 → 该色地盘；黑白都接触 → 公气，不计。
            if touches.len() == 1 {
                if touches.contains(&Stone::Black) {
                    black_territory += region.len() as u32;
                } else {
                    white_territory += region.len() as u32;
                }
            }
        }
    }

    let black = (black_stones + black_territory) as f32;
    let white = (white_stones + white_territory) as f32 + komi;
    let winner = if black > white { Stone::Black } else { Stone::White };

    Ok(AreaScore {
        black,
        white,
        black_territory,
        white_territory,
        dead_removed: dead.len() as u32,
        winner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Board, Coord, Stone};
    use crate::game::RuleError;

    #[test]
    fn capture_single() {
        let mut b = Board::<9>::new();
        // 白在 (1,1)，黑围三面后在 (1,0) 补一手提子
        b.set(Coord::new(1, 1), Stone::White);
        b.set(Coord::new(0, 1), Stone::Black);
        b.set(Coord::new(2, 1), Stone::Black);
        b.set(Coord::new(1, 2), Stone::Black);
        let out = try_place(&mut b, Coord::new(1, 0), Stone::Black, None, 9).unwrap();
        assert_eq!(out.captured, vec![Coord::new(1, 1)]);
        assert_eq!(b.get(Coord::new(1, 1)), Some(Stone::Empty));
    }

    #[test]
    fn suicide_rejected() {
        let mut b = Board::<9>::new();
        b.set(Coord::new(0, 1), Stone::White);
        b.set(Coord::new(1, 0), Stone::White);
        // 黑在 (0,0) 自杀（无气且不提子）
        let err = try_place(&mut b, Coord::new(0, 0), Stone::Black, None, 9).unwrap_err();
        assert_eq!(err, RuleError::Suicide);
        // 棋盘应回滚，未落子
        assert_eq!(b.get(Coord::new(0, 0)), Some(Stone::Empty));
    }

    #[test]
    fn suicide_with_capture_allowed() {
        // 关键反例：落子看似自杀但同时提掉对手则允许（提子后获得气）。
        // 白在 (1,0) 且仅剩一气 (1,1)；黑占据其余三面及落子点的侧翼，使 (1,1) 填子后不自杀。
        let mut b = Board::<9>::new();
        b.set(Coord::new(1, 0), Stone::White);
        b.set(Coord::new(0, 0), Stone::Black);
        b.set(Coord::new(2, 0), Stone::Black);
        b.set(Coord::new(0, 1), Stone::Black);
        b.set(Coord::new(2, 1), Stone::Black);
        b.set(Coord::new(1, 2), Stone::Black);
        // 黑在 (1,1) 落子：提白 (1,0)，自身因提子后在 (1,0) 获得气而不自杀
        let out = try_place(&mut b, Coord::new(1, 1), Stone::Black, None, 9).unwrap();
        assert!(out.captured.contains(&Coord::new(1, 0)));
        assert_eq!(b.get(Coord::new(1, 0)), Some(Stone::Empty));
        assert_eq!(b.get(Coord::new(1, 1)), Some(Stone::Black));
    }

    #[test]
    fn occupied_rejected() {
        let mut b = Board::<9>::new();
        b.set(Coord::new(4, 4), Stone::Black);
        assert!(try_place(&mut b, Coord::new(4, 4), Stone::White, None, 9).is_err());
    }

    #[test]
    fn out_of_bounds_rejected() {
        let mut b = Board::<9>::new();
        assert!(try_place(&mut b, Coord::new(9, 0), Stone::Black, None, 9).is_err());
    }

    /// 劫形态：黑白交错「风车」——黑 P=(1,1) 三面邻白，白 Q=(1,2) 仅一气 (1,1)；
    /// 黑填 P 提 Q 后 P 为单子一气 → 记劫点 Q。
    #[test]
    fn ko_shape_detected() {
        let mut b = Board::<9>::new();
        // 黑固定三子：Q=(1,2) 的另外三邻。
        b.set(Coord::new(0, 2), Stone::Black);
        b.set(Coord::new(2, 2), Stone::Black);
        b.set(Coord::new(1, 3), Stone::Black);
        // 白固定三子：P=(1,1) 的另外三邻（互不相连、各有外气）。
        b.set(Coord::new(0, 1), Stone::White);
        b.set(Coord::new(2, 1), Stone::White);
        b.set(Coord::new(1, 0), Stone::White);
        // 白 Q 入住：四邻三黑一空，气=(1,1) 一口。
        b.set(Coord::new(1, 2), Stone::White);
        // 黑 P 落子：提白 Q，P 单子、唯一气是被提空位 → 成劫。
        let out = try_place(&mut b, Coord::new(1, 1), Stone::Black, None, 9).unwrap();
        assert_eq!(out.captured, vec![Coord::new(1, 2)]);
        assert_eq!(out.new_ko, Some(Coord::new(1, 2)));
    }

    /// 劫点落子被拒；且提多子不构成劫。
    #[test]
    fn ko_point_rejected_and_multi_capture_not_ko() {
        let mut b = Board::<9>::new();
        // 提两子的形状不构成劫：白 (0,0)、(0,1) 两子仅共一气 (1,0)……
        // 直接布置成黑一手提两白：白 (1,0)、(1,1)，黑围 (0,0)/(0,1)/(0,2)/(1,2)/(2,1)，黑在 (1,0)??——
        // 用更直接的构造：白 (1,0) 与 (1,1) 连块只有外气 (0,0) 与 (2,0)/(2,1)…
        // 简化：验证劫点拒绝 + 大块提子 new_ko=None 即可。
        b.set(Coord::new(1, 0), Stone::White);
        b.set(Coord::new(1, 1), Stone::White);
        b.set(Coord::new(0, 0), Stone::Black);
        b.set(Coord::new(0, 1), Stone::Black);
        b.set(Coord::new(0, 2), Stone::Black);
        b.set(Coord::new(1, 2), Stone::Black);
        b.set(Coord::new(2, 1), Stone::Black);
        // 黑 (2,0)：白块 (1,0)-(1,1) 气为 (2,0) 一口 → 提两子。
        let out = try_place(&mut b, Coord::new(2, 0), Stone::Black, None, 9).unwrap();
        assert_eq!(out.captured.len(), 2);
        // 提多子不构成劫。
        assert_eq!(out.new_ko, None);

        // 劫点拒绝：把 (2,1) 当劫点，黑在该点落子（有子）→ occupied 先于 ko；
        // 空点劫点检查用干净盘。
        let mut b2 = Board::<9>::new();
        assert_eq!(
            try_place(&mut b2, Coord::new(4, 4), Stone::Black, Some(Coord::new(4, 4)), 9).unwrap_err(),
            RuleError::Ko
        );
    }

    /// 区域计分：角上黑地 + 全盘公气（黑白都接触的空域不计）。
    #[test]
    fn score_area_basic() {
        let mut b = Board::<9>::new();
        // 黑四子封住右上角：(8,8) 的邻 (7,8),(8,7) 均黑 + 两条边 → 孤立空域归黑。
        b.set(Coord::new(6, 8), Stone::Black);
        b.set(Coord::new(7, 8), Stone::Black);
        b.set(Coord::new(8, 7), Stone::Black);
        b.set(Coord::new(7, 7), Stone::Black);
        // 白一子在左下：让全盘其余空点同时接触黑白 → 公气不计。
        b.set(Coord::new(2, 2), Stone::White);
        let s = score_area(&b, &[], KOMI, 9).unwrap();
        assert_eq!(s.black_territory, 1);
        assert_eq!(s.black, 5.0); // 4 子 + 1 地
        assert_eq!(s.white, 8.5); // 1 子 + 7.5
        assert_eq!(s.winner, Stone::White);
    }

    /// 死子移除后计分：白死子从盘面扣除，其原位置成为黑地。
    #[test]
    fn score_area_dead_removed() {
        let mut b = Board::<9>::new();
        // 左上：黑 (1,0),(0,1) 围住 (0,0)；白 (0,0) 是死子。
        b.set(Coord::new(1, 0), Stone::Black);
        b.set(Coord::new(0, 1), Stone::Black);
        b.set(Coord::new(0, 0), Stone::White);
        // 右下白子两颗：让大盘空域成为公气，隔离出干净的局部计分。
        b.set(Coord::new(8, 8), Stone::White);
        b.set(Coord::new(7, 8), Stone::White);
        // 不标死：(0,0) 白占位。黑 2 子 0 地；白 3 子 0 地；其余全公气。
        let live = score_area(&b, &[], KOMI, 9).unwrap();
        assert_eq!(live.black, 2.0);
        assert_eq!(live.white, 10.5);
        // 标死：白子移除，(0,0) 成孤立空域、只接触黑 → 黑地 1。
        let dead = score_area(&b, &[Coord::new(0, 0)], KOMI, 9).unwrap();
        assert_eq!(dead.black, 3.0);
        assert_eq!(dead.white, 9.5);
        assert_eq!(dead.dead_removed, 1);
    }

    /// 公气（黑白都接触的空域）不计入任何一方。
    #[test]
    fn score_area_dame_neutral() {
        let mut b = Board::<9>::new();
        // 黑 (0,0) 与白 (2,0) 对峙，(1,0) 是公气。
        b.set(Coord::new(0, 0), Stone::Black);
        b.set(Coord::new(2, 0), Stone::White);
        let s = score_area(&b, &[], KOMI, 9).unwrap();
        assert_eq!(s.black_territory, 0);
        assert_eq!(s.white_territory, 0);
        assert_eq!(s.black, 1.0);
        assert_eq!(s.white, 8.5);
    }

    /// 死子参数校验：空点/越界拒绝。
    #[test]
    fn score_area_dead_validation() {
        let b = Board::<9>::new();
        assert_eq!(
            score_area(&b, &[Coord::new(0, 0)], KOMI, 9).unwrap_err(),
            RuleError::Other("dead point is empty".into())
        );
        assert_eq!(
            score_area(&b, &[Coord::new(9, 0)], KOMI, 9).unwrap_err(),
            RuleError::OutOfBounds
        );
    }
}
