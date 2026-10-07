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
    // 第二个 join：拒绝（A 仍 waiting，靠 opponent 占位拦下；reason = 「对方不在等待对局」）。
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
    reduce(&mut a, Event::Net(GameMsg::new(2, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 8, y: 8 } }, by: "white".into() })), &c);
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
    let UrlIntent::User { user_id, pwd: _, rtc, spec: true, .. } = intent else {
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

/// 挑战受理到达时，挑战者必须被带到对局页（他在名册页发起的挑战）。
/// 回归 2026-09-18 实机测试：受理方自动进入对局，挑战方却停在名册页。
#[test]
fn challenge_accepted_navigates_challenger_to_game() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    // 挑战者在主页向对方发起挑战（他会留在名册页）。
    reduce(&mut a, Event::Ui(UiCommand::ServerChallenge("u-b".into())), &c);
    assert!(a.phase == Phase::Home, "挑战发起后仍在主页");
    // 对方受理 → 挑战者应被导航到 /p2p。
    let fx = reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "challenge-accepted".into(), payload: serde_json::json!({ "name": "乙" }) }), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Nav(p) if p == "/p2p")), "挑战者应被带到对局页");
}

/// 认输：认输者未必正在行棋——胜负必须按**认输者的执色**判，且认输不增加手数。
/// 回归 2026-09-18 实机测试：黑落一手后（轮到白）点认输，被判「黑胜」；镜像还把
/// 认输记成一手停手，手数从 1 变 2。
#[test]
fn resign_by_non_moving_side_judges_by_resigner_color() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    a.phase = Phase::Playing;
    a.role = Role::Inviter;
    a.my_color = "black".into();
    a.peer_connected = true;
    // 黑落一手 → 轮到白（此时黑认输 = 非行棋方认输）。
    reduce(&mut a, Event::Ui(UiCommand::Place { x: 7, y: 7 }), &c);
    assert_eq!(a.to_move, "white");
    assert_eq!(a.history.len(), 1);
    // 黑认输。
    let fx = reduce(&mut a, Event::Ui(UiCommand::Resign), &c);
    assert_eq!(a.winner.as_deref(), Some("white"), "认输者执黑 → 白胜");
    assert_eq!(a.history.len(), 1, "认输不是一手棋，手数不得增加");
    let msg = fx.iter().find_map(|e| match e { Effect::Broadcast(m) => Some(m.clone()), _ => None }).expect("应广播认输");
    // 对手收到：即使轮到白、by=black 也必须立即生效（不得被轮次守卫拦下）。
    let mut b = mk("b", false);
    b.phase = Phase::Playing;
    b.role = Role::Invitee;
    b.my_color = "white".into();
    b.peer_connected = true;
    b.to_move = "white".into();
    reduce(&mut b, Event::Net(msg), &c);
    assert_eq!(b.winner.as_deref(), Some("white"), "对手应立刻收到认输结果");
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

/// 旧链接窗口拿着**上一轮**钥匙的 answer 回传：必须拒绝，且不得掐死眼前这条等待中的连接。
/// 回归：判据曾是「带了 rtcAns 且自己是邀请者就放行」，而邀请链接是发给任意人的——
/// 旧窗口会先 `close_all_rtc()` 掐死本轮连接、再拿旧 answer 去喂它，两端都停在
/// 「对局中」而 `peerConnected` 恒 false，全程无报错。判据改成「answer 解得开本轮 pwd」。
#[test]
fn answer_from_a_stale_invite_link_is_rejected() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: None, answer_plain: None, offer_enc: Some("G1INVITE".into()), answer_enc: None }, &c);
    let round_game = a.game_id.clone().expect("开局应有 gameId");
    // 旧窗口形态与真实一致：answer 是**旧 pwd** 的 G1 密文，presence 不带 pwd
    //（`on_rtc_ready` 的受邀者分支里 `"pwd": null` 是硬编码）。
    let stale = crate::codec::encode(r#"{"s":"OLD-SDP","t":"answer","r":"player"}"#, "0000-old-key").unwrap();
    let fx = reduce(
        &mut a,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-old".into(),
            from_name: "旧窗口".into(),
            pwd: None,
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-stale".into(),
            rtc_ans: Some(stale),
        }),
        &c,
    );
    assert!(fx.iter().any(|e| matches!(e, Effect::SendPresence(v) if v["t"] == "reject")), "旧钥匙的 answer 必须被拒");
    assert!(!fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { .. })), "不得把旧 answer 喂给本轮连接");
    assert_eq!(a.phase, Phase::Waiting, "本局应仍在等待");
    assert_eq!(a.game_id.as_deref(), Some(round_game.as_str()), "不得改用旧局的 gameId");
    assert!(a.rtc_peers.iter().any(|q| q.tag == "main"), "本轮连接账本不得被清空");
}

