//! 链接模型 — 邀请/回执/观战/主页链接的构造与解析（路径风格 + 旧 query 兼容）。
//!
//! 行为对齐历史 `frontend/src/net/links.ts`（对齐依据：其 vitest 用例全部在
//! 本模块单测中复刻）：
//! - `/` 菜单；`/local` `/p2p` `/users` `/settings` 静态页；
//! - `/<userId>?pwd=&kind=&size=[&rtc=]` 邀请；`/<userId>?pwd=&spec=1` 观战（spec 链接，
//!   pwd 为观战钥匙整局有效）；`/<userId>` 主页；`/watch/<game>` 同源观战；
//! - 旧 query：`?room=` 观战房间、`?u=` 用户主页、`?watch=` 观战；
//! - 解析与域名无关：粘贴文本只取路径与查询参数，任意域名都能识别；
//! - 回执链接 `?rtcAns=` 由 [`parse_pasted_answer`] 单独解析。
//!
//! `pasted` 语义差异（与 TS 一致）：浏览器地址栏解析（parseUrl）的单段路径一律视为
//! 用户主页；粘贴解析（parsePastedLink）收紧——单段路径必须像用户链接（u- 前缀或
//! 带 pwd/rtc 参数），否则视为普通文本/陌生网址，防止粘贴任意内容被误判成挑战入口。
//!
//! **刻意不用 `url` crate**：它按 RFC 3986 归一化路径里的 `%2F`→`/`、`%3F`→`?`，
//! 而 WHATWG URL（浏览器/TS 行为）在 pathname 中原样保留这些转义——归一化会破坏
//! 含 `/ ? &` 的 userId 往返。这里手写最小切分：path 原样切分逐段解码，query 按
//! form-urlencoded 解码（`+`→空格）。

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// URL 意图（TS UrlIntent）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlIntent {
    Menu,
    Local,
    P2p,
    Users,
    Settings,
    /// 用户主页 / 邀请 / 观战链接。`spec=1` 表示观战（pwd 为观战钥匙，整局有效）；
    /// `rtc` 为无服务器跨设备信令（加密 offer）；服务器模式不带 rtc，pwd 经服务器
    /// 交互校验（错误转弹窗询问）。
    User {
        user_id: String,
        pwd: Option<String>,
        kind: String,
        size: u16,
        rtc: Option<String>,
        spec: bool,
    },
    Watch {
        game_id: String,
    },
}

/// 回执参数（TS parsePastedAnswer 返回）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerIntent {
    pub inviter_id: String,
    pub pwd: String,
    pub rtc_ans: String,
    /// spec=1 观战回执（回执类型由链接属性自动判断）。
    pub spectator: bool,
    pub game_id: Option<String>,
    pub kind: Option<String>,
    pub size: Option<u16>,
}

/// form 编码集合（URLSearchParams.set 口径）：除 `A-Za-z0-9*-._` 外全编码；空格特判为 `+`。
const FORM_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'*').remove(b'-').remove(b'.').remove(b'_');

/// encodeURIComponent 口径：不编码 `A-Za-z0-9-_.!~*'()`；`%` 在 [`encode_path_segment`] 先行转义。
const PATH_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*')
    .remove(b'\'').remove(b'(').remove(b')');

/* ---------------- 最小 URL 切分与解码原语 ---------------- */

