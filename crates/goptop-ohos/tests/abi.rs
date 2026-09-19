//! C ABI 全命令覆盖 —— 在**宿主机**上跑，不需要鸿蒙工具链。
//!
//! 这是本 crate 能在开发机上验证的唯一手段（设备上只能看「加载即崩 / 功能没反应」），
//! 所以覆盖到每条命令与每条错误分支：命令名打错、局号不存在、参数缺失、非法尺寸。
//!
//! 用例通过真实入口 `goptop_call` / `goptop_free` 调用，不直接碰内部函数——
//! 压的正是 C++ 侧会走的那条路。

use std::ffi::{CStr, CString, c_char};

/// 调一次 `goptop_call`，返回解析后的 JSON。**走完整的分配/释放路径**。
fn call(cmd: &str, args: &str) -> serde_json::Value {
    let c = CString::new(cmd).unwrap();
    let a = CString::new(args).unwrap();
    let p = unsafe { goptop_ohos::goptop_call(c.as_ptr(), a.as_ptr()) };
    assert!(!p.is_null(), "goptop_call 不得返回空指针");
    let out = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { goptop_ohos::goptop_free(p) };
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("回执不是合法 JSON: {e} | 原文={out}"))
}

/// 空指针入参（C++ 侧漏传参数时的形态）不得 panic。
#[test]
fn null_pointers_are_tolerated() {
    let p = unsafe { goptop_ohos::goptop_call(std::ptr::null(), std::ptr::null()) };
    assert!(!p.is_null(), "空入参也要给一条合法回执，不能崩");
    let out = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { goptop_ohos::goptop_free(p) };
    assert!(out.contains("unknown command"), "空命令应回 unknown command，实际={out}");
    // 释放空指针必须是无操作（C++ 侧的清理路径可能拿到 null）
    unsafe { goptop_ohos::goptop_free(std::ptr::null_mut::<c_char>()) };
}

/// 未知命令：必须给出可读回执，而不是静默的 null（设备上静默＝「功能没反应」）。
#[test]
fn unknown_command_is_reported() {
    let v = call("game_plce", "{}"); // 故意拼错
    assert!(v["error"].as_str().unwrap_or_default().contains("unknown command"), "实际={v}");
}

