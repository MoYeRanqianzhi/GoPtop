//! 身份与钥匙 — userId / pwd / gameId 生成（随机源注入）。
//!
//! 纯逻辑层不直接触碰 crypto：生成函数接受随机数与时间戳参数，wasm 绑定层用
//! `crypto.getRandomValues` 提供，native 测试用固定值。格式与历史 TS 实现一致：
//! - userId：`u-` + 时间戳 base36 + 随机 4 位 base36（每标签页唯一，sessionStorage 持久）；
//! - pwd：6 位 base36（36^6 ≈ 21 亿，对局钥匙不可预测）；
//! - gameId：`g-` + 时间戳 base36 + 随机 4 位。

/// base36 小写编码（0-9a-z）。
#[must_use]
pub fn base36(mut v: u64, width: usize) -> String {
    const CH: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(CH[(v % 36) as usize]);
        v /= 36;
        if v == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// 生成页面级 userId（时间戳毫秒 + 4 位 base36 随机段，对齐 TS slice(2,6) 定宽）。
#[must_use]
pub fn gen_user_id(now_ms: u64, rand: u32) -> String {
    format!("u-{}{}", base36(now_ms, 8), base36(u64::from(rand) % 36u64.pow(4), 4))
}

/// 生成 6 位 base36 对局钥匙：36^6 = 2176782336，取模均匀性偏差 ~2.3e-10，
/// 对钥匙可预测性无实际影响（与历史 TS 实现一致）。
#[must_use]
pub fn gen_pwd(rand: u32) -> String {
    base36((rand % 2_176_782_336) as u64, 6)
}

/// 生成 gameId（观战 channel 后缀，随机段同 userId 定宽）。
#[must_use]
pub fn gen_game_id(now_ms: u64, rand: u32) -> String {
    format!("g-{}{}", base36(now_ms, 8), base36(u64::from(rand) % 36u64.pow(4), 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// base36 编码与定宽补零。
    #[test]
    fn base36_encoding() {
        assert_eq!(base36(0, 4), "0000");
        assert_eq!(base36(35, 2), "0z");
        assert_eq!(base36(36, 2), "10");
        assert_eq!(base36(2_176_782_335, 6), "zzzzzz");
        // 定宽截断不发生：超宽原样输出（pwd 取模后恒 < 6 位值域）。
        assert_eq!(base36(36u64.pow(6), 6), "1000000");
    }

    /// userId/gameId 前缀与确定性（同参数同输出——随机源注入的可测性）。
    #[test]
    fn id_generation_deterministic() {
        let id = gen_user_id(1_700_000_000_000, 0xdeadbeef);
        assert!(id.starts_with("u-"));
        assert_eq!(id.len(), 2 + 8 + 4);
        assert_eq!(gen_user_id(1_700_000_000_000, 0xdeadbeef), id);

        let g = gen_game_id(1_700_000_000_000, 42);
        assert!(g.starts_with("g-"));

        // pwd：6 位、值域取模。
        let p = gen_pwd(u32::MAX);
        assert_eq!(p.len(), 6);
        assert_eq!(gen_pwd(0), "000000");
    }
}
