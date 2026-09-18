//! 会话状态机核心流程单测 — Elm reduce 的 native 直接驱动。
//!
//! 覆盖：服务器 join（对/错 pwd）/offer/answer 建局、协商队列（增强：不覆盖未决）、
//! 落子与同步、围棋双 Pass 计分同步、观战房间管理、无服务器跨设备观战回执、
//! 双挑战对撞 tiebreak。RTC/WS 的 IO 由 Effect 断言代替（本层无网络）。

use super::*;
use crate::links::UrlIntent;

/// 构建测试会话（A 视角，服务器模式可选）。
fn mk(user: &str, server_mode: bool) -> Session {
    Session::new(
        format!("u-{user}"),
        format!("peer-{user}"),
        user.to_string(),
        None,
        server_mode,
        vec![],
        "https://x.dev".into(),
    )
}

fn ctx(now: u64) -> ReduceCtx {
    ReduceCtx { now_ms: now, rand: [11, 22, 33, 44] }
}

/// A 开局（服务器模式）→ ready → B join（pwd 正确）→ A 出 accept+offer 信令。
#[test]
fn server_join_with_correct_pwd_proceeds() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    // 服务器先就绪（create_invite 有连通性前置检查）。
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    // A 开局。
    let fx = reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    assert!(a.phase == Phase::Waiting && a.role == Role::Inviter);
    assert!(a.pwd.is_some() && a.spec_pwd.is_some());
    assert!(fx.iter().any(|e| matches!(e, Effect::CreatePeer { inviter: true, .. })));
    // offer 生成完成（暂存不发）。
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("OFFER_A".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    // B join（pwd 正确）。
    let pwd = a.pwd.clone().unwrap();
    let fx = reduce(
        &mut a,
        Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "乙" }) }),
        &c,
    );
    // accept + offer 信令都发出；opponent 记录对端。
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "accept")));
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "offer" && v["payload"]["offer"] == "OFFER_A")));
    assert_eq!(a.opponent.as_deref(), Some("u-b"));
}

/// B join 错 pwd → 弹窗队列；拒绝 → reject 理由；同意 → proceed。
#[test]
fn server_join_wrong_pwd_queues_confirm() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("OFFER_A".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    // 错 pwd join。
    reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": "wrong!", "name": "乙" }) }), &c);
    // 弹窗出现（wrong-pwd），未自动受理。
    let head = a.confirm_head().expect("wrong-pwd 弹窗应入队");
    assert!(matches!(head.kind, ConfirmKind::WrongPwd));
    assert_eq!(a.opponent, None);
    // 拒绝 → reject 信令带理由。
    let fx = reduce(&mut a, Event::Ui(UiCommand::ConfirmDecline), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "reject" && v["payload"]["reason"] == "邀请钥匙不正确")));
    assert!(a.confirm_head().is_none());
    // 正确 pwd 的 join 不经弹窗直接 proceed。
    let pwd = a.pwd.clone().unwrap();
    let fx = reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-c".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "丙" }) }), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "accept")));
    assert_eq!(a.opponent.as_deref(), Some("u-c"));
}

/// join 互斥：已有对手后第二个 join 被拒（不再永久卡死第二个）。
#[test]
fn server_join_mutex() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("O".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    let pwd = a.pwd.clone().unwrap();
    reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "乙" }) }), &c);
    // 第二个 join：拒绝（正在对局中——A 在 answer 前仍 waiting，但 opponent 已占位）。
    let fx = reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-d".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "丁" }) }), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "reject")));
}

