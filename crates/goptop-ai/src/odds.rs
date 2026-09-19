//! 胜率映射 —— 把各引擎的搜索输出折算成 0..1 的胜率。
//!
//! 两个棋种的来源不同：
//! - **围棋**：MCTS 根节点的访问统计天然就是胜率估计，可直接用；
//! - **五子棋**：NNUE 输出的是评估分，不是胜率，需要 logistic 映射
//!   `P = 1 / (1 + e^(-x/s))`，尺度 `s` 必须针对 figrid 的评估尺度标定（见下）。
//!
//! 标定不能拍脑袋：`s` 取错会让胜率条在均势时贴边或在中盘不动，看起来像坏了。
//!
//! # x 的视角（最容易搞错的地方，已实测）
//!
//! `score` 是 **negamax 的「轮到走的一方」视角**，不是固定黑方视角。实测同一局面
//! （黑 (7,7)(8,7)(9,7)(10,7) 连四、两端空）：黑先时 `score=+999000`，
//! 白先时 `score=-998998`。所以本函数只负责「分数 → 该方胜率」，
//! **调用方必须先把它归到自己要的那一方**（`gomoku::analyze` 负责这一步：
//! score 的归属方是 `board.side_to_move`，与 `my_color` 不同色时取 `1 - P`）。
//!
//! # 分数尺度与 s 的标定
//!
//! figrid 的分数量级不是国际象棋的 centipawn：`|score| >= 998_000` 是「已算杀」的
//! 哨兵值（vendor 的 `WIN_SCORE = 999_000`），真实局面的评估分只有百位量级。
//! 用自我对弈（双方各 400ms/手，连下 40 手直到分出胜负）采样真实中盘分数，
//! 非哨兵样本 n=42：min 13 / p25 66 / p50 90 / p75 104 / p90 141，最大值 141 之后
//! 下一手就跳到了 999000 的必杀分。
//!
//! 取 `s = 150`，实测各局的落点：
//!
//! | 局面 | score（stm 视角） | P(stm) |
//! |---|---|---|
//! | 空盘、黑先 | +14 | 0.523 |
//! | 天元后、白先 | -11 | 0.482 |
//! | 中盘均势（自我对弈 p50） | ±90 | 0.354 / 0.646 |
//! | 黑活三、白先 | -194 | 0.215 |
//! | 黑冲四一端已堵、白先 | -178 | 0.234 |
//! | 黑连四两端空、黑先（必杀） | +999000 | 1.000 |
//! | 黑连四两端空、白先（必败） | -998998 | 0.000 |
//!
//! 即：均势 ≈ 0.5，中盘优劣能明显推动胜率条（±90 分就走到 0.35/0.65），
//! 已成四的必杀分饱和到端点。`s` 再小会让中盘频繁贴边，再大则整条中盘挤在 0.5 附近。

/// logistic 的尺度常数：score 偏离 0 每 150 分，胜率赔率变化 e 倍。
///
/// 这个数直接决定胜率条的手感，改动前先重跑 `gomoku` 里的自我对弈标定，
/// 别只凭观感调。标定数据见文件头注释。
pub const SCORE_SCALE: f64 = 150.0;

/// figrid 评估分 → 该「轮到走的一方」的胜率。
///
/// 入参视角由调用方保证：本函数不碰棋色，换个视角请自行取 `1 - P`。
pub fn score_to_win_rate(score: i32) -> f64 {
    let p = 1.0 / (1.0 + (-(f64::from(score) / SCORE_SCALE)).exp());
    // 数学上 logistic 的值域已经是开区间 (0,1)，这里 clamp 是把「返回 [0,1]」
    // 钉成对外契约：调用方（胜率条、序列化、跨语言边界）不必再自己兜一次。
    p.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表驱动钉住文件头里那份标定表。数值变了说明 `SCORE_SCALE` 或公式被动过。
    #[test]
    fn calibration_table() {
        for (score, want) in [
            (0, 0.5),
            (14, 0.5233),
            (-11, 0.4817),
            (90, 0.6457),
            (-178, 0.2339),
            (-194, 0.2153),
            (SCORE_SCALE as i32, 0.7311),
            (2 * SCORE_SCALE as i32, 0.8808),
        ] {
            let got = score_to_win_rate(score);
            assert!((got - want).abs() < 5e-4, "score={score} 期望 {want}，实际 {got}");
        }
    }

    /// 已成四这类必杀分必须落到端点，不能被 logistic 拉回中间。
    #[test]
    fn decided_scores_saturate() {
        assert_eq!(score_to_win_rate(999_000), 1.0);
        assert_eq!(score_to_win_rate(-998_998), 0.0);
        assert_eq!(score_to_win_rate(i32::MAX), 1.0);
        assert_eq!(score_to_win_rate(i32::MIN), 0.0);
    }

    /// 单调不减，且恒落在 [0,1]。
    #[test]
    fn monotonic_and_bounded() {
        let mut prev = f64::NEG_INFINITY;
        for score in (-3000..=3000).step_by(7) {
            let p = score_to_win_rate(score);
            assert!((0.0..=1.0).contains(&p), "score={score} 越界: {p}");
            assert!(p >= prev, "score={score} 破坏单调性: {prev} -> {p}");
            prev = p;
        }
    }

    /// 反对称：换到对面视角正好互补。
    #[test]
    fn symmetric_under_viewpoint_swap() {
        for score in [0, 1, 37, 150, 999, 12_345] {
            let sum = score_to_win_rate(score) + score_to_win_rate(-score);
            assert!((sum - 1.0).abs() < 1e-12, "score={score} 互补性被破坏: {sum}");
        }
    }
}