/// 本轮受邀者的 answer（本轮 pwd 的 G1 密文）经 presence 回传：自动受理。
/// 同时压住「钥匙随 Effect 携带」这条硬约束——`accept_challenge_with` 紧接着就清 `s.pwd`，
/// 而 Effect 是 reduce 返回之后才执行的，到那时再读 session 只能读到 None。
#[test]
fn answer_back_with_the_current_key_is_accepted() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: None, answer_plain: None, offer_enc: Some("G1INVITE".into()), answer_enc: None }, &c);
    let pwd = a.pwd.clone().expect("等待中的邀请者应有 pwd");
    let ans = crate::codec::encode(r#"{"s":"SDP-B","t":"answer","r":"player"}"#, &pwd).unwrap();
    let fx = reduce(
        &mut a,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-b".into(),
            from_name: "乙".into(),
            pwd: None,
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-b".into(),
            rtc_ans: Some(ans.clone()),
        }),
        &c,
    );
    assert!(
        fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { answer, pwd: Some(p), .. } if answer == &ans && p == &pwd)),
        "应受理本轮 answer 并带上本局钥匙"
    );
    assert_eq!(a.phase, Phase::Playing);
    assert_eq!(a.game_id.as_deref(), Some("g-b"));
}

/// 服务器 join 受理后 `main` 已改名为对端 ID：受理挑战时重登记的槽位必须沿用同一个 tag。
/// 回归：喂 answer 取一次 tag、登记槽位再取一次（此时 `close_all_rtc()` 已把
/// `inviter_main` 清成 None，取到的是回退值 "main"）——槽位记在 "main" 上、answer 喂给
/// 对端 ID，`on_peer_state(tag)` 找不到槽位，连接通了 `peer_connected` 也置不上。
#[test]
fn accepted_connection_slot_keeps_the_renamed_tag() {
    let mut a = mk("a", true);
    let c = ctx(1000);
    // 直接摆出「服务器 join 已受理」后的账本：main 已改名成对端 ID。
    a.role = Role::Inviter;
    a.phase = Phase::Waiting;
    a.pwd = Some("p1w2e3".into());
    a.game_id = Some("g-a".into());
    a.inviter_main = Some("u-b".into());
    a.rtc_peers.push(PeerSlot { tag: "u-b".into(), player: true, spectator: false, opened: false, offer_ready: true, offer_plain: None, awaiting_peer: None });
    let ans = crate::codec::encode(r#"{"s":"SDP-B","t":"answer","r":"player"}"#, "p1w2e3").unwrap();
    let fx = reduce(
        &mut a,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-b".into(),
            from_name: "乙".into(),
            pwd: Some("p1w2e3".into()),
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-b".into(),
            rtc_ans: Some(ans),
        }),
        &c,
    );
    assert!(fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { tag, .. } if tag == "u-b")), "answer 应喂给 transport 里真实的 tag");
    assert!(a.rtc_peers.iter().any(|q| q.tag == "u-b"), "受理后槽位 tag 必须仍是 transport 侧的 u-b");
    assert!(!a.rtc_peers.iter().any(|q| q.tag == "main"), "不得登记回退 tag");
}

/// 跨设备对局回执全链路（无服务器唯一的跨设备配对通路）：A 邀请 → B 点链接出 answer →
/// B 的回执链接 → A 粘贴受理。
///
/// **回执必须带回本轮 pwd**：受理端的校验是 `Some(r.pwd) == s.pwd`，回执链接里 pwd 为空
/// 就会被一律回「回执钥匙与本局不符，已拒绝」——跨设备粘贴这条路从头到尾走不通（观战回执
/// 那条传的是 spec_pwd，两条同理，只有对局回执曾把 pwd 写死成 ""）。
/// 本用例刻意走**真实产物**（`answer_back_url`）而不是手工拼一条带正确 pwd 的链接：
/// 手工拼的链接测不到「链接里到底带了什么」这一层，正是本条要压的地方。
/// 受理后 answer 的解密钥匙取**本局 pwd**——回执校验已保证 `s.pwd == r.pwd`，此处必为 Some；
/// 观战钥匙（spec_pwd）是另一把钥匙，解不开对局 answer，不能当兜底。
#[test]
fn cross_device_receipt_carries_the_key_and_is_accepted() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: None, answer_plain: None, offer_enc: Some("G1INVITE".into()), answer_enc: None }, &c);
    let pwd = a.pwd.clone().expect("等待中的邀请者应有 pwd");
    let link = a.invite_url.clone().expect("无服务器开局应回填邀请链接");
    // B：真实入口——点邀请链接进来（Boot 路径）。
    let mut b = mk("b", false);
    reduce(&mut b, Event::Boot { href: link }, &c);
    assert_eq!(b.role, Role::Invitee, "点链接应进受邀者态");
    let ans = crate::codec::encode(r#"{"s":"SDP-B","t":"answer","r":"player"}"#, &pwd).unwrap();
    reduce(&mut b, Event::RtcReady { tag: "main".into(), offer_plain: Some("PLAIN".into()), answer_plain: None, offer_enc: None, answer_enc: Some(ans.clone()) }, &c);
    // B 的回执链接（跨设备时就是复制这条链接发回邀请者）。
    let receipt = b.answer_back_url.clone().expect("受邀者应生成回执链接");
    let intent = crate::links::parse_pasted_answer(&receipt).expect("回执链接应可解析");
    assert_eq!(intent.pwd, pwd, "回执链接必须带回本轮钥匙，否则邀请者一律拒收");
    // A 粘贴受理。
    let fx = reduce(&mut a, Event::Ui(UiCommand::AcceptReceipt(Box::new(intent))), &c);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { answer, pwd: Some(p), .. } if answer == &ans && p == &pwd)),
        "回执受理应带上本局 pwd；实际 effects={:?}",
        fx.iter().map(|e| format!("{e:?}").chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
    // 受理只喂 answer，不进局——进局留给「玩家连接 open」。这一步曾只判受邀者，
    // 邀请者于是永远停在 waiting 而 peerConnected 已为真（浏览器实测：A 卡等待、
    // 棋盘不可落子，B 已在对局里，全程无报错）。
    assert_eq!(a.phase, Phase::Waiting, "受理当场不该进局，等直连 open");
    let fx2 = reduce(&mut a, Event::PeerState { tag: "main".into(), opened: true, closed: false, failed: false }, &c);
    assert_eq!(a.phase, Phase::Playing, "玩家连接 open 后邀请者必须进对局");
    assert!(a.peer_connected, "连接已开，peerConnected 应为真");
    assert!(
        fx2.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("对局开始"))),
        "应给用户一句「对局开始」；实际 effects={:?}",
        fx2.iter().map(|e| format!("{e:?}").chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
    assert!(!fx2.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("执白"))), "邀请者执黑，不得套用受邀者的「你执白」提示");
}

