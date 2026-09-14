//! G1 信令编码 — offer/answer 的压缩 + 混淆 + URL 安全编码。
//!
//! 格式（与历史 TS 实现 `frontend/src/net/rtc.ts` 双向互通）：
//! `"G1" + b64url( xor( deflate_raw( utf8(json) ), keystream(pwd) ) )`
//! - 压缩：标准 raw deflate 流（无 zlib 头）。TS 侧曾用浏览器 CompressionStream，
//!   Rust 侧用 flate2(miniz_oxide)——两者产物字节可不同但都合法互通；Rust 侧同时
//!   承担压缩与解压后，解码不再依赖 DecompressionStream（消除了旧内核兼容问题）。
//! - 混淆：pwd 派生的重复密钥流逐字节 XOR——链接里不出现可读 SDP（含本机 IP）。
//!   fnv1a 双散列扩展成 4 字节步进避免同钥周期性；这是混淆级而非密码学级
//!   （pwd 本就在同一链接里），真正的传输安全由 WebRTC DTLS 端到端加密保证。
//! - base64：URL 安全字母表 + 去填充。

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

/// 版本头：未来换编码格式时可平滑迁移。
pub const RTC_ENC_MAGIC: &str = "G1";

/// 编码错误。
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unknown rtc token")]
    UnknownToken,
    #[error("bad base64url")]
    BadBase64,
    #[error("deflate failure")]
    Deflate,
    #[error("inflate failure")]
    Inflate,
    #[error("bad utf8")]
    BadUtf8,
}

/// 由短钥匙派生重复密钥流：fnv1a 双散列扩展成 4 字节步进（与 TS keyStream 逐位一致：
/// Math.imul = u32 wrapping_mul，`>>> n` = u32 逻辑右移）。
fn key_stream(pwd: &str, len: usize) -> Vec<u8> {
    let mut h1: u32 = 0x811c_9dc5;
    let mut h2: u32 = 0x1b87_3593;
    for &b in pwd.as_bytes() {
        h1 = (h1 ^ b as u32).wrapping_mul(0x0100_0193);
        h2 = h2.wrapping_add(b as u32).wrapping_mul(0x85eb_ca6b);
    }
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        h1 = h1.wrapping_mul(0x0100_0193) ^ (h1 >> 15);
        h2 = h2.wrapping_mul(0x85eb_ca6b) ^ (h2 >> 13);
        out.push((h1 ^ h2) as u8);
    }
    out
}

fn xor_bytes(bytes: &[u8], pwd: &str) -> Vec<u8> {
    let ks = key_stream(pwd, bytes.len());
    bytes.iter().zip(ks).map(|(b, k)| b ^ k).collect()
}

fn deflate_raw(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut c = Compress::new(Compression::new(6), false);
    // 压缩上界 = 输入 + 5 字节/64KB 余量（deflate 规范上界），一轮出结果。
    let mut out = vec![0u8; bytes.len() + bytes.len() / 8 + 64];
    let mut consumed = 0usize;
    let mut produced = 0usize;
    while consumed < bytes.len() {
        let before_in = c.total_in() as usize;
        let before_out = c.total_out() as usize;
        let status = c
            .compress(&bytes[consumed..], &mut out[produced..], FlushCompress::Finish)
            .map_err(|_| CodecError::Deflate)?;
        consumed += c.total_in() as usize - before_in;
        produced += c.total_out() as usize - before_out;
        if matches!(status, Status::StreamEnd) && consumed >= bytes.len() {
            break;
        }
        // 输出缓冲不足（理论不可达）：扩容重试。
        out.resize(out.len() * 2, 0);
    }
    out.truncate(produced);
    Ok(out)
}

fn inflate_raw(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut d = Decompress::new(false);
    let mut out = Vec::with_capacity(bytes.len() * 8 + 256);
    loop {
        let before_in = d.total_in() as usize;
        let before_out = d.total_out() as usize;
        if out.len() == before_out {
            out.resize(out.len() * 2 + 256, 0);
        }
        let status = d
            .decompress(&bytes[before_in.min(bytes.len())..], &mut out[before_out..], FlushDecompress::Finish)
            .map_err(|_| CodecError::Inflate)?;
        let new_in = d.total_in() as usize;
        let new_out = d.total_out() as usize;
        out.truncate(new_out);
        if matches!(status, Status::StreamEnd) {
            return Ok(out);
        }
        // 输入耗尽但流未结束：截断流——Finish 对非法结尾会 Err，这里防死循环直接报错。
        if new_in >= bytes.len() {
            return Err(CodecError::Inflate);
        }
    }
}