/// 从粘贴文本/地址栏 href 提取 `(path, query_pairs)`。
/// - 无协议补 `https://`（TS pasted 分支）；
/// - `#fragment` 全弃（TS new URL 同样不进 searchParams）；
/// - path 不做整体解码（浏览器 pathname 原样保留 %XX），由调用方逐段解码。
fn split_href(text: &str) -> Option<(String, Vec<(String, String)>)> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    // 剥 fragment。
    let no_frag = raw.split('#').next().unwrap_or(raw);
    // scheme 判定：TS `new URL` 需要协议（无协议的 pasted 补 https）。
    let has_scheme = no_frag.contains("://");
    let body = if has_scheme {
        let idx = no_frag.find("://")? + 3;
        &no_frag[idx..]
    } else {
        no_frag
    };
    // authority = 第一个 '/' 之前；path 从第一个 '/' 起。
    let (path, query) = match body.find(['?', '/']) {
        None => ("", ""),
        Some(i) if body.as_bytes()[i] == b'?' => ("", &body[i + 1..]),
        Some(i) => {
            let rest = &body[i..];
            match rest.find('?') {
                None => (rest, ""),
                Some(q) => (&rest[..q], &rest[q + 1..]),
            }
        }
    };
    let pairs = if query.is_empty() {
        Vec::new()
    } else {
        query
            .split('&')
            .filter(|kv| !kv.is_empty())
            .map(|kv| match kv.split_once('=') {
                Some((k, v)) => (decode_form(k), decode_form(v)),
                None => (decode_form(kv), String::new()),
            })
            .collect()
    };
    Some((path.to_string(), pairs))
}

/// form-urlencoded 解码（searchParams 口径）：`+`→空格，`%XX` 十六进制，非法序列按
/// 字面保留（TS TextDecoder 不抛，只有畸形 % 在 decodeURIComponent 抛）。
fn decode_form(s: &str) -> String {
    let plus = s.replace('+', " ");
    percent_decode_str(&plus).decode_utf8_lossy().into_owned()
}

/// decodeURIComponent 口径：`+` 不转空格；全部 `%` 必须是合法转义（对齐 TS——
/// 畸形序列抛 URIError，被上层吞成 menu/null）。
fn decode_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            if !hex.iter().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    percent_decode_str(s).decode_utf8().ok().map(std::borrow::Cow::into_owned)
}

/// encodeURIComponent 等价（PATH_SAFE 含 `%`，无需先行转义）。
fn encode_path_segment(s: &str) -> String {
    utf8_percent_encode(s, PATH_SAFE).to_string()
}

/// URLSearchParams.set 等价（form 编码 + 空格→+）。
fn encode_form_value(s: &str) -> String {
    let enc: String = utf8_percent_encode(s, FORM_SAFE).to_string();
    enc.replace("%20", "+")
}

/* ---------------- 解析 ---------------- */

/// 从任意文本解析意图（粘贴用：无协议补 https、单段路径收紧）。
/// 失败返回 None（TS parsePastedLink 的 catch 兜底）。
#[must_use]
pub fn parse_pasted_link(text: &str) -> Option<UrlIntent> {
    parse_intent_inner(text, true)
}

/// 解析当前页面地址（TS parseUrl）。失败回退 Menu（TS catch 兜底）。
#[must_use]
pub fn parse_url(href: &str) -> UrlIntent {
    parse_intent_inner(href, false).unwrap_or(UrlIntent::Menu)
}