/// 观战者申请发言：向自己的 host 发 spec-chat-req 信令。
#[test]
fn spectator_request_chat_sends_signal_to_host() {
    let mut c = mk("c", true);
    c.role = Role::Spectator;
    c.my_host = Some("u-a".into());
    c.server_state = "ready".into();
    c.phase = Phase::Playing;
    let cc = ctx(1000);
    let fx = reduce(&mut c, Event::Ui(UiCommand::RequestSpecChat), &cc);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::SendServer(v) if v["kind"] == "spec-chat-req" && v["to"] == "u-a")),
        "应发 spec-chat-req 给 host；实际 effects={:?}",
        fx.iter().map(|e| format!("{e:?}").chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
}

/// `UiCommand` 的 serde 往返：原生宿主（Tauri / 鸿蒙）的命令面只有
/// `session_cmd(cmdJson)` 一个口子，路径全靠这份标签——**写错标签在设备上只是
/// 「这条命令没反应」**，不会报错，所以逐条锁死。
///
/// 覆盖三类形状（serde 的**外部标签**默认形态）：单元变体是裸字符串 `"backHome"`、
/// 结构体变体是 `{"place":{"x":7,"y":8}}`、元组变体是 `{"sendChat":"文本"}`，
/// 另外还有带 `#[serde(rename_all="camelCase")]` 字段的 `AnswerIntent`（回执受理那条，
/// 字段最多）。前端按同一份形状构造，写错就是「这条命令没反应」，所以形态要钉死。
#[test]
fn ui_command_json_round_trip() {
    use crate::links::AnswerIntent;
    let cases = vec![
        UiCommand::CreateInvite,
        UiCommand::BackHome,
        UiCommand::Place { x: 7, y: 8 },
        UiCommand::ToggleDead { x: 0, y: 14 },
        UiCommand::AcceptInvite {
            inviter_id: "u-a".into(),
            pwd: Some("p1w2e3".into()),
            kind: "gomoku".into(),
            size: 15,
            rtc: Some("G1TOKEN".into()),
            spec: false,
        },
        UiCommand::SendChat("你好".into()),
        UiCommand::MuteSpec("u-c".into(), true),
        UiCommand::PickSize(19),
        UiCommand::AcceptReceipt(Box::new(AnswerIntent {
            inviter_id: "u-a".into(),
            pwd: "p1w2e3".into(),
            rtc_ans: "G1ANS".into(),
            spectator: false,
            game_id: Some("g-1".into()),
            kind: Some("go".into()),
            size: Some(9),
        })),
    ];
    for c in cases {
        let json = serde_json::to_string(&c).expect("UiCommand 必须可序列化");
        let back: UiCommand = serde_json::from_str(&json).unwrap_or_else(|e| panic!("往返失败: {json} → {e}"));
        // 逐字节比对而不是 Debug：Debug 不保证稳定，而这里要锁的正是**线上形态**
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
    // 标签是 camelCase 而非 PascalCase（前端按 camelCase 构造，写死在这里防回退）；
    // 单元变体是**裸字符串**（serde 外部标签的默认形态），不是 {"backHome":null}
    assert_eq!(serde_json::to_string(&UiCommand::BackHome).unwrap(), r#""backHome""#);
    assert_eq!(serde_json::to_string(&UiCommand::Place { x: 1, y: 2 }).unwrap(), r#"{"place":{"x":1,"y":2}}"#);
    assert_eq!(serde_json::to_string(&UiCommand::SendChat("嗨".into())).unwrap(), r#"{"sendChat":"嗨"}"#);
    assert_eq!(
        serde_json::to_string(&UiCommand::AcceptInvite { inviter_id: "u-a".into(), pwd: None, kind: "go".into(), size: 9, rtc: None, spec: true }).unwrap(),
        r#"{"acceptInvite":{"inviterId":"u-a","pwd":null,"kind":"go","size":9,"rtc":null,"spec":true}}"#
    );
    // 未知标签必须报错，不能静默吞掉（否则前端拼错命令名 = 无声无息）
    assert!(serde_json::from_str::<UiCommand>(r#"{"noSuchCmd":null}"#).is_err());
}

/* ---------------- 回归：受理挑战切 channel / sync_epoch 复位 ---------------- */

/// 同源挑战受理后必须切到受邀者的 game channel（两人同一 channel）。
/// 回归：accept_challenge_with 采纳了受邀者 gameId 却不发 JoinChannel（TS 参考实现
/// 在同位置有 transport.join，迁移时漏掉），双方 Hello/SyncRequest 互相不可达，
/// peer_connected 恒 false、can_place 拒绝一切落子，全程无报错。手动受理与 pwd 自动
/// 受理共用同一函数，两条入口都锁。
#[test]
fn challenge_accept_joins_the_invitee_channel() {
    let c = ctx(1000);
    // 手动受理路径：主页收到挑战 → 弹窗确认。
    let mut a = mk("a", false);
    reduce(
        &mut a,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-b".into(),
            from_name: "乙".into(),
            pwd: None,
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-b".into(),
            rtc_ans: None,
        }),
        &c,
    );
    assert!(a.incoming.is_some(), "主页收到挑战应弹窗待确认");
    let fx = reduce(&mut a, Event::Ui(UiCommand::AcceptChallenge), &c);
    assert_eq!(a.game_id.as_deref(), Some("g-b"), "受理即采纳受邀者的 gameId");
    assert!(
        fx.iter().any(|e| matches!(e, Effect::JoinChannel(g) if g == "g-b")),
        "受理后必须切到受邀者的 game channel；实际 effects={:?}",
        fx.iter().map(|e| format!("{e:?}").chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
    // pwd 自动受理路径（等待中的邀请者收到带本局钥匙的挑战）。
    let mut w = mk("w", false);
    reduce(&mut w, Event::Ui(UiCommand::CreateInvite), &c);
    let pwd = w.pwd.clone().unwrap();
    let fx = reduce(
        &mut w,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-c".into(),
            from_name: "丙".into(),
            pwd: Some(pwd),
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-c".into(),
            rtc_ans: None,
        }),
        &c,
    );
    assert_eq!(w.phase, Phase::Playing, "钥匙对的挑战应自动受理");
    assert!(fx.iter().any(|e| matches!(e, Effect::JoinChannel(g) if g == "g-c")));
    // 受理也是新对局入口：带悔棋史的会话（旧纪元）受理后必须归零，
    // 否则会把新对手（epoch 0）的开局快照按 `sv < epoch` 静默全部丢弃。
    let mut old = mk("old", false);
    old.sync_epoch = 5;
    reduce(
        &mut old,
        Event::Presence(PresenceEvt::Challenge {
            from: "u-d".into(),
            from_name: "丁".into(),
            pwd: None,
            kind: "gomoku".into(),
            size: 15,
            game_id: "g-d".into(),
            rtc_ans: None,
        }),
        &c,
    );
    reduce(&mut old, Event::Ui(UiCommand::AcceptChallenge), &c);
    assert_eq!(old.sync_epoch, 0, "受理挑战是新局入口，epoch 必须归零");
}

/// sync_epoch 跨局必须复位：会话在同一标签页/原生宿主里跨局存活，上一局悔棋攒下的
/// 纪元会把下一局新对手（epoch 0）发来的快照按 `sv < epoch` 静默全部丢弃——漏手补
/// 同步与观战开局同步双双失效且无任何提示。复位放在每个新对局/观战入口，
/// **不放 reset_board_for**（局内重开必须继续 +1，且线上 Reset 消息处理器与之共用）。
#[test]
fn sync_epoch_resets_at_new_game_entries() {
    let mut a = mk("a", true);
    let mut b = mk("b", true);
    let c = ctx(1000);
    // —— 第 1 局：A/B 协商悔棋成功，纪元各 +1 ——
    reduce(&mut a, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("O".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    let pwd = a.pwd.clone().unwrap();
    reduce(&mut a, Event::Server(ServerEvt::Signal { from: "u-b".into(), kind: "join".into(), payload: serde_json::json!({ "pwd": pwd, "name": "乙" }) }), &c);
    reduce(&mut b, Event::Server(ServerEvt::State { s: "ready".into(), detail: None }), &c);
    reduce(&mut b, Event::Server(ServerEvt::Signal { from: "u-a".into(), kind: "offer".into(), payload: serde_json::json!({ "name": "甲", "kind": "gomoku", "size": 15, "gameId": "g-1", "offer": "O" }) }), &c);
    a.phase = Phase::Playing;
    a.opponent = Some("u-b".into());
    a.peer_connected = true;
    b.phase = Phase::Playing;
    b.opponent = Some("u-a".into());
    b.peer_connected = true;
    // A 落一手（悔棋请求的 history 非空守卫才放行）。
    let fx = reduce(&mut a, Event::Ui(UiCommand::Place { x: 7, y: 7 }), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    // B 请求悔棋 → A 同意 → 双方各自应用、纪元 +1。
    let fx = reduce(&mut b, Event::Ui(UiCommand::RequestUndo), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut a, Event::Net(m), &c);
    }
    let fx = reduce(&mut a, Event::Ui(UiCommand::ConfirmApprove), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut b, Event::Net(m), &c);
    }
    assert_eq!(a.sync_epoch, 1, "悔棋后纪元 +1");
    assert_eq!(b.sync_epoch, 1);
    // —— 回主页开第 2 局：新对局入口必须归零 ——
    reduce(&mut a, Event::Ui(UiCommand::BackHome), &c);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    assert_eq!(a.sync_epoch, 0, "新局入口不复位，旧纪元会拒收新对手的低 sv 快照");
    // —— 第 2 局对全新会话 N（epoch=0）：A 漏掉 N 的那手，靠快照补同步 ——
    // （直连 IO 不在本层：对局态与连接就绪照既有用例惯例直接摆出。）
    a.phase = Phase::Playing;
    a.peer_connected = true;
    let mut n = mk("n", true);
    n.phase = Phase::Playing;
    n.role = Role::Invitee;
    n.my_color = "white".into();
    n.peer_connected = true;
    // A(黑) 落一手送达 N。
    let fx = reduce(&mut a, Event::Ui(UiCommand::Place { x: 7, y: 7 }), &c);
    if let Some(m) = find_broadcast(&fx) {
        reduce(&mut n, Event::Net(m), &c);
    }
    // N(白) 落一手但对 A 隐去（模拟漏手，不发广播给 A）。
    reduce(&mut n, Event::Ui(UiCommand::Place { x: 8, y: 8 }), &c);
    // A 发 SyncRequest，N 回全量快照（sv=0）。
    let fx = reduce(&mut a, Event::Timer("hello-delay"), &c);
    for e in fx {
        if let Effect::Broadcast(m) = e {
            if matches!(m.kind, MsgKind::SyncRequest) {
                let reply = reduce(&mut n, Event::Net(m), &c);
                if let Some(snap) = find_broadcast(&reply) {
                    assert!(matches!(snap.kind, MsgKind::SyncState { sv: Some(0), .. }));
                    reduce(&mut a, Event::Net(snap), &c);
                }
            }
        }
    }
    assert_eq!(a.history.len(), 2, "新对手的 sv=0 快照必须被采纳（修复前被旧纪元静默丢弃）");
    assert_eq!(a.board[8][8], "white", "漏掉的那手经快照补齐");
    // —— 观战变体：曾当对局者并悔过棋的会话进观战，开局快照必须被采纳 ——
    let mut sp = mk("sp", false);
    sp.sync_epoch = 3;
    reduce(&mut sp, Event::Boot { href: "https://x.dev/watch/g-9".into() }, &c);
    assert_eq!(sp.role, Role::Spectator);
    assert_eq!(sp.sync_epoch, 0, "观战入局入口必须归零");
    let mut board = vec![vec!["empty".to_string(); 9]; 9];
    board[3][3] = "black".into();
    board[4][4] = "white".into();
    let snap = GameMsg::new(
        1,
        "peer-h",
        "u-h",
        MsgKind::SyncState {
            sv: Some(0),
            board,
            to_move: "black".into(),
            winner: None,
            history: vec![
                crate::protocol::HistoryEntry::Place(CoordT { x: 3, y: 3 }),
                crate::protocol::HistoryEntry::Place(CoordT { x: 4, y: 4 }),
            ],
            last_move: Some(CoordT { x: 4, y: 4 }),
            kind: "go".into(),
            size: 9,
        },
    );
    reduce(&mut sp, Event::Net(snap), &c);
    assert_eq!(sp.board[3][3], "black", "房主的开局快照必须被采纳");
    assert_eq!(sp.history.len(), 2);
}

/* ---------------- 回归：观战者应用 Pass / 观战槽连接灯 ---------------- */

/// 观战者必须像对局者一样应用 Move{Pass}：围棋双 Pass 是每局必然到达的一手，
/// 被角色守卫丢掉的话 to_move 永不翻转、终局计分永远进不去，观战从此冻结。
/// 回归：Pass 分支曾多加 `|| role == Spectator`（历史 TS 只按 by!==toMove 守卫）。
#[test]
fn spectator_applies_pass_moves_like_place() {
    let mut s = mk("s", false);
    let c = ctx(1000);
    s.role = Role::Spectator;
    s.phase = Phase::Playing;
    s.peer_connected = true;
    s.engine = goptop_core::game::GameState::new(goptop_core::game::GameKind::Go { size: 9 });
    sync_mirror_from_engine(&mut s);
    // 黑停一手：观战者照常应用，to_move 翻转。
    reduce(&mut s, Event::Net(GameMsg::new(1, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Pass, by: "black".into() })), &c);
    assert_eq!(s.to_move, "white", "观战者的 Pass 也必须翻转行棋方");
    assert_eq!(s.history.len(), 1);
    // 白停一手 → 双 Pass 终局：观战者同样进计分态。
    reduce(&mut s, Event::Net(GameMsg::new(2, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Pass, by: "white".into() })), &c);
    assert!(s.scoring, "双 Pass 后观战者应进终局计分态");
    assert_eq!(s.to_move, "black", "两手都应用、轮转两次");
    assert!(s.pending_moves.is_empty());
    // 中盘序列锁：停一手之后的落子照常轮转补应用（乱序暂存机制不得被本修复破坏）。
    let mut g = mk("g", false);
    g.role = Role::Spectator;
    g.phase = Phase::Playing;
    g.peer_connected = true;
    g.engine = goptop_core::game::GameState::new(goptop_core::game::GameKind::Go { size: 9 });
    sync_mirror_from_engine(&mut g);
    reduce(&mut g, Event::Net(GameMsg::new(1, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Pass, by: "black".into() })), &c);
    reduce(&mut g, Event::Net(GameMsg::new(2, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 2, y: 3 } }, by: "white".into() })), &c);
    reduce(&mut g, Event::Net(GameMsg::new(3, "peer-a", "u-a", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 5, y: 6 } }, by: "black".into() })), &c);
    assert_eq!(g.board[3][2], "white", "board 按 [y][x] 索引");
    assert_eq!(g.board[6][5], "black");
    assert!(g.pending_moves.is_empty());
    assert_eq!(g.to_move, "white", "停一手加两手落子共三手，轮回白");
}