/// URL 安全 base64 编码（无填充；62='-' 63='_'，对齐 TS 侧 btoa 后替换）。
#[must_use]
pub fn b64url_encode(bytes: &[u8]) -> String {
    const URL_SAFE: [char; 64] = [
        'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
        'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j',
        'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1',
        '2', '3', '4', '5', '6', '7', '8', '9', '-', '_',
    ];
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(URL_SAFE[(n >> 18) as usize & 63]);
        out.push(URL_SAFE[(n >> 12) as usize & 63]);
        if chunk.len() > 1 {
            out.push(URL_SAFE[(n >> 6) as usize & 63]);
        }
        if chunk.len() > 2 {
            out.push(URL_SAFE[n as usize & 63]);
        }
    }
    out
}

/// URL 安全 base64 解码（容忍缺失填充，拒绝非法字符）。
pub fn b64url_decode(s: &str) -> Result<Vec<u8>, CodecError> {
    fn val(c: u8) -> Result<u32, CodecError> {
        match c {
            b'A'..=b'Z' => Ok(u32::from(c - b'A')),
            b'a'..=b'z' => Ok(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Ok(u32::from(c - b'0') + 52),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            _ => Err(CodecError::BadBase64),
        }
    }
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// 编码 offer/answer：JSON → deflate-raw → pwd XOR → URL 安全 base64（G1 头）。
pub fn encode(payload_json: &str, pwd: &str) -> Result<String, CodecError> {
    let deflated = deflate_raw(payload_json.as_bytes())?;
    let encrypted = xor_bytes(&deflated, pwd);
    Ok(format!("{RTC_ENC_MAGIC}{}", b64url_encode(&encrypted)))
}

/// 解码 offer/answer（[`encode`] 的逆）。token 允许首尾空白。
pub fn decode(token: &str, pwd: &str) -> Result<String, CodecError> {
    let t = token.trim();
    let Some(body) = t.strip_prefix(RTC_ENC_MAGIC) else {
        return Err(CodecError::UnknownToken);
    };
    let encrypted = b64url_decode(body)?;
    let deflated = xor_bytes(&encrypted, pwd);
    let raw = inflate_raw(&deflated)?;
    String::from_utf8(raw).map_err(|_| CodecError::BadUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与历史 TS 实现（浏览器 CompressionStream + keyStream）的互通向量：
    /// token 由 node 22 以与 TS 逐位一致的实现生成。
    #[test]
    fn interop_with_ts_token() {
        const TOKEN: &str = "G1cRwS-sZMHRAYyazUtmPYA4N6zuk9Cxhzllx14_MJguNs7gyKP8TlhXLDbF_ndZ_HKW8JZw";
        let json = decode(TOKEN, "abc123").unwrap();
        assert_eq!(json, r#"{"s":"v=0 test-sdp-line","t":"offer","r":"player"}"#);
    }

    /// 往返：encode 后 decode 还原（含中文/特殊字符的 SDP 形态文本）。
    #[test]
    fn roundtrip() {
        let payload = r#"{"s":"m=application 9 UDP/DTLS/SCTP webrtc-dataChannel\r\na=ice-ufrag:中文测试","t":"answer","r":"spectator"}"#;
        let token = encode(payload, "p1w2e3").unwrap();
        assert!(token.starts_with("G1"));
        // URL 安全：不含 + / =（encode 后再次 URL 编码也安全）。
        assert!(!token.contains('+') && !token.contains('/') && !token.contains('='));
        assert_eq!(decode(&token, "p1w2e3").unwrap(), payload);
    }

    /// 错钥/坏头/截断：稳定错误（TS 侧以 unknown rtc token 单独提示）。
    #[test]
    fn error_paths() {
        let token = encode(r#"{"a":1}"#, "right").unwrap();
        assert!(matches!(decode(&token, "wrong"), Err(CodecError::Inflate | CodecError::BadUtf8)));
        assert!(matches!(decode("XXabc", "right"), Err(CodecError::UnknownToken)));
        assert!(matches!(decode("G1!!!", "right"), Err(CodecError::BadBase64)));
    }

    /// b64url 与标准实现的兼容性：解码容忍标准字母表 + 填充。
    #[test]
    fn b64url_padding_tolerant() {
        assert_eq!(b64url_decode(b64url_encode(b"any-carnal-pleasure").as_str()).unwrap(), b"any-carnal-pleasure");
        assert_eq!(b64url_decode("YWJj").unwrap(), b"abc");
        assert_eq!(b64url_decode("YQ==").unwrap(), b"a");
    }
}