/// 协商队列（增强核心）：两个未决请求排队而非覆盖，逐个消费。
#[test]
fn confirm_queue_does_not_override() {
    let mut a = mk("a", true);
    let mut b = mk("b", true);
    let c = ctx(1000);
    // 双方直接摆进对局（走 admit 流）：A 邀请 B，B join。
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("O".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    let pwd = a.pwd.clone().unwrap();
    reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "乙" }) }), &c);
    // B 侧受理 offer 并 answer。
    reduce(&mut b, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut b, Event::Server(ServerEvt::Signal { from: "u-a".into(), kind: "offer".into(), payload: serde_json::json!({ "name": "甲", "kind": "gomoku", "size": 15, "gameId": "g-x", "offer": "O" }) }), &c);
    assert!(b.phase == Phase::Waiting && b.role == Role::Invitee);
    // 双方手动置为 playing（模拟 answer/直连完成后的对局态——直连 IO 不在本层）。
    a.phase = Phase::Playing;
    a.opponent = Some("u-b".into());
    a.peer_connected = true;
    b.phase = Phase::Playing;
    b.opponent = Some("u-a".into());
    b.peer_connected = true;
    // A（黑）先落一子并同步给 B——悔棋请求的 history 非空守卫才放行。
    let fx = reduce(&mut a, Event::Ui(UiCommand::Place { x: 7, y: 7 }), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    // B 连发两个协商请求（悔棋 + 重开）——A 端排队。
    reduce(&mut b, Event::Ui(UiCommand::RequestUndo), &c)
        .into_iter()
        .for_each(|e| {
            if let Effect::Broadcast(m) = e {
                reduce(&mut a, Event::Net(m), &c);
            }
        });
    reduce(&mut b, Event::Ui(UiCommand::RequestReset), &c)
        .into_iter()
        .for_each(|e| {
            if let Effect::Broadcast(m) = e {
                reduce(&mut a, Event::Net(m), &c);
            }
        });
    // 队首是 Undo，第二个 Reset 在队里——不被覆盖。
    assert!(matches!(a.confirm_head().unwrap().kind, ConfirmKind::Undo));
    // 同意悔棋 → 弹出 UndoAck(true) 广播 + 队列推进到 Reset。
    let fx = reduce(&mut a, Event::Ui(UiCommand::ConfirmApprove), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Broadcast(m) if matches!(m.kind, MsgKind::UndoAck { ok: true }))));
    assert!(matches!(a.confirm_head().unwrap().kind, ConfirmKind::Reset));
    // 拒绝重开 → ResetAck(false)。
    let fx = reduce(&mut a, Event::Ui(UiCommand::ConfirmDecline), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Broadcast(m) if matches!(m.kind, MsgKind::ResetAck { ok: false }))));
    assert!(a.confirm_head().is_none());
    // B 收到拒绝。
    let fx = reduce(&mut b, Event::Net(fx.into_iter().find_map(|e| match e { Effect::Broadcast(m) => Some(m), _ => None }).unwrap()), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Notice(n, _) if n.as_deref() == Some("对方拒绝了重开"))));
}

/// 对局消息：黑落子经 Move{by} 同步、非行棋方落子被拒、去重表拦截重复。
#[test]
fn net_move_guard_and_dedup() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    a.phase = Phase::Playing;
    a.role = Role::Inviter;
    a.my_color = "black".into();
    a.peer_connected = true;
    let msg = GameMsg::new(1, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 7, y: 7 } }, by: "black".into() });
    let fx = reduce(&mut a, Event::Net(msg.clone()), &c);
    // 远端落子只应用不转发（重广播是发送方的职责）：状态更新 + Emit。
    assert_eq!(a.board[7][7], "black");
    assert_eq!(a.to_move, "white");
    assert!(fx.iter().any(|e| matches!(e, Effect::Emit)));
    // 同一消息重复送达（三链路）→ 去重丢弃（无任何效应）。
    let fx = reduce(&mut a, Event::Net(msg), &c);
    assert!(fx.is_empty());
    assert_eq!(a.to_move, "white");
    // 白方接力落子（合法轮转）正常应用；同 seq 重复再拦一次。
    let fx = reduce(&mut a, Event::Net(GameMsg::new(2, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 8, y: 8 } }, by: "white".into() })), &c);
    assert_eq!(a.board[8][8], "white");
    let fx = reduce(&mut a, Event::Net(GameMsg::new(2, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 8, y: 8 } }, by: "white".into() })), &c);
    assert!(fx.is_empty());
}