/// 连接状态灯只跟对手（player）连接走：观战槽 open 不得清 conn_lost、不得置
/// peer_connected——否则对手失联后观战者一重连，红灯翻绿「已连接」，对局者继续把
/// 落子广播进只剩观战者的通道，全程无报错。与 gone 分支的 is_player 过滤对称。
#[test]
fn spectator_slot_open_does_not_mark_opponent_connected() {
    let c = ctx(1000);
    // (1) 对局中：对手连接断开亮红灯后，观战槽 open 不得洗白。
    let mut a = mk("a", false);
    a.role = Role::Inviter;
    a.phase = Phase::Playing;
    a.peer_connected = true;
    a.rtc_peers.push(PeerSlot { tag: "main".into(), player: true, spectator: false, opened: true, offer_ready: true, offer_plain: None, awaiting_peer: None });
    a.rtc_peers.push(PeerSlot { tag: "spec-live-0".into(), player: false, spectator: true, opened: false, offer_ready: true, offer_plain: None, awaiting_peer: None });
    reduce(&mut a, Event::PeerState { tag: "main".into(), opened: false, closed: true, failed: false }, &c);
    assert!(a.conn_lost, "对手连接断开应亮「已中断」");
    reduce(&mut a, Event::PeerState { tag: "spec-live-0".into(), opened: true, closed: false, failed: false }, &c);
    assert!(a.conn_lost, "观战槽 open 不得把对手失联洗回「已连接」");
    // (2) 等待中的房主只受理了观战回执：观战槽 open 不是「对手已连接」，
    // phase 也不得被带进对局（否则棋盘对一手都没同步的局放行落子）。
    let mut w = mk("w", false);
    w.role = Role::Inviter;
    w.phase = Phase::Waiting;
    w.rtc_peers.push(PeerSlot { tag: "spec-live-0".into(), player: false, spectator: true, opened: false, offer_ready: true, offer_plain: None, awaiting_peer: None });
    reduce(&mut w, Event::PeerState { tag: "spec-live-0".into(), opened: true, closed: false, failed: false }, &c);
    assert!(!w.peer_connected, "观战槽 open 不是「对手已连接」");
    assert_eq!(w.phase, Phase::Waiting);
}