fn kind_size_from_params(pairs: &[(String, String)]) -> (String, u16) {
    let kind = pairs
        .iter()
        .find(|(k, _)| k == "kind")
        .map(|(_, v)| v.clone())
        .filter(|v| v == "go")
        .unwrap_or_else(|| "gomoku".to_string());
    let size_raw: Option<u16> = pairs
        .iter()
        .find(|(k, _)| k == "size")
        .and_then(|(_, v)| v.parse().ok());
    let size = if matches!(size_raw, Some(9 | 13 | 15 | 19)) {
        size_raw.unwrap()
    } else if kind == "go" {
        19
    } else {
        15
    };
    (kind, size)
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn parse_intent_inner(text: &str, pasted: bool) -> Option<UrlIntent> {
    let raw = text.trim();
    if raw.is_empty() {
        return if pasted { None } else { Some(UrlIntent::Menu) };
    }
    // 有协议就当 URL 解析；无协议（如 "goptop.pages.dev/u-1a2b?pwd=xx"）补上再解析。
    let with_proto = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let Some((path, pairs)) = split_href(&with_proto) else {
        return if pasted { None } else { Some(UrlIntent::Menu) };
    };
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // 旧 query 风格优先（?u= / ?room= / ?watch=）
    if let Some(room) = get(&pairs, "room") {
        return Some(UrlIntent::Watch { game_id: room.to_string() });
    }
    if let Some(u) = get(&pairs, "u") {
        // searchParams 已解码一次；不再二次解码（ID 含 % 时二次解码会失败/畸变）
        let (kind, size) = kind_size_from_params(&pairs);
        return Some(UrlIntent::User {
            user_id: u.to_string(),
            pwd: get(&pairs, "pwd").map(str::to_string),
            kind,
            size,
            rtc: get(&pairs, "rtc").map(str::to_string),
            spec: get(&pairs, "spec") == Some("1"),
        });
    }
    if let Some(w) = get(&pairs, "watch") {
        return Some(UrlIntent::Watch { game_id: w.to_string() });
    }

    // 路径风格
    match segs.as_slice() {
        [first] if *first == "local" => Some(UrlIntent::Local),
        [first] if *first == "p2p" => Some(UrlIntent::P2p),
        [first] if *first == "users" => Some(UrlIntent::Users),
        [first] if *first == "settings" => Some(UrlIntent::Settings),
        [first, second] if *first == "watch" => Some(UrlIntent::Watch { game_id: decode_component(second)? }),
        [first] => {
            let spec = get(&pairs, "spec") == Some("1");
            // 粘贴收紧：单段路径必须像用户链接，否则视为普通文本/陌生网址。
            if pasted && !first.starts_with("u-") && !pairs.iter().any(|(k, _)| k == "pwd" || k == "rtc" || k == "specrtc") {
                return None;
            }
            let (kind, size) = kind_size_from_params(&pairs);
            // 观战链接的直连参数名为 specrtc（与对局 rtc 语义区分）。
            let rtc_key = if spec { "specrtc" } else { "rtc" };
            Some(UrlIntent::User {
                user_id: decode_component(first)?,
                pwd: get(&pairs, "pwd").map(str::to_string),
                kind,
                size,
                rtc: get(&pairs, rtc_key).map(str::to_string),
                spec,
            })
        }
        [] => if pasted { None } else { Some(UrlIntent::Menu) },
        _ => if pasted { None } else { Some(UrlIntent::Menu) },
    }
}

/// 从粘贴的任意 URL 中提取邀请者回执参数（inviterId + pwd + rtcAns + game/kind/size）。
pub fn parse_pasted_answer(text: &str) -> Option<AnswerIntent> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let with_proto = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let (path, pairs) = split_href(&with_proto)?;
    // rtcAns 必须存在（TS query_pairs find?）。
    let rtc_ans = get(&pairs, "rtcAns")?.to_string();
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let (Some(first), None) = (segs.next(), segs.next()) else {
        return None; // 多段路径拒识
    };
    let inviter_id = decode_component(first)?;
    let kind = get(&pairs, "kind").filter(|k| *k == "go" || *k == "gomoku").map(str::to_string);
    let size = get(&pairs, "size").and_then(|s| s.parse::<u16>().ok()).filter(|s| matches!(s, 9 | 13 | 15 | 19));
    Some(AnswerIntent {
        inviter_id,
        pwd: get(&pairs, "pwd").unwrap_or("").to_string(),
        rtc_ans,
        spectator: get(&pairs, "spec") == Some("1"),
        game_id: get(&pairs, "game").map(str::to_string),
        kind,
        size,
    })
}

/* ---------------- 构造函数（origin 由调用方注入：Web=location.origin，Tauri=常量） ---------------- */

/// 邀请链接：`<origin>/<inviterId>?pwd=<pwd>&kind=&size=[&rtc=<inviteOffer>]`。
#[must_use]
pub fn invite_to_url(origin: &str, inviter_id: &str, pwd: &str, kind: &str, size: u16, rtc_offer: Option<&str>) -> String {
    let mut url = format!("{}/{}?pwd={}&kind={}&size={}", origin.trim_end_matches('/'), encode_path_segment(inviter_id), encode_form_value(pwd), encode_form_value(kind), size);
    if let Some(offer) = rtc_offer {
        url.push_str("&rtc=");
        url.push_str(&encode_form_value(offer));
    }
    url
}