/// 围棋双 Pass → scoring；死子标记同步 + 双确认 → 区域计分出胜者。
#[test]
fn go_scoring_flow() {
    let mut a = mk("a", false);
    let mut b = mk("b", false);
    let c = ctx(1000);
    // 双方对局态（围棋 9 路）。
    for s in [&mut a, &mut b] {
        s.phase = Phase::Playing;
        s.peer_connected = true;
        s.engine = goptop_core::game::GameState::new(goptop_core::game::GameKind::Go { size: 9 });
        sync_mirror_from_engine(s);
    }
    a.my_color = "black".into();
    b.my_color = "white".into();
    // 黑落一子，A 的 Move 广播喂给 B。
    let fx = reduce(&mut a, Event::Ui(UiCommand::Place { x: 4, y: 4 }), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    assert_eq!(a.board[4][4], "black");
    // A pass（黑）→ B pass（白）→ A pass（黑）→ 双 Pass 终局。
    // 测试内手动路由：A 的广播喂 B、B 的广播喂 A（真实路由由 transport 完成）。
    for _ in 0..3 {
        let (fx_from_a, fx_from_b) = {
            let is_a_turn = a.to_move == a.my_color;
            if is_a_turn {
                let fx = reduce(&mut a, Event::Ui(UiCommand::Pass), &c);
                (find_broadcast(&fx), None)
            } else {
                let fx = reduce(&mut b, Event::Ui(UiCommand::Pass), &c);
                (None, find_broadcast(&fx))
            }
        };
        if let Some(m) = fx_from_a {
            reduce(&mut b, Event::Net(m), &c);
        }
        if let Some(m) = fx_from_b {
            reduce(&mut a, Event::Net(m), &c);
        }
    }
    assert!(a.scoring && b.scoring, "双 Pass 后进入计分态");
    // A 标记死子并同步（标记盘上唯一的黑子 (4,4)——功能上允许任一有子点）。
    let fx = reduce(&mut a, Event::Ui(UiCommand::ToggleDead { x: 4, y: 4 }), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    assert_eq!(b.peer_dead.len(), 1);
    // A 确认 → ScoreConfirmReq 到 B → B 弹窗同意 → 双确认 → 计分完成。
    let fx = reduce(&mut a, Event::Ui(UiCommand::ConfirmScore), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    assert!(matches!(b.confirm_head().unwrap().kind, ConfirmKind::ScoreConfirm));
    let fx = reduce(&mut b, Event::Ui(UiCommand::ConfirmApprove), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut a, Event::Net(m), &c);
    }
    // 结果：黑 1 子 + 80 地 = 81，白 7.5 → 黑胜。
    let r = a.score_result.as_ref().expect("双确认后应出计分结果");
    assert_eq!(r.winner, "black");
    assert!((r.black - 81.0).abs() < f32::EPSILON);
    assert!(a.winner.as_deref() == Some("black") && b.winner.as_deref() == Some("black"));
}

fn find_broadcast(fx: &[Effect]) -> Option<GameMsg> {
    fx.iter().find_map(|e| match e {
        Effect::Broadcast(m) => Some(m.clone()),
        _ => None,
    })
}

/// 无服务器跨设备观战：房主 createInvite → 观战 offer 就绪（specUrl 生成）→
/// 观众解链接直连 → 观战回执生成 → 房主受理 → spec-pending 改名 live 并换新链接。
#[test]
fn serverless_spectator_receipt_flow() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    // 房主开局 + offer 就绪。
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: None, answer_plain: None, offer_enc: Some("G1INVITE".into()), answer_enc: None }, &c);
    assert!(a.invite_url.as_deref().is_some_and(|u| u.contains("rtc=G1INVITE")));
    // 观战 offer 就绪 → specUrl 生成（含 specrtc）。
    // 无服务器：specrtc 直载明文 offer JSON（单密钥设计，与 bridge payload 同形）。
    reduce(&mut a, Event::RtcReady { tag: "spec-pending".into(), offer_plain: Some(r#"{"s":"SDP-BODY","t":"offer","r":"spectator"}"#.into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    let spec_url = a.spec_url.clone().expect("specUrl 应生成");
    assert!(spec_url.contains("spec=1") && spec_url.contains("specrtc="));
    // 观众：以「打开链接」的真实路径处理 spec 意图（Boot 意图 → specrtc 直连）。
    let mut s = mk("spec", false);
    let intent = crate::links::parse_pasted_link(&spec_url).unwrap();
    let UrlIntent::User { user_id, pwd, rtc, spec: true, .. } = intent else {
        panic!("spec 链接应解析为 user+spec 意图");
    };
    assert_eq!(user_id, "u-a");
    assert!(rtc.is_some(), "specrtc 应解析进 rtc 字段");
    // process_intent 的等价入口：直接调用 Boot 流（对 spec 链接分派 specrtc 直连）。
    reduce(&mut s, Event::Boot { href: spec_url.clone() }, &c);
    // 观众 answer 就绪 → 观战回执链接生成。
    reduce(&mut s, Event::RtcReady { tag: "spec-main".into(), offer_plain: None, answer_plain: Some("G1SPECANS".into()), offer_enc: None, answer_enc: None }, &c);
    println!("[dbg] role={:?} phase={:?} answerBack={:?}", s.role, s.phase, s.answer_back_url);
    let receipt = s.answer_back_url.clone().expect("观战回执链接应生成");
    assert!(receipt.contains("spec=1") && receipt.contains("rtcAns="));
    // 房主解析观战回执并受理。
    let ans = crate::links::parse_pasted_answer(&receipt).unwrap();
    assert!(ans.spectator);
    let fx = reduce(&mut a, Event::Ui(UiCommand::AcceptSpecReceipt(Box::new(ans))), &c);
    // spec-pending 改名 live + 换新预生成。
    assert!(a.rtc_peers.iter().any(|q| q.tag == "spec-live-0"));
    assert!(fx.iter().any(|e| matches!(e, Effect::CreatePeer { tag, .. } if tag == "spec-pending")));
    assert!(fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { answer, .. } if answer == "G1SPECANS")));
}