/* ---------------- 回归：坏回执后同钥匙重试 ---------------- */

/// 坏回执受理后同钥匙的重试必须仍被受理：进对局被刻意推迟到 PeerState open
///（「坏回执保持 waiting 可重试」），受理就清 pwd 的话，waiting 配 None 钥匙，
/// 第二次回执会命中「回执钥匙与本局不符」被拒、同源自动受理同样被拒——重试设计
/// 自相矛盾。钥匙改到进对局那一刻消费（与受邀方 enter_playing_as_invitee 对齐）。
#[test]
fn bad_receipt_keeps_pwd_so_same_key_retry_is_accepted() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: None, answer_plain: None, offer_enc: Some("G1INVITE".into()), answer_enc: None }, &c);
    let pwd = a.pwd.clone().unwrap();
    // 第一份回执：钥匙正确、answer 是死载荷（对端 PC 已关，ICE 永远不通——
    // transport 侧静默失败，状态机只看到「已受理」）。
    let bad = AnswerIntent {
        inviter_id: a.user_id.clone(),
        pwd: pwd.clone(),
        rtc_ans: "G1dead-answer".into(),
        spectator: false,
        game_id: Some("g-b".into()),
        kind: Some("gomoku".into()),
        size: Some(15),
    };
    let fx = reduce(&mut a, Event::Ui(UiCommand::AcceptReceipt(Box::new(bad))), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { .. })), "第一份回执应被受理并交给 transport");
    assert_eq!(a.phase, Phase::Waiting, "进对局留给直连 open（坏回执保持等待可重试）");
    assert!(a.pwd.is_some(), "受理不得当场清钥匙——否则重试窗口焊死");
    // 第二份回执：同一把钥匙的正确 answer。修复前被「回执钥匙与本局不符」打回。
    let ans2 = crate::codec::encode(r#"{"s":"SDP-B2","t":"answer","r":"player"}"#, &pwd).unwrap();
    let good = AnswerIntent {
        inviter_id: a.user_id.clone(),
        pwd: pwd.clone(),
        rtc_ans: ans2.clone(),
        spectator: false,
        game_id: Some("g-b".into()),
        kind: Some("gomoku".into()),
        size: Some(15),
    };
    let fx = reduce(&mut a, Event::Ui(UiCommand::AcceptReceipt(Box::new(good))), &c);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::AcceptAnswer { answer, pwd: Some(p), .. } if answer == &ans2 && p == &pwd)),
        "同钥匙重试必须被受理；实际 effects={:?}",
        fx.iter().map(|e| format!("{e:?}").chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("本局不符"))));
    assert!(a.pwd.is_some(), "重试受理后钥匙仍在，等进对局才消费");
    // 直连 open → 进对局，钥匙此刻才失效。
    reduce(&mut a, Event::PeerState { tag: "main".into(), opened: true, closed: false, failed: false }, &c);
    assert_eq!(a.phase, Phase::Playing);
    assert!(a.peer_connected);
    assert!(a.pwd.is_none(), "钥匙只在进对局那一刻消费");
}