/// 回执链接：受邀者把自动生成的 answer 编进 URL 发回邀请者（跨设备粘贴一次）。
#[must_use]
pub fn answer_to_url(
    origin: &str,
    inviter_id: &str,
    pwd: &str,
    rtc_ans: &str,
    game_id: Option<&str>,
    kind: Option<&str>,
    size: Option<u16>,
) -> String {
    let mut url = format!("{}/{}?pwd={}&rtcAns={}", origin.trim_end_matches('/'), encode_path_segment(inviter_id), encode_form_value(pwd), encode_form_value(rtc_ans));
    if let Some(g) = game_id {
        url.push_str(&format!("&game={}", encode_form_value(g)));
    }
    if let Some(k) = kind {
        url.push_str(&format!("&kind={}", encode_form_value(k)));
    }
    if let Some(s) = size {
        url.push_str(&format!("&size={s}"));
    }
    url
}

/// 用户主页链接：`<origin>/<userId>`（无 pwd，只看到主页、可手动挑战）。
#[must_use]
pub fn user_to_url(origin: &str, user_id: &str) -> String {
    format!("{}/{}", origin.trim_end_matches('/'), encode_path_segment(user_id))
}

/// 观战链接：`<origin>/<userId>?pwd=<specPwd>&spec=1`（观战钥匙整局有效）。
#[must_use]
pub fn spec_link_url(origin: &str, user_id: &str, spec_pwd: &str) -> String {
    format!("{}/{}?pwd={}&spec=1", origin.trim_end_matches('/'), encode_path_segment(user_id), encode_form_value(spec_pwd))
}

