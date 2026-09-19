//! 五子棋 AI —— figrid-board（Gomocup 参赛引擎）的适配层。
//!
//! 三件事：把 core 的棋盘搬进 figrid 的表示、跑 α-β 搜索、把结果搬回来。
//!
//! 规则对齐：figrid 的 `Board::new()` 默认 `RuleSet::Freestyle`，即无禁手的连五获胜，
//! 与 core 的五子棋（`gomoku::is_five` 只数五连、不判禁手）一致，无需切换规则集。
//!
//! **评估分的符号约定**（胜率折算的前提，已实测确认，不要凭直觉假设）：
//! `SearchResult::score` 是 **negamax 的「轮到走的一方」视角**，不是固定黑方视角。
//! 实测同一局面（黑连四、两端空）黑先时 `score=+999000`、白先时 `score=-998998`；
//! 空盘黑先 `score=+14`。所以折算胜率前必须先看 `board.side_to_move` 与 `my_color`
//! 是否同色——写反了胜率条会整体反相，而且在对称局面上看不出来。
//!
//! 分数量级：`|score| >= 998_000` 是 figrid 的「已算杀」哨兵值（`WIN_SCORE`，
//! 见 vendor 的 search.rs），真实局面的评估分只有百位量级（自我对弈实测
//! 中盘 |score| 中位数约 90、p90 约 141）。logistic 的尺度 s 就是照这个量级标的，
//! 数据见 `crate::odds`。

use crate::AnalyzeResult;
use crate::odds::score_to_win_rate;
use figrid_board::{
    BOARD_SIZE, Board, GOMOKU_NNUE_CONFIG, Searcher, Stone as FgStone, to_idx, to_rc,
};
use goptop_core::board::{Coord, Stone};
use goptop_core::game::{GameState, Move};
use noru::network::NnueWeights;
use std::sync::OnceLock;
use std::time::Duration;

/// 搜索的深度上限。实测 1 秒预算下 native/wasm 都只到 depth 7，20 是安全的封顶
/// （真正生效的限制是时间预算，深度上限只在残局无着可下时兜底）。
const MAX_DEPTH: u32 = 20;

/// v52 NNUE 权重（gzip 1.7MB，解压约 14.3MB），随 vendored crate 分发。
static WEIGHTS_GZ: &[u8] =
    include_bytes!("../vendor/figrid-board/models/gomoku_v52_5stone_conv_93k.bin.gz");

static WEIGHTS: OnceLock<NnueWeights> = OnceLock::new();

/// 权重单例：首次调用解压 + 反序列化（实测约 56ms，之后常驻）。
///
/// 用 `OnceLock` 而非每次重建：14.3MB 的解压与反序列化付一次就够，且 wasm 单线程下
/// 无并发问题。解压失败会 panic——权重是编译期内嵌的常量，解压不出来属于构建错误，
/// 不该在运行时降级掩盖。
fn weights() -> &'static NnueWeights {
    WEIGHTS.get_or_init(|| {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut decoder = GzDecoder::new(WEIGHTS_GZ);
        let mut raw = Vec::with_capacity(15_000_000);
        decoder.read_to_end(&mut raw).expect("解压内嵌 NNUE 权重");
        NnueWeights::load_from_bytes(&raw, Some(GOMOKU_NNUE_CONFIG)).expect("反序列化 NNUE 权重")
    })
}

/// 预热权重。Worker 启动时调用一次，免得第一步棋才付解压开销。
pub fn warmup() {
    let _ = weights();
}