/* ---------------- 回归：线上快照/Reset 的尺寸防御 ---------------- */

/// 远端 SyncState 的 ragged 棋盘必须整体拒绝（先校验后落账）：镜像曾被先整块入库
/// 再校验，校验失败只 early-return，镜像已被污染——随后 apply_move/can_place 的
/// `board[y][x]` 越界 panic，release panic="abort" 直接进程闪退。
#[test]
fn ragged_sync_state_is_rejected_without_polluting_the_mirror() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    a.phase = Phase::Playing;
    a.role = Role::Inviter;
    a.my_color = "black".into();
    a.peer_connected = true;
    // ragged：15 行但第 0 行只有 1 个元素；sv 抬高绕过双键守卫，to_move 改成对端色。
    let mut board = vec![vec!["empty".to_string(); 15]; 15];
    board[0] = vec!["black".to_string()];
    let dirty = GameMsg::new(1, "peer-b", "u-b", MsgKind::SyncState { sv: Some(9999), board, to_move: "white".into(), winner: None, history: vec![], last_move: None, kind: "gomoku".into(), size: 15 });
    let fx = reduce(&mut a, Event::Net(dirty), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("已忽略"))), "脏快照须提示并忽略");
    // 镜像零突变：修复前 board[0] 已被污染成长度 1，随后任何 [0][x] 索引即越界。
    assert_eq!(a.board.len(), 15);
    assert!(a.board.iter().all(|r| r.len() == 15), "镜像不得被 ragged 快照污染");
    assert_eq!(a.board[0][0], "empty");
    assert_eq!(a.to_move, "black");
    assert!(a.winner.is_none() && a.history.is_empty());
    // 后续落子（本地点击 + 远端 Move）都不得 panic，局面照常推进。
    reduce(&mut a, Event::Ui(UiCommand::Place { x: 5, y: 0 }), &c);
    reduce(&mut a, Event::Net(GameMsg::new(2, "peer-b", "u-b", MsgKind::Move { move_: MoveT::Place { coord: CoordT { x: 6, y: 0 } }, by: "white".into() })), &c);
    assert_eq!(a.board[0][5], "black", "board 按 [y][x] 索引");
    assert_eq!(a.board[0][6], "white");
    // 异尺寸方形快照（9×9 对 15 路局）：整体拒绝、引擎不换、镜像仍 15×15。
    let small = GameMsg::new(3, "peer-b", "u-b", MsgKind::SyncState { sv: Some(10000), board: vec![vec!["empty".to_string(); 9]; 9], to_move: "black".into(), winner: None, history: vec![], last_move: None, kind: "gomoku".into(), size: 9 });
    let fx = reduce(&mut a, Event::Net(small), &c);
    assert!(fx.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("已忽略"))));
    assert_eq!(a.kind, "gomoku");
    assert_eq!(a.size, 15);
    assert_eq!(a.board.len(), 15);
    assert_eq!(a.engine.kind.size(), 15);
}