/// 同源观战链接：`<origin>/watch/<gameId>`。
#[must_use]
pub fn watch_to_url(origin: &str, game_id: &str) -> String {
    format!("{}/watch/{}", origin.trim_end_matches('/'), encode_path_segment(game_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ---------------- 解析（对齐 links.test.ts 用例） ---------------- */

    #[test]
    fn pasted_full_invite_domain_independent() {
        let r = parse_pasted_link("https://goptop.pages.dev/u-abc123?pwd=k9d2x1&kind=go&size=13&rtc=G1AbCd").unwrap();
        assert_eq!(r, UrlIntent::User { user_id: "u-abc123".into(), pwd: Some("k9d2x1".into()), kind: "go".into(), size: 13, rtc: Some("G1AbCd".into()), spec: false });
    }

    #[test]
    fn pasted_without_protocol() {
        let r = parse_pasted_link("goptop.pages.dev/u-abc123?pwd=xyz789&kind=gomoku&size=15").unwrap();
        assert_eq!(r, UrlIntent::User { user_id: "u-abc123".into(), pwd: Some("xyz789".into()), kind: "gomoku".into(), size: 15, rtc: None, spec: false });
    }

    #[test]
    fn legacy_query_styles() {
        let u = parse_pasted_link("https://x.dev/?u=u-abc&pwd=p1w2e3&kind=go&size=9").unwrap();
        assert!(matches!(u, UrlIntent::User { ref user_id, ref pwd, ref kind, size, .. }
            if user_id == "u-abc" && pwd.as_deref() == Some("p1w2e3") && kind == "go" && size == 9));
        // ?u= 只解码一次：ID 含 % 不失败（A4 双解码回归）。
        let u2 = parse_pasted_link("https://x.dev/?u=u-a%25b&pwd=p1").unwrap();
        assert!(matches!(u2, UrlIntent::User { ref user_id, .. } if user_id == "u-a%b"));
        assert_eq!(parse_pasted_link("https://x.dev/?room=g-1"), Some(UrlIntent::Watch { game_id: "g-1".into() }));
        assert_eq!(parse_pasted_link("https://x.dev/?watch=g-2"), Some(UrlIntent::Watch { game_id: "g-2".into() }));
    }

    #[test]
    fn pasted_single_segment_tightened() {
        assert_eq!(parse_pasted_link("https://x.dev/hello"), None);
        assert_eq!(parse_pasted_link("随便一段中文"), None);
        assert_eq!(parse_pasted_link("192.168.1.5:8080"), None);
        // 带 pwd 即识别。
        let r = parse_pasted_link("https://x.dev/u-xyz?pwd=abc123&kind=gomoku&size=15").unwrap();
        assert!(matches!(r, UrlIntent::User { ref user_id, .. } if user_id == "u-xyz"));
        // 未知 kind 回退 gomoku、非法 size 回退默认。
        let r2 = parse_pasted_link("https://x.dev/u-a?pwd=q1w2e3&kind=chess&size=7").unwrap();
        assert!(matches!(r2, UrlIntent::User { ref kind, size, .. } if kind == "gomoku" && size == 15));
    }

    #[test]
    fn address_bar_routing() {
        assert_eq!(parse_url("https://x.dev/"), UrlIntent::Menu);
        assert_eq!(parse_url("https://x.dev"), UrlIntent::Menu);
        assert_eq!(parse_url("https://x.dev/local"), UrlIntent::Local);
        assert_eq!(parse_url("https://x.dev/p2p"), UrlIntent::P2p);
        assert_eq!(parse_url("https://x.dev/users"), UrlIntent::Users);
        assert_eq!(parse_url("https://x.dev/settings"), UrlIntent::Settings);
        assert_eq!(parse_url("https://x.dev/watch/g-1"), UrlIntent::Watch { game_id: "g-1".into() });
        assert_eq!(parse_url("https://x.dev/a/b"), UrlIntent::Menu);
        assert_eq!(parse_url("https://x.dev/local/extra"), UrlIntent::Menu);
        // 畸形 %：解码失败回 menu（URIError 被吞口径）。
        assert_eq!(parse_url("https://x.dev/u-a%zz?pwd=k"), UrlIntent::Menu);
        // 地址栏不收紧：任何单段路径都是 user。
        assert_eq!(
            parse_url("https://x.dev/hello"),
            UrlIntent::User { user_id: "hello".into(), pwd: None, kind: "gomoku".into(), size: 15, rtc: None, spec: false }
        );
    }

    #[test]
    fn address_bar_user_full_fields() {
        let u = parse_url("https://x.dev/u-abc?pwd=k&kind=go&size=13&rtc=G1X");
        assert_eq!(u, UrlIntent::User { user_id: "u-abc".into(), pwd: Some("k".into()), kind: "go".into(), size: 13, rtc: Some("G1X".into()), spec: false });
        let s = parse_url("https://x.dev/u-abc?pwd=spec123&spec=1");
        assert_eq!(s, UrlIntent::User { user_id: "u-abc".into(), pwd: Some("spec123".into()), kind: "gomoku".into(), size: 15, rtc: None, spec: true });
        // 路径 userId 百分号解码。
        let d = parse_url("https://x.dev/u-a%20b%2Fc");
        assert!(matches!(d, UrlIntent::User { ref user_id, .. } if user_id == "u-a b/c"));
        // fragment 不参与解析。
        let f = parse_url("https://x.dev/watch/g-9#anchor");
        assert_eq!(f, UrlIntent::Watch { game_id: "g-9".into() });
    }

    /* ---------------- 回执解析 ---------------- */

    #[test]
    fn answer_full_fields() {
        let r = parse_pasted_answer("https://x.dev/u-inviter01?pwd=q9bwbu&rtcAns=G1AnsToken&game=g-99&kind=go&size=13").unwrap();
        assert_eq!(r, AnswerIntent {
            inviter_id: "u-inviter01".into(),
            pwd: "q9bwbu".into(),
            rtc_ans: "G1AnsToken".into(),
            spectator: false,
            game_id: Some("g-99".into()),
            kind: Some("go".into()),
            size: Some(13),
        });
        // spec=1 识别为观战回执。
        let s = parse_pasted_answer("https://x.dev/u-h?pwd=a1b2c3&rtcAns=G1X&game=g-1&kind=gomoku&size=15&spec=1").unwrap();
        assert!(s.spectator);
    }

    #[test]
    fn answer_rejections() {
        // 无 rtcAns / 多段路径 / 裸文本。
        assert!(parse_pasted_answer("https://x.dev/u-h?pwd=x&game=g").is_none());
        assert!(parse_pasted_answer("https://x.dev/watch/g-1?rtcAns=G1X").is_none());
        assert!(parse_pasted_answer("这不是链接").is_none());
        assert!(parse_pasted_answer("").is_none());
        // 非法 kind/size 置 None（由上层校验）。
        let r = parse_pasted_answer("https://x.dev/u-h?pwd=x&rtcAns=G1X&kind=xxx&size=0").unwrap();
        assert_eq!(r.kind, None);
        assert_eq!(r.size, None);
    }

    /* ---------------- 构造 → 解析往返 ---------------- */

    const ORIGIN: &str = "https://x.dev";

    #[test]
    fn invite_roundtrip() {
        let url = invite_to_url(ORIGIN, "u-abc", "k9d2x1", "go", 13, Some("G1X"));
        assert!(url.starts_with("https://x.dev/u-abc?"));
        assert_eq!(
            parse_pasted_link(&url),
            Some(UrlIntent::User { user_id: "u-abc".into(), pwd: Some("k9d2x1".into()), kind: "go".into(), size: 13, rtc: Some("G1X".into()), spec: false })
        );
        // 不带 rtc：解析出 rtc=None 且 URL 不含 rtc=。
        let url2 = invite_to_url(ORIGIN, "u-abc", "k9d2x1", "gomoku", 15, None);
        assert!(!url2.contains("rtc="));
        assert!(matches!(parse_pasted_link(&url2), Some(UrlIntent::User { rtc: None, spec: false, .. })));
    }

    #[test]
    fn spec_watch_user_roundtrip() {
        let spec = spec_link_url(ORIGIN, "u-host", "specpwd1");
        assert!(spec.contains("spec=1"));
        assert!(matches!(
            parse_pasted_link(&spec),
            Some(UrlIntent::User { pwd: Some(p), spec: true, .. }) if p == "specpwd1"
        ));
        assert_eq!(parse_pasted_link(&watch_to_url(ORIGIN, "g-42")), Some(UrlIntent::Watch { game_id: "g-42".into() }));
        // u- 前缀无 pwd 也识别为用户主页。
        assert!(matches!(
            parse_pasted_link(&user_to_url(ORIGIN, "u-plain")),
            Some(UrlIntent::User { pwd: None, spec: false, .. })
        ));
    }

    #[test]
    fn answer_roundtrip() {
        let url = answer_to_url(ORIGIN, "u-inviter", "q9bwbu", "G1Ans", Some("g-99"), Some("go"), Some(13));
        assert_eq!(
            parse_pasted_answer(&url),
            Some(AnswerIntent { inviter_id: "u-inviter".into(), pwd: "q9bwbu".into(), rtc_ans: "G1Ans".into(), spectator: false, game_id: Some("g-99".into()), kind: Some("go".into()), size: Some(13) })
        );
    }

    /// userId 特殊字符：编入（不裸拼）、解码往返等值（含 / ? & % 等路径毒字符）。
    #[test]
    fn user_id_special_chars_roundtrip() {
        for id in ["u-a b/c?", "u-中文", "u-100%安全", "u-a&b=c", "u-a?b#c", "u-+空格"] {
            let url = invite_to_url(ORIGIN, id, "p1w2e3", "gomoku", 15, None);
            assert!(!url.contains(id), "确已编码，未裸拼进 URL: {url}");
            assert!(matches!(parse_pasted_link(&url), Some(UrlIntent::User { user_id, .. }) if user_id == id), "id={id} url={url}");
        }
    }
}