/// 把 `GameState` 的棋子搬进 figrid 的 `Board`。
///
/// **必须按 `state.history` 的落子顺序重放**：`Board::make_move(idx)` 落的是当前
/// `side_to_move` 的颜色，与 idx 本身无关。改成遍历棋盘（`y`/`x` 双循环）落子的话，
/// 落子序与真实行棋序无关，黑白会整片错位——而错位后的局面往往仍有模有样，
/// 只是评估完全跑偏，属于不会报错的静默错误。
///
/// 坐标轴同向也要留意：core 的 `Coord` 是 `(x=列, y=行)`、棋盘语义 `board[y][x]`，
/// 而 figrid 的 `to_idx(row, col)` 算的是 `row * 15 + col`，所以传参是 `(y, x)`。
/// 写反等于沿主对角线镜像：对称局面（比如天元开局）看不出来，非对称局面必错。
fn build_board(state: &GameState) -> Board {
    let mut board = Board::new();
    let mut replayed = false;
    for mv in &state.history {
        match mv {
            Move::Place(c) => {
                board.make_move(to_idx(c.y as usize, c.x as usize));
                replayed = true;
            }
            // core 的五子棋 Pass 只翻手、不落子（UI 不暴露，协议保留以统一棋种）。
            // 这里必须同样翻 figrid 的 side_to_move，否则 Pass 之后的每一手都会
            // 落到错误的颜色上——而不翻手时棋盘看上去完全正常。
            Move::Pass => board.side_to_move = board.side_to_move.opponent(),
            // 认输即刻产生 winner，`crate::analyze` 已在终局时提前返回，走不到这里。
            Move::Resign => {}
        }
    }
    if !replayed {
        replay_by_color(&mut board, state);
    }
    board
}

/// 兜底：`history` 里没有任何 `Place`，但棋盘上有子。core 的正常流程不会产生这种
/// 状态（棋盘与 history 同步演进），只有外部直接构造的 `GameState` 才可能。
///
/// 没有落子序可依赖，只能退化成按遍历序重放。为了颜色不错位，先按颜色分组、
/// 再按「黑先交替」的规则交错落子：这样每一手都恰好落在 `side_to_move` 期望的颜色上，
/// 重放结束时行棋方也自然与 `state.to_move` 对齐。
fn replay_by_color(board: &mut Board, state: &GameState) {
    let mut blacks = Vec::new();
    let mut whites = Vec::new();
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            match state.board.get(Coord::new(x as u8, y as u8)) {
                Some(Stone::Black) => blacks.push(to_idx(y, x)),
                Some(Stone::White) => whites.push(to_idx(y, x)),
                _ => {}
            }
        }
    }
    for (b, w) in blacks.iter().zip(whites.iter()) {
        board.make_move(*b);
        board.make_move(*w);
    }
    // 黑先，所以黑子只可能比白子多一个。
    if blacks.len() > whites.len()
        && let Some(b) = blacks.last()
    {
        board.make_move(*b);
    }
}