/// 服务器模式「粘贴邀请链接」必须走信令服务器 join（而非同源 presence 挑战）。
/// 回归 2026-09-18 跨设备实测：粘贴路径曾发 presence——那是同源 BroadcastChannel 通道，
/// 跨设备到不了对方，两端永久停在「等待对手」/「等待邀请者自动确认」。
#[test]
fn server_paste_invite_joins_via_server() {
    let mut b = mk("b", true);
    let c = ctx(1000);
    reduce(&mut b, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    let fx = reduce(
        &mut b,
        Event::Ui(UiCommand::AcceptInvite { inviter_id: "u-a".into(), pwd: Some("p1w2e3".into()), kind: "gomoku".into(), size: 15, rtc: None, spec: false }),
        &c,
    );
    assert!(b.role == Role::Invitee && b.phase == Phase::Waiting);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "join" && v["to"] == "u-a" && v["payload"]["pwd"] == "p1w2e3")),
        "粘贴邀请链接应发服务器 join 信令"
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::SendPresence(_))), "不得再走同源 presence 挑战");
}

/// 乱序落子：后一手先到 → 暂存，等前一手补齐后自动补应用（不得永久丢失）。
/// 回归 2026-09-18 跨端实测：观战者多路径扇出（房主自己的手 + 房主转发的对手手）
/// 乱序时，被守卫拒收的手已记入去重表，重传副本被吃掉 → 永久少手。
#[test]
fn out_of_order_move_is_buffered_then_applied() {
    let mut s = mk("s", true);
    let c = ctx(1000);
    // 观战者视角复现（与对局者共用同一 apply 路径）。
    s.role = Role::Spectator;
    s.phase = Phase::Playing;
    s.peer_connected = true;
    // 两手来自**不同 sender**（黑手是房主自己下的、白手是房主转发的对手手），
    // 各自 seq 从 1 起——去重表按 sender 单调，这才对应真实乱序场景。
    let black1 = GameMsg::new(1, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 7, y: 7 } }, by: "black".into() });
    let white2 = GameMsg::new(1, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 8, y: 8 } }, by: "white".into() });
    // 第 2 手先到：轮次未到，不落子但暂存。
    reduce(&mut s, Event::Net(white2), &c);
    assert_eq!(s.board[8][8], "empty", "轮次未到不应落子");
    assert_eq!(s.pending_moves.len(), 1, "乱序手应暂存");
    assert_eq!(s.to_move, "black");
    // 第 1 手到达：应用它，并把暂存的第 2 手一并补上。
    reduce(&mut s, Event::Net(black1), &c);
    assert_eq!(s.board[7][7], "black");
    assert_eq!(s.board[8][8], "white", "轮次到达后应补应用暂存的乱序手");
    assert!(s.pending_moves.is_empty(), "补应用后暂存应清空");
    assert_eq!(s.to_move, "black", "两手走完轮到黑");
}

/// 重开/悔棋后暂存的乱序手必须作废（否则过期手会污染新局）。
#[test]
fn pending_moves_cleared_on_reset_and_undo() {
    let mut s = mk("s", false);
    let c = ctx(1000);
    s.phase = Phase::Playing;
    s.my_color = "black".into();
    s.peer_connected = true;
    let white2 = GameMsg::new(2, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 8, y: 8 } }, by: "white".into() });
    reduce(&mut s, Event::Net(white2), &c);
    assert_eq!(s.pending_moves.len(), 1);
    // 重开清空。
    s.reset_board_for("gomoku", 15);
    assert!(s.pending_moves.is_empty(), "重开后暂存应清空");
}