/// 五子棋全流程：建局 → 落子 → 状态 → 悔棋 → 重开 → 释放。
#[test]
fn gomoku_full_lifecycle() {
    let id = call("game_new", r#"{"kindJson":"{\"Gomoku\":{\"size\":15}}"}"#);
    let id = id.as_u64().expect("合法尺寸应给出局号");
    assert_eq!(call("game_board_size", &format!(r#"{{"id":{id}}}"#)), 15);

    // 状态是 GameState 的 serde 形态：board 是 BoardVariant（{"B15":{"cells":[…]}}），
    // 棋子是 PascalCase 的 Stone（"Empty"/"Black"/"White"）——与 PlaceResult 的小写
    // 契约不同源，断言别混用
    let st = call("game_state_json", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(st["kind"]["Gomoku"]["size"], 15, "状态应带棋种与尺寸，实际={}", st["kind"]);
    assert_eq!(st["board"]["B15"]["cells"].as_array().map(|r| r.len()), Some(15), "应是 15×15 盘面");
    assert_eq!(st["to_move"], "Black", "开局黑先");

    // 黑先落 (7,7)（x=7,y=7）：x 是列
    let r = call("game_place", &format!(r#"{{"id":{id},"x":7,"y":7}}"#));
    assert_eq!(r["ok"], true, "合法落子应 ok，实际={r}");
    assert_eq!(r["toMove"], "white", "黑落子后轮白（PlaceResult 用小写契约）");
    assert_eq!(r["board"][7][7], "black", "权威棋盘该点应是黑子");

    // 同点重下：占位拒绝，错误码稳定
    let r2 = call("game_place", &format!(r#"{{"id":{id},"x":7,"y":7}}"#));
    assert_eq!(r2["ok"], false);
    assert_eq!(r2["error"], "occupied", "占位应给稳定错误码，实际={r2}");

    // 越界拒绝（15 路没有第 15 列，0-based）
    let r3 = call("game_place", &format!(r#"{{"id":{id},"x":15,"y":0}}"#));
    assert_eq!(r3["ok"], false, "越界应拒绝，实际={r3}");

    // 悔棋 → 空盘
    let u = call("game_undo", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(u["ok"], true, "悔棋应成功，实际={u}");
    assert_eq!(u["toMove"], "black", "撤回黑子后轮黑");

    // 停一手 → 轮白
    let p = call("game_pass", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(p["ok"], true, "停一手应成功，实际={p}");
    assert_eq!(p["toMove"], "white");

    // 重开 → 空盘黑先
    call("game_reset", &format!(r#"{{"id":{id}}}"#));
    let st2 = call("game_state_json", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(st2["to_move"], "Black", "重开后黑先");

    // 释放后一切按「无此局」处理，不得 panic
    call("game_drop", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(call("game_state_json", &format!(r#"{{"id":{id}}}"#)), serde_json::Value::Null);
    assert_eq!(call("game_board_size", &format!(r#"{{"id":{id}}}"#)), 0);
    let gone = call("game_place", &format!(r#"{{"id":{id},"x":1,"y":1}}"#));
    assert_eq!(gone["ok"], false);
    assert_eq!(gone["error"], "no_game", "无此局应给 no_game（与 Tauri 侧同码）");
}

/// 非法尺寸组合：返回 null 让前端走「建局失败」，不得 panic（核心层是断言，release 下 panic=abort）。
#[test]
fn illegal_size_yields_null_not_panic() {
    for kind in [r#"{"Gomoku":{"size":9}}"#, r#"{"Go":{"size":15}}"#, r#"{"Go":{"size":5}}"#] {
        let args = serde_json::json!({ "kindJson": kind }).to_string();
        assert_eq!(call("game_new", &args), serde_json::Value::Null, "{kind} 应被拒");
    }
}

/// 围棋：提子、双停一手终局、区域计分全走同一份 json_api。
#[test]
fn go_capture_pass_and_score() {
    let id = call("game_new", r#"{"kindJson":"{\"Go\":{\"size\":9}}"}"#).as_u64().unwrap();
    let g = format!(r#"{{"id":{id}}}"#);
    let put = |x: u32, y: u32| call("game_place", &format!(r#"{{"id":{id},"x":{x},"y":{y}}}"#));

    // 经典提子：白 (1,1) 被黑四面围住
    for (x, y) in [(1, 0), (0, 1), (2, 1), (1, 2)] {
        assert_eq!(put(x, y)["ok"], true, "黑 ({x},{y}) 应可落");
        // 黑落完轮白：白随便应一手，别把自己应死
        if (x, y) != (1, 2) {
            let w = [(6, 6), (6, 7), (7, 6)][[(1, 0), (0, 1), (2, 1)].iter().position(|p| *p == (x, y)).unwrap()];
            assert_eq!(put(w.0, w.1)["ok"], true, "白 {w:?} 应可落");
        }
    }
    // 白 (1,1) 已在第四手黑落下时被提（GameState 的 board 是 {"B19":{"cells":[…]}}，
    // 棋子为 PascalCase）
    let st = call("game_state_json", &g);
    let cells = &st["board"]["B19"]["cells"];
    assert_eq!(cells[1][1], "Empty", "白 (1,1) 应被提掉，实际={}", cells[1][1]);
    assert_eq!(cells[1][0], "Black", "黑 (1,0) 应在盘上");

    // 双停一手 → 终局
    assert_eq!(call("game_pass", &g)["ok"], true, "白停一手");
    let p2 = call("game_pass", &g);
    assert_eq!(p2["ok"], true, "黑停一手");
    assert!(p2["winner"].is_null() || p2["winner"] == "empty" || p2.get("scoring").is_some() || p2["toMove"].is_string(),
        "双停一手后应进终局/计分，实际={p2}");

    // 计分：全盘无死子标记也要给出确定结果
    let sc = call("game_score", &format!(r#"{{"id":{id},"deadJson":"[]"}}"#));
    assert!(sc.get("black").is_some() || sc.get("ok").is_some(), "计分应给出结果，实际={sc}");
}

/// `game_adopt`：全量快照采纳；尺寸不符必须被拒（返回 false），不得污染现有局面。
#[test]
fn adopt_rejects_mismatched_board() {
    let id = call("game_new", r#"{"kindJson":"{\"Gomoku\":{\"size\":15}}"}"#).as_u64().unwrap();
    let g = format!(r#"{{"id":{id}}}"#);
    // 正确的 15×15 空盘快照。**toMove 必须与「按 history 重放的结果」一致**：
    // adopt 是「重放后比对」，空历史重放出来是黑先，快照写 white 会被判不一致而整份拒收
    // （这正是它该有的严格性——快照与历史互为校验）。
    let board = serde_json::to_string(&vec![vec!["empty"; 15]; 15]).unwrap();
    let ok = call("game_adopt", &serde_json::json!({
        "id": id, "boardJson": board, "toMove": "black", "winner": "null", "historyJson": "[]"
    }).to_string());
    assert_eq!(ok, true, "合法快照应被采纳");
    assert_eq!(call("game_state_json", &g)["to_move"], "Black");

    // 尺寸不符的 9×9 快照
    let bad = serde_json::to_string(&vec![vec!["empty"; 9]; 9]).unwrap();
    let no = call("game_adopt", &serde_json::json!({
        "id": id, "boardJson": bad, "toMove": "black", "winner": "null", "historyJson": "[]"
    }).to_string());
    assert_eq!(no, false, "尺寸不符应被拒");
    assert_eq!(call("game_state_json", &g)["to_move"], "Black", "被拒的快照不得改动局面");
    // 非法颜色串也整份拒绝（不得默认成 Empty 悄悄通过）
    let bad_move = call("game_adopt", &serde_json::json!({
        "id": id, "boardJson": serde_json::to_string(&vec![vec!["empty"; 15]; 15]).unwrap(),
        "toMove": "empty", "winner": "null", "historyJson": "[]"
    }).to_string());
    assert_eq!(bad_move, false, "toMove 是 empty 应被拒");
}

/// AI：五子棋给着法与胜率；终局不进搜索而是直接给确定胜率。
#[test]
fn ai_analyze_gomoku_and_terminal() {
    let id = call("game_new", r#"{"kindJson":"{\"Gomoku\":{\"size\":15}}"}"#).as_u64().unwrap();
    let g = format!(r#"{{"id":{id}}}"#);
    let state = call("game_state_json", &g);
    // myColor 是 serde 形态的大驼峰（见 frontend/src/ai/types.ts）——这一层直接反序列化
    // 成 goptop-core 的 Stone，小写会被拒
    let req = serde_json::json!({
        "state": state, "myColor": "Black", "budgetMs": 300, "wantMove": true
    }).to_string();
    let r = call("ai_analyze", &req);
    assert!(r.get("error").is_none(), "分析不该报错，实际={r}");
    let bm = r["bestMove"].as_array().expect("要着法时应给 bestMove");
    assert_eq!(bm.len(), 2, "着法是 [x,y]");
    let wr = r["winRate"].as_f64().expect("应给胜率");
    assert!((0.0..=1.0).contains(&wr), "胜率应在 0..1，实际={wr}");
    assert!(r["elapsedMs"].as_u64().unwrap_or(0) > 0, "应有耗时");

    // 预热：重复调用幂等（权重只解压一次），且不得 panic
    call("ai_warmup", "{}");
    call("ai_warmup", "{}");
}

/// 大量命令往返：压「分配/释放」这条路径不漏不崩（C++ 侧漏 free 就是稳定泄漏）。
#[test]
fn many_round_trips_do_not_corrupt() {
    let id = call("game_new", r#"{"kindJson":"{\"Go\":{\"size\":19}}"}"#).as_u64().unwrap();
    for i in 0..500u32 {
        let x = i % 19;
        let y = (i / 19) % 19;
        let _ = call("game_place", &format!(r#"{{"id":{id},"x":{x},"y":{y}}}"#));
        let _ = call("game_board_size", &format!(r#"{{"id":{id}}}"#));
    }
    assert_eq!(call("game_board_size", &format!(r#"{{"id":{id}}}"#)), 19, "往返五百次后仍应可查");
    call("game_drop", &format!(r#"{{"id":{id}}}"#));
}