pub fn analyze(
    state: &GameState,
    _size: u8,
    my_color: Stone,
    budget_ms: u32,
    want_move: bool,
) -> AnalyzeResult {
    let mut board = build_board(state);
    let started = web_time::Instant::now();
    let result = Searcher::new().search(
        &mut board,
        weights(),
        MAX_DEPTH,
        Some(Duration::from_millis(u64::from(budget_ms))),
    );
    let elapsed_ms = started.elapsed().as_millis() as u64;

    // score 是「轮到走的一方」视角，先归到 my_color 视角才能拿去换算胜率。
    let stm = match board.side_to_move {
        FgStone::Black => Stone::Black,
        FgStone::White => Stone::White,
    };
    // 重放 history 的结果与 GameState::to_move 是同一个真源的两份表示，必须一致；
    // 一旦漂移，胜率方向整体反相而数值照样「合理」，没有任何其他迹象。
    debug_assert_eq!(
        stm, state.to_move,
        "重放 history 后的行棋方与 GameState::to_move 不一致"
    );
    let win_rate = if my_color == stm {
        score_to_win_rate(result.score)
    } else {
        1.0 - score_to_win_rate(result.score)
    };

    AnalyzeResult {
        // want_move=false 同样走完整搜索：胜率也依赖搜索精度，退回静态评估会
        // 让纯分析模式（观战/复盘）的胜率与对战模式对不上。只是丢弃选点。
        best_move: if want_move {
            result.best_move.map(|idx| {
                let (y, x) = to_rc(idx);
                (x as u8, y as u8)
            })
        } else {
            None
        },
        win_rate,
        depth: result.depth,
        nodes: result.nodes,
        elapsed_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalyzeRequest;
    use goptop_core::game::GameKind;

    /// 测试统一用 1 秒预算：debug 构建下 α-β 只到 depth 3~4（release 到 7），
    /// 但战术题在 depth 3 已能看清（成五/挡四都是 1~2 层内的事）。
    const BUDGET_MS: u32 = 1000;

    /// 按 `(x, y)` 交替落子构造局面，走的是与生产代码同一条 history 重放路径。
    fn state_from(moves: &[(u8, u8)]) -> GameState {
        let mut st = GameState::new(GameKind::default_gomoku());
        for &(x, y) in moves {
            st.try_play(Move::Place(Coord::new(x, y))).expect("构造局面的落子必须合法");
        }
        st
    }

    fn analyze_best_move(st: &GameState, my_color: Stone, budget_ms: u32) -> Option<(u8, u8)> {
        let req = AnalyzeRequest {
            state: st.clone(),
            my_color,
            budget_ms,
            want_move: true,
        };
        crate::analyze(&req).best_move
    }

    #[test]
    fn empty_board_plays_tengen() {
        let st = state_from(&[]);
        assert_eq!(st.to_move, Stone::Black);
        assert_eq!(analyze_best_move(&st, Stone::Black, BUDGET_MS), Some((7, 7)));
    }

    #[test]
    fn blocks_black_open_three() {
        // 黑 (7,7)(8,7)(9,7) 活三，轮到白：不挡两端之一黑下一手就是活四。
        let st = state_from(&[(7, 7), (0, 0), (8, 7), (0, 2), (9, 7)]);
        assert_eq!(st.to_move, Stone::White);
        let mv = analyze_best_move(&st, Stone::White, BUDGET_MS);
        assert!(matches!(mv, Some((6, 7)) | Some((10, 7))), "白应挡活三，实际 {mv:?}");
    }

    #[test]
    fn black_completes_four_into_five() {
        // 黑 (7,7)(8,7)(9,7)(10,7) 活四（两端皆空），轮到黑：两端任一处都成五。
        let st = state_from(&[
            (7, 7),
            (2, 5),
            (8, 7),
            (9, 14),
            (9, 7),
            (3, 11),
            (10, 7),
            (5, 1),
        ]);
        assert_eq!(st.to_move, Stone::Black);
        let mv = analyze_best_move(&st, Stone::Black, BUDGET_MS);
        assert!(matches!(mv, Some((6, 7)) | Some((11, 7))), "黑应连五，实际 {mv:?}");
    }

    #[test]
    fn white_must_block_black_four() {
        // 黑冲四、左端 (6,7) 已被白占死，轮到白：唯一不成五的应手就是 (11,7)，
        // 这是死局里唯一「不是立刻输」的一手，比活四更能钉住挡点。
        let st = state_from(&[(7, 7), (6, 7), (8, 7), (0, 0), (9, 7), (2, 5), (10, 7)]);
        assert_eq!(st.to_move, Stone::White);
        assert_eq!(analyze_best_move(&st, Stone::White, BUDGET_MS), Some((11, 7)));
    }

    /// 胜率必须以 my_color 为准，而不是以「谁先走」为准。
    ///
    /// 刻意用必杀局（黑活四两端空、轮白）：两边的 score 都会饱和到 figrid 的
    /// 哨兵分，胜率恒为 0.0 / 1.0，不受搜索深度抖动影响，因此能精确断言。
    ///
    /// 只测「两视角互补」是不够的：把 `my_color == stm` 的判据整个写反，
    /// 两边同时变成 `1 - p`，互补性照样成立。真正抓这个 bug 的是方向断言
    /// （占优方必须 > 0.9）——所以下面两个 assert 缺一不可。
    #[test]
    fn win_rate_follows_my_color() {
        let st = state_from(&[
            (7, 7),
            (2, 5),
            (8, 7),
            (9, 14),
            (9, 7),
            (3, 11),
            (10, 7),
        ]);
        assert_eq!(st.to_move, Stone::White);
        let mk = |color| {
            crate::analyze(&AnalyzeRequest {
                state: st.clone(),
                my_color: color,
                budget_ms: BUDGET_MS,
                want_move: false,
            })
            .win_rate
        };
        let (black, white) = (mk(Stone::Black), mk(Stone::White));
        assert!(
            (black + white - 1.0).abs() < 1e-9,
            "视角胜率应互补，黑={black} 白={white}"
        );
        assert!(black > 0.9, "黑手握活四是必胜，黑方胜率应贴近 1，实际 {black}");
        assert!(white < 0.1, "白无法阻止黑连五，白方胜率应贴近 0，实际 {white}");
    }

    /// 纯胜率模式：丢弃选点，但胜率仍来自真实搜索（不是静态评估的退化值）。
    ///
    /// 这里换成「黑活三、白先」这类未分胜负的局面：边界只做宽松的方向断言，
    /// 因为时间预算下的搜索深度会抖动，score 因此不是逐位可复现的。
    #[test]
    fn want_move_false_still_searches() {
        let st = state_from(&[(7, 7), (0, 0), (8, 7), (0, 2), (9, 7)]);
        assert_eq!(st.to_move, Stone::White);
        let r = crate::analyze(&AnalyzeRequest {
            state: st.clone(),
            my_color: Stone::White,
            budget_ms: BUDGET_MS,
            want_move: false,
        });
        assert_eq!(r.best_move, None, "want_move=false 不该返回选点");
        assert!(r.depth > 0, "want_move=false 也应真的搜索过");
        assert!(r.win_rate < 0.45, "白方要应付黑的活三，胜率应低于均势，实际 {}", r.win_rate);
    }

    /// 输入侧坐标映射：core 的 `(x=列, y=行)` 送进 figrid 时必须写成 `to_idx(y, x)`。
    ///
    /// 用一个**不对称**图案把转置钉死：黑 (2,5)、白 (11,3)。正确映射下黑落在
    /// `to_idx(5, 2) = 77`、白落在 `to_idx(3, 11) = 56`；把传参写成 `to_idx(x, y)`
    /// 时黑会落到 35、白落到 168，四个断言同时炸。
    ///
    /// 必须断言具体 idx 而不是「局面看着对不对」：转置是棋盘沿主对角线的镜像，
    /// 天元开局、横四这类对称局面镜像后完全一样，只有 idx 能分辨。
    #[test]
    fn board_mapping_is_not_transposed() {
        let st = state_from(&[(2, 5), (11, 3)]);
        let fb = build_board(&st);
        assert_eq!(fb.black.count_ones(), 1, "黑应恰好一子");
        assert_eq!(fb.white.count_ones(), 1, "白应恰好一子");
        assert!(fb.black.get(to_idx(5, 2)), "黑子应在 idx {}", to_idx(5, 2));
        assert!(!fb.black.get(to_idx(2, 5)), "黑子不该出现在转置后的 idx");
        assert!(fb.white.get(to_idx(3, 11)), "白子应在 idx {}", to_idx(3, 11));
        assert!(!fb.white.get(to_idx(11, 3)), "白子不该出现在转置后的 idx");
        // 两手过后轮黑，必须与 GameState::to_move 对齐。
        assert_eq!(fb.side_to_move, FgStone::Black);
        assert_eq!(st.to_move, Stone::Black);
    }

    /// 输出侧坐标映射：figrid 的 idx 换回 core 坐标后，必须能在 core 棋盘上真的落子。
    ///
    /// 与上一个测试正交——那个钉输入、这个钉输出。只钉输入的话，「两侧同时转置」
    /// 仍然全绿（一致转置是棋盘的对角镜像，局面上不可分辨）。
    /// 这里把 AI 给的坐标原样喂回 core 并检查胜负，不一致的映射会立刻现形。
    #[test]
    fn best_move_is_valid_on_core_board() {
        // 黑 (3,11)(4,11)(5,11)(6,11) 横向冲四，左端 (2,11) 已被白占死：
        // 唯一成五点是 (7,11)。换成转置坐标 (11,7) 在 core 上什么也不是。
        // 8 手、白收在第 8 手之后轮黑（黑 4 子白 4 子，黑先）。
        let st = state_from(&[
            (3, 11), (2, 11), (4, 11), (0, 0), (5, 11), (0, 2), (6, 11), (0, 4),
        ]);
        assert_eq!(st.to_move, Stone::Black);
        let mv = analyze_best_move(&st, Stone::Black, BUDGET_MS).expect("应有选点");
        assert_eq!(mv, (7, 11), "应连五于 (7,11)");
        let mut after = st.clone();
        let eff = after
            .try_play(Move::Place(Coord::new(mv.0, mv.1)))
            .expect("AI 选点必须在 core 棋盘上合法");
        assert_eq!(eff.winner, Some(Stone::Black), "AI 选点 {mv:?} 在 core 上不成五");
    }

    /// 视角折算的互补性，在一个**未分出胜负**（分数不饱和）的局面上验证。
    ///
    /// 与 `win_rate_follows_my_color` 互补：那一题靠必杀分的饱和保证两次调用
    /// 逐位一致，只能验方向；这一题验的是 logistic 分支本身在中间段的正确性，
    /// 代价是两次独立搜索的分数会抖，所以容差放宽到 0.15，并另加方向断言兜底。
    #[test]
    fn win_rate_viewpoint_complements_on_quiet_position() {
        let st = state_from(&[(7, 7), (0, 0), (8, 7), (0, 2), (9, 7)]);
        assert_eq!(st.to_move, Stone::White, "黑活三、轮到白");
        let mk = |color| {
            crate::analyze(&AnalyzeRequest {
                state: st.clone(),
                my_color: color,
                budget_ms: BUDGET_MS,
                want_move: false,
            })
            .win_rate
        };
        let (black, white) = (mk(Stone::Black), mk(Stone::White));
        assert!((black + white - 1.0).abs() < 0.15, "视角胜率应大致互补，黑={black} 白={white}");
        // 黑活三、白必须应：黑方胜率应高于均势，白方低于均势。
        // 若 `score` 被当成固定黑方视角（而非「轮到走的一方」视角），
        // 这一对方向断言会反过来，是真能区分对错的判据。
        assert!(black > 0.55, "黑握活三应占优，实际 {black}");
        assert!(white < 0.45, "白需应对活三应劣势，实际 {white}");
    }

    /// 时间预算必须有界。
    ///
    /// figrid 的根 VCT 有**独立**于主搜索的预算（`time_limit / 8`，夹在
    /// 100ms..2000ms），且在主搜索之前跑完；`elapsed_ms` 是适配层实测值，
    /// 所以真实上界是 `budget + clamp(budget/8, 100, 2000)`，不是 `budget`。
    ///
    /// **release 实测**（本机，单独跑）：预算 200/600/1500ms → 实耗
    /// 236/608/1508ms，其中 200ms 那次含权重首次解压约 56ms。**debug 构建会明显
    /// 超支**（600ms 预算实测 629~846ms）：节点代价被放大后，deadline 的检查间隔
    /// 就摊得开了——这是构建档位的性质，不是时间控制写错。断言留 500ms 余量，
    /// 在两种档位下都不假阳，但仍能抓住成倍的超支。
    #[test]
    fn time_budget_is_respected() {
        let st = state_from(&[(7, 7), (0, 0), (8, 7), (0, 2), (9, 7)]);
        let budget = 600u32;
        let r = crate::analyze(&AnalyzeRequest {
            state: st,
            my_color: Stone::White,
            budget_ms: budget,
            want_move: true,
        });
        let ceiling = u64::from(budget) + u64::from(budget) / 8 + 500;
        println!("budget={budget}ms elapsed={}ms depth={} nodes={}", r.elapsed_ms, r.depth, r.nodes);
        assert!(
            r.elapsed_ms <= ceiling,
            "预算 {budget}ms 实际 {}ms，超过上界 {ceiling}ms",
            r.elapsed_ms
        );
    }
}