/// 线上 Reset 消息的 size 必须归一化后才进 empty_board：该消息无 phase/role 守卫、
/// 不去重、size 是无校验的 u16——65535 直通曾打出 4e9 个 String 的分配 → OOM abort，
/// 一条 ~70 字节消息在任意 phase 打死任一端，可重复。归一化同时保持 (kind,size) 与
/// 引擎一致：("go",15) 曾留下 self.size=15 而引擎是 Go{19} 的错位。
#[test]
fn reset_net_message_normalizes_hostile_kind_size() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    a.phase = Phase::Playing;
    a.peer_connected = true;
    // 修复前下一行直接 abort 测试进程（分配失败）——缺陷的可执行证明。
    reduce(&mut a, Event::Net(GameMsg::new(1, "peer-b", "u-b", MsgKind::Reset { kind: "gomoku".into(), size: 65535 })), &c);
    assert_eq!(a.size, 15);
    assert_eq!(a.board.len(), 15);
    assert_eq!(a.board[0].len(), 15);
    assert_eq!(a.engine.kind.size(), 15);
    // 非法组合按棋类回默认尺寸（对齐 pick_kind 的原子修正）。
    reduce(&mut a, Event::Net(GameMsg::new(2, "peer-b", "u-b", MsgKind::Reset { kind: "go".into(), size: 15 })), &c);
    assert_eq!(a.kind, "go");
    assert_eq!(a.size, 19, "(kind,size) 必须与引擎一致，不得留下 go/15 的错位");
    assert_eq!(a.board.len(), 19);
    assert_eq!(a.engine.kind.size(), 19);
}