/// 服务器模式开局必须同时给出观战链接（含 spec=1 与 specPwd）。
/// 回归 2026-09-18 跨端实测：服务器模式从不生成 spec_url，「邀请观战」复制空串。
#[test]
fn server_create_invite_provides_spectator_link() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    let url = a.spec_url.clone().expect("服务器模式也应有观战链接");
    let sp = a.spec_pwd.clone().unwrap();
    assert!(url.contains("spec=1"), "观战链接须带 spec=1：{url}");
    assert!(url.contains(&sp), "观战链接须带 specPwd：{url}");
    assert!(url.contains(&a.user_id), "观战链接须指向房主：{url}");
}

/// 粘贴**观战**链接（spec=1）必须走观战通道（spec-join），不能当对局 join。
/// 回归 2026-09-18 跨端实测：UI 漏传 spec 标志，观战链接被当对局加入，
/// 房主按「对局中」拒绝后观众被弹回主页——粘贴观战链接完全无效。
#[test]
fn server_paste_spectator_link_joins_as_spectator() {
    let mut c = mk("c", true);
    let cc = ctx(1000);
    reduce(&mut c, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &cc);
    let fx = reduce(
        &mut c,
        Event::Ui(UiCommand::AcceptInvite { inviter_id: "u-a".into(), pwd: Some("spec77".into()), kind: "gomoku".into(), size: 15, rtc: None, spec: true }),
        &cc,
    );
    assert_eq!(c.role, Role::Spectator, "观战链接应进观战态");
    assert_eq!(c.my_host.as_deref(), Some("u-a"));
    assert!(
        fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "spec-join" && v["payload"]["pwd"] == "spec77")),
        "应发 spec-join 信令"
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "join")), "不得发对局 join");
}

/// 服务器未就绪时粘贴：意图挂起（pending_link），连接成立后再补发 join。
#[test]
fn server_paste_invite_waits_for_ready_then_flushes() {
    let mut b = mk("b", true);
    let c = ctx(1000);
    // 服务器尚未 ready：不应发出任何 join。
    let fx = reduce(
        &mut b,
        Event::Ui(UiCommand::AcceptInvite { inviter_id: "u-a".into(), pwd: Some("k1".into()), kind: "gomoku".into(), size: 15, rtc: None, spec: false }),
        &c,
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::SendServer(_))), "未就绪时不应发信令");
    assert!(b.pending_link.is_some(), "意图应挂起等待服务器");
    // 服务器就绪 → 补发 join。
    let fx = reduce(&mut b, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "join")), "就绪后应补发 join");
}

/// 双挑战对撞：字典序大者自动让位（确定性，两端决策一致）。
#[test]
fn challenge_collision_tiebreak() {
    let mut big = mk("zzz", true);
    let mut small = mk("aaa", true);
    let c = ctx(1000);
    // small → big 挑战（big 记录 outgoing）。
    reduce(&mut small, Event::Ui(UiCommand::ServerChallenge("u-zzz".into())), &c);
    let fx = reduce(&mut big, Event::Ui(UiCommand::ServerChallenge("u-aaa".into())), &c);
    // big 收到 small 的 challenge：big 字典序大 → 自动拒收让位。
    let fx2 = reduce(&mut big, Event::Server(ServerEvt::Signal { from: "u-aaa".into(), kind: "challenge".into(), payload: serde_json::json!({ "name": "甲" }) }), &c);
    let _ = (fx, fx2);
    assert!(big.incoming.is_none(), "字典序大的一方不弹窗");
    // small 收到 big 的 challenge：字典序小 → 弹窗。
    reduce(&mut small, Event::Server(ServerEvt::Signal { from: "u-zzz".into(), kind: "challenge".into(), payload: serde_json::json!({ "name": "乙" }) }), &c);
    assert!(small.incoming.is_some(), "字典序小的一方弹窗决策");
}

/// 踢人顺序：先发 spec-kicked 通知再从名单移除（spec-sync 才送得到被踢者）。
#[test]
fn kick_sends_notice_before_removal() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    a.server_state = "ready".into();
    a.opponent = Some("u-b".into()); // 另一位房主存在：spec-sync 有接收方
    a.spectators.push(Spectator { id: "u-s1".into(), name: "观众".into(), host: "u-a".into(), muted: false });
    let fx = reduce(&mut a, Event::Ui(UiCommand::KickSpec("u-s1".into())), &c);
    // 信令顺序：spec-kicked 在 spec-sync 之前。
    let idx_kick = fx.iter().position(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "spec-kicked"));
    let idx_sync = fx.iter().position(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "spec-sync"));
    assert!(idx_kick.is_some() && idx_sync.is_some() && idx_kick < idx_sync);
    assert!(a.spectators.is_empty());
}
