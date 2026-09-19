//! 鸿蒙会话命令面 —— 在**宿主机**上跑，不需要鸿蒙工具链。
//!
//! 覆盖的是「C ABI 那一层能不能正确驱动会话」：命令名/参数形态、拉模式回的
//! 快照与宿主动作、无会话时的回执。真正的跨端对局由模拟器上的 e2e 压。

use std::ffi::{CStr, CString};

/// 调一次 `goptop_call`（真实入口，含分配/释放），返回解析后的 JSON。
fn call(cmd: &str, args: &str) -> serde_json::Value {
    let c = CString::new(cmd).unwrap();
    let a = CString::new(args).unwrap();
    let p = unsafe { goptop_ohos::goptop_call(c.as_ptr(), a.as_ptr()) };
    assert!(!p.is_null(), "goptop_call 不得返回空指针");
    let out = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { goptop_ohos::goptop_free(p) };
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("回执不是合法 JSON: {e} | 原文={out}"))
}

/// 铺一份设置快照（相当于 ArkTS 建会话时传进来的整表）。
fn settings(user: &str) -> serde_json::Value {
    serde_json::json!({
        "goptop:userId": user,
        "goptop:stun": "[]",          // 关掉 STUN：本地候选即刻收集完，省掉 8s 兜底
        "goptop:name": "阿鸿",
    })
}

fn new_session(href: &str, user: &str) -> u32 {
    let cfg = serde_json::json!({
        "name": "阿鸿",
        "serverMode": false,
        "shareOrigin": "https://goptop.pages.dev",
        "kind": "gomoku",
        "size": 15,
    })
    .to_string();
    let r = call(
        "session_new",
        &serde_json::json!({ "cfgJson": cfg, "href": href, "settings": settings(user) }).to_string(),
    );
    r.as_u64().expect("建会话应给出会话号") as u32
}

/// 建会话 → 拉快照：身份必须来自**传入的设置整表**。
///
/// 这是鸿蒙侧与桌面端最大的形状差异（没有反向通道，Rust 读不到 JS 的门面），
/// 传丢了的表现是「每启一次换一个身份」，而界面上完全看不出来。
#[test]
fn session_new_reads_identity_from_passed_settings() {
    let id = new_session("https://appassets.goptop/", "u-ohos-test-1");
    let poll = call("session_poll", &format!(r#"{{"id":{id}}}"#));
    let snap: serde_json::Value = serde_json::from_str(poll["snapshot"].as_str().unwrap()).unwrap();
    assert_eq!(snap["userId"], "u-ohos-test-1", "身份应取自传入的设置整表");
    assert_eq!(snap["phase"], "home");
    assert_eq!(snap["serverMode"], false);
    call("session_drop", &format!(r#"{{"id":{id}}}"#));
}

/// 带邀请链接建会话：Boot 应把会话推进到等待态（与桌面/Web 同一条状态机路径）。
#[test]
fn session_new_with_invite_link_enters_waiting() {
    let href = "https://appassets.goptop/u-other-1?pwd=abc123&kind=gomoku&size=15&rtc=G1DUMMY";
    let id = new_session(href, "u-ohos-test-2");
    let poll = call("session_poll", &format!(r#"{{"id":{id}}}"#));
    let snap: serde_json::Value = serde_json::from_str(poll["snapshot"].as_str().unwrap()).unwrap();
    assert_eq!(snap["phase"], "waiting", "带邀请链接应进等待态，实际={}", snap["phase"]);
    assert_eq!(snap["role"], "invitee");
    call("session_drop", &format!(r#"{{"id":{id}}}"#));
}

/// 命令面：标签形态、未知标签报错、无会话回执。
#[test]
fn session_cmd_contract() {
    let id = new_session("https://appassets.goptop/", "u-ohos-test-3");
    // 合法命令（serde 外部标签：单元变体是裸字符串）
    assert_eq!(call("session_cmd", &format!(r#"{{"id":{id},"cmdJson":"\"createInvite\""}}"#))["ok"], true);
    // 拼错的标签不能静默——静默在设备上的表现是「这个按钮没反应」
    let bad = call("session_cmd", &format!(r#"{{"id":{id},"cmdJson":"\"noSuchCmd\""}}"#));
    assert_eq!(bad["ok"], false);
    assert!(bad["error"].as_str().unwrap_or_default().contains("bad cmd"), "实际={bad}");
    // 不存在的会话
    let gone = call("session_cmd", r#"{"id":999999,"cmdJson":"\"backHome\""}"#);
    assert_eq!(gone["ok"], false);
    assert_eq!(gone["error"], "no_session");
    call("session_drop", &format!(r#"{{"id":{id}}}"#));
}

/// 纯函数解析不需要会话实例（与桌面端同一份实现，输出形态一致）。
#[test]
fn parse_commands_need_no_session() {
    let r = call("session_parse_link", r#"{"text":"https://x.dev/u-a1b2?pwd=zz"}"#);
    let v: serde_json::Value = serde_json::from_str(r.as_str().unwrap()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["intent"]["mode"], "user");
    assert_eq!(v["intent"]["pwd"], "zz");
    let bad = call("session_parse_link", r#"{"text":"随便一段话"}"#);
    let b: serde_json::Value = serde_json::from_str(bad.as_str().unwrap()).unwrap();
    assert_eq!(b["ok"], false);
}

/// 释放会话后，读类命令按「无此局」处理且不 panic。
#[test]
fn dropped_session_is_inert() {
    let id = new_session("https://appassets.goptop/", "u-ohos-test-4");
    call("session_drop", &format!(r#"{{"id":{id}}}"#));
    for cmd in ["session_poll", "session_state_json", "session_state_debug", "session_ice_debug"] {
        let v = call(cmd, &format!(r#"{{"id":{id}}}"#));
        assert!(v.is_null() || v["snapshot"] == "null", "{cmd} 对已释放会话应给空回执，实际={v}");
    }
}