/* ---------------- 回归：answer 应用失败不再静默（stale 回执重试） ---------------- */

/// 邀请者在等待态收到 RtcApplyFailed → 必须给出可行动的提示。
///
/// 场景：旧回执已应用（→stable）但连接未成，对方补发新回执；状态机已受理重试
/// （128502f），transport 的二次 SRD 却失败——此前该失败被静默吞掉，界面停在
/// 「连接中」毫无反应。
#[test]
fn rtc_apply_failed_in_waiting_notices_the_inviter() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("OFFER_A".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    assert!(a.phase == Phase::Waiting && a.role == Role::Inviter);
    let fx = reduce(&mut a, Event::RtcApplyFailed { tag: "main".into() }, &c);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("回执"))),
        "等待态的邀请者必须收到指引提示，实际 effects={fx:?}"
    );
}

/// 同一事件在非等待态（对局中/受邀者/已回主页）没有可行动的恢复动作 → 维持静默。
#[test]
fn rtc_apply_failed_outside_waiting_is_silent() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("OFFER_A".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    // 走完受理进对局（PeerState open：受邀者/邀请者共用这条入口）。
    reduce(&mut a, Event::PeerState { tag: "main".into(), opened: true, closed: false, failed: false }, &c);
    assert_eq!(a.phase, Phase::Playing);
    let fx = reduce(&mut a, Event::RtcApplyFailed { tag: "main".into() }, &c);
    assert!(fx.is_empty(), "对局中的应用失败没有恢复动作，不得弹提示，实际={fx:?}");
    // 已回主页（backHome 后 phase=Home）的残留事件同样静默。
    let mut b = mk("b", false);
    reduce(&mut b, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut b, Event::Ui(UiCommand::BackHome), &c);
    let fx = reduce(&mut b, Event::RtcApplyFailed { tag: "main".into() }, &c);
    assert!(fx.is_empty(), "残留事件不得对已回主页的会话弹提示，实际={fx:?}");
}

/// **升级时间线**：receipt1 应用后 ICE 最终升级 failed（PeerState failed → gone 分支
/// 把 main 槽位移除），此时 receipt2 的二次 SRD 失败事件**仍必须弹提示**。
///
/// 实测 Chrome 可能长时间停在 disconnected 不升级（那条时间线槽位还在、E2E
/// receipt-retry.js 已压全链）；本测试钉住「升级后槽位已被移除、accept_receipt 又不
/// 重新登记」的另一条时间线——守卫若按槽位在册过滤，这条线上会永远静默。
#[test]
fn rtc_apply_failed_after_ice_gone_still_notices() {
    let mut a = mk("a", false);
    let c = ctx(1000);
    reduce(&mut a, Event::Ui(UiCommand::CreateInvite), &c);
    reduce(&mut a, Event::RtcReady { tag: "main".into(), offer_plain: Some("OFFER_A".into()), answer_plain: None, offer_enc: None, answer_enc: None }, &c);
    // receipt1 已应用（SRD 成功 → stable），但对端已关：ICE 失败 → gone 分支移除槽位。
    reduce(&mut a, Event::PeerState { tag: "main".into(), opened: false, closed: true, failed: true }, &c);
    assert!(a.phase == Phase::Waiting, "连接从未 open，必须还留在等待态");
    assert!(!a.rtc_peers.iter().any(|q| q.tag == "main"), "gone 分支应已移除 main 槽位（复刻真路径前提）");
    // receipt2 的 SRD 失败事件到达：仍要提示。
    let fx = reduce(&mut a, Event::RtcApplyFailed { tag: "main".into() }, &c);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Notice(Some(t), _) if t.contains("回执"))),
        "槽位被 ICE 失败移除后，重试的失败事件仍必须提示，实际 effects={fx:?}"
    );
}
