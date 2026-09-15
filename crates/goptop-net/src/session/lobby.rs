//! 大厅域 — 邀请/回执流程、URL 意图、presence 同源信令、RTC 产物分派、
//! 无服务器跨设备观战（回执模型扩展）。

use super::*;
use crate::links::{AnswerIntent, UrlIntent};
use crate::links;

impl Session {
    /// 展示名（昵称回退 userId 前 8 位）。
    pub(crate) fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.user_id.chars().take(8).collect()
        } else {
            self.name.clone()
        }
    }

    /// 关闭全部 P2P 连接与棋盘 channel（换局入口与 backHome 共用）。
    pub(crate) fn close_all_rtc(&mut self) {
        self.rtc_peers.clear();
        self.inviter_main = None;
        self.relay_targets.clear();
        self.opponent = None;
        self.conn_lost = false;
        // 观战聊天权限是「本局」概念，不复位会带进新局。
        self.spec_can_chat = false;
        self.spec_chat_acks.clear();
        self.spec_denied = false;
        self.incoming = None;
        self.pending_specs.clear();
    }

    /// 回主页：服务器模式对局者退出前先散场自己名下的观战者。
    pub(crate) fn do_back_home(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.role != Role::Spectator && self.server_mode && self.server_state == "ready" {
            for s in &self.spectators {
                if s.host == self.user_id {
                    fx.push(self.signal_effect(&s.id, "spec-kicked", serde_json::json!({ "name": self.display_name() })));
                }
            }
        }
        fx.push(Effect::ClosePeers);
        fx.push(Effect::LeaveChannel);
        self.close_all_rtc();
        // 观战钥匙不清理会泄漏到下一局。
        self.spec_pwd = None;
        self.spec_url = None;
        self.spectate_enabled = true;
        self.spectators.clear();
        self.spec_requests.clear();
        self.role = Role::Idle;
        self.phase = Phase::Home;
        self.game_id = None;
        self.pwd = None;
        self.invite_url = None;
        self.watch_url = None;
        self.spec_answer = None;
        self.peer_connected = false;
        self.my_color = "black".into();
        // 计分态残留清理。
        self.reset_score_state();
        let (board, _) = empty_board(self.size);
        self.board = board;
        self.winner = None;
        self.history.clear();
        self.last_move = None;
        self.engine = goptop_core::game::GameState::new(make_engine_kind(&self.kind.clone(), self.size));
        fx.push(Effect::Notice(None, None));
        fx.push(Effect::Nav("/".into()));
        fx.push(self.announce_effect());
        fx.push(Effect::Emit);
        fx
    }

    pub(crate) fn announce_effect(&self) -> Effect {
        self.announce()
    }

    /// 重置棋盘到指定规则（换局/对局开始共用；sv 不动——调用点均在换局后）。
    pub(crate) fn reset_board_for(&mut self, kind: &str, size: SizeT) -> Vec<Effect> {
        self.kind = kind.to_string();
        self.size = size;
        self.engine = goptop_core::game::GameState::new(make_engine_kind(kind, size));
        let (board, _) = empty_board(size);
        self.board = board;
        self.to_move = "black".into();
        self.winner = None;
        self.history.clear();
        self.last_move = None;
        self.scoring = false;
        self.reset_score_state();
        vec![Effect::Emit]
    }

    pub(crate) fn reset_score_state(&mut self) {
        self.my_dead.clear();
        self.peer_dead.clear();
        self.my_score_ok = false;
        self.peer_score_ok = false;
        self.score_result = None;
    }
}

/* ---------------- 启动与导航 ---------------- */

/// 启动：处理地址栏意图。
pub(crate) fn on_boot(s: &mut Session, href: &str) -> Vec<Effect> {
    s.processed_href = Some(href.to_string());
    process_intent(s, &ctx_fallback(), &links::parse_url(href))
}

/// 站内导航/前进后退后的意图处理。
pub(crate) fn on_navigate(s: &mut Session, href: &str) -> Vec<Effect> {
    if s.processed_href.as_deref() == Some(href) {
        return Vec::new();
    }
    s.processed_href = Some(href.to_string());
    process_intent(s, &ctx_fallback(), &links::parse_url(href))
}

/// Boot/Navigate 不需要随机源：意图处理本身不生成随机值。
/// （函数签名仍带 ctx 以防未来扩展。）
#[allow(clippy::needless_pass_by_value)]
fn ctx_fallback() -> ReduceCtx {
    ReduceCtx { now_ms: 0, rand: [0; 4] }
}

/// 按 URL 意图行动：观战 / 邀请 / 服务器 userId 链接（pwd 客户端校验）。
fn process_intent(s: &mut Session, ctx: &ReduceCtx, it: &UrlIntent) -> Vec<Effect> {
    let mut fx = Vec::new();
    match it {
        // —— 观战意图 ——
        UrlIntent::Watch { game_id } if s.phase == Phase::Home => {
            fx.extend(join_as_spectator_local(s, game_id));
        }
        // —— userId 链接 ——
        UrlIntent::User { user_id, pwd, kind, size, rtc, spec } if s.phase == Phase::Home => {
            if user_id == &s.user_id {
                return fx; // 自己的主页
            }
            if *spec {
                // 观战链接（spec=1）：服务器模式走信令；无服务器走 specrtc offer 直连。
                if s.server_mode {
                    s.pending_link = Some(PendingLink { target: user_id.clone(), pwd: pwd.clone(), spec: true });
                    fx.extend(s.flush_pending_link());
                } else if let Some(offer) = rtc {
                    fx.extend(accept_spec_offer_serverless(s, ctx, user_id.clone(), pwd.clone(), offer.clone()));
                } else {
                    fx.push(Effect::Notice(Some("此观战链接缺少直连信令：请向房主要求含直连参数的最新链接".into()), Some(4200)));
                }
                return fx;
            }
            if let Some(p) = pwd {
                if s.server_mode && rtc.is_none() {
                    // 服务器模式邀请链接：pwd 校验在对局者端（错误会转弹窗询问）。
                    s.pending_link = Some(PendingLink { target: user_id.clone(), pwd: Some(p.clone()), spec: false });
                    fx.extend(s.flush_pending_link());
                    return fx;
                }
                if s.role == Role::Idle {
                    // 回执链接被当页面打开：绝不据此发起挑战（同源两窗互弈防线）。
                    if href_has_receipt(rtc) {
                        fx.push(Effect::Notice(Some("这是回执链接：请在邀请者的「等待对手」页点「输入回执」粘贴它".into()), Some(3200)));
                        return fx;
                    }
                    let href_key = current_href_marker(user_id);
                    if s.invite_done_href.as_deref() == Some(href_key.as_str()) {
                        return fx;
                    }
                    s.invite_done_href = Some(href_key);
                    // 带 pwd 打开：自动发起带钥匙连接；邀请者校验后自动同意。
                    fx.extend(accept_invite(s, ctx, user_id, Some(p.clone()), kind, *size, rtc.clone()));
                }
            }
            // 无 pwd：仅展示对方主页，由用户手动挑战（无 effect）。
        }
        _ => {}
    }
    fx
}

/// 无服务器模式：回执意图判定（旧 TS 用 URL ?rtcAns=；Rust 版沿用 rtcAns 参数）。
/// 此处 rtc 参数存在与否用于区分「邀请 offer 链接」与「主页展示链接」。
fn href_has_receipt(rtc: &Option<String>) -> bool {
    rtc.is_none()
}

fn current_href_marker(user_id: &str) -> String {
    format!("user:{user_id}")
}

/* ---------------- 邀请 / 回执 ---------------- */

/// 邀请者：开启对战（waiting），生成 pwd/gameId/specPwd，预生成直连 offer
/// （无服务器编进邀请链接；服务器等 join 受理后发出）。无服务器模式同时预生成
/// 观战 offer（&specrtc=，跨设备观战）。挑战受理流复用本函数。
pub(crate) fn create_invite(s: &mut Session, ctx: &ReduceCtx) -> Vec<Effect> {
    // 连通性前置：服务器未连接时不写任何状态（防脏等待态）。
    if s.server_mode && s.server_state != "ready" {
        return vec![Effect::Notice(Some("服务器未连接：请检查设置页服务器配置或切换到无服务器模式".into()), None)];
    }
    s.close_all_rtc();
    let g = crate::identity::gen_game_id(ctx.now_ms, ctx.rand[0]);
    let p = crate::identity::gen_pwd(ctx.rand[1]);
    s.game_id = Some(g.clone());
    s.pwd = Some(p.clone());
    s.role = Role::Inviter;
    s.phase = Phase::Waiting;
    s.my_color = "black".into();
    s.peer_connected = false;
    s.invite_url = None;
    s.watch_url = None;
    s.spec_url = None;
    s.spec_pwd = Some(crate::identity::gen_pwd(ctx.rand[2]));
    s.spectate_enabled = true;
    let mut fx = s.reset_board_for(&s.kind.clone(), s.size);
    fx.push(Effect::JoinChannel(g.clone()));
    s.inviter_main = Some("main".into());
    s.rtc_peers.push(PeerSlot { tag: "main".into(), player: true, spectator: false, opened: false, offer_ready: false, offer_plain: None, awaiting_peer: None });
    fx.push(Effect::CreatePeer { tag: "main".into(), inviter: true, spectator: false });
    if s.server_mode {
        // 服务器模式：链接回归 /<userId>?pwd=（无 rtc，最短）；pwd 校验在收到 join
        // signal 时于本端进行（错误转弹窗询问），offer 在受理后发出。
        s.invite_url = Some(links::invite_to_url(&s.share_origin, &s.user_id, &p, &s.kind, s.size, None));
        fx.push(Effect::Notice(None, None));
    } else {
        // 无服务器：offer 生成完成后（RtcReady）回填邀请链接与观战链接；
        // 生成前不提供链接（无 rtc 的链接跨设备永远连不上）。
        fx.push(Effect::Notice(Some("正在生成直连邀请…".into()), None));
    }
    fx.push(s.announce_effect());
    fx.push(Effect::Emit);
    fx
}

/// 受邀者：向某用户发起连接（粘贴链接/主页挑战；pwd 可空）。
pub(crate) fn accept_invite(s: &mut Session, ctx: &ReduceCtx, inviter_id: &str, pwd: Option<String>, kind: &str, size: SizeT, rtc_offer: Option<String>) -> Vec<Effect> {
    s.close_all_rtc();
    let g = crate::identity::gen_game_id(ctx.now_ms, ctx.rand[0]);
    s.game_id = Some(g.clone());
    s.role = Role::Invitee;
    s.my_color = "white".into();
    s.peer_connected = false;
    let mut fx = s.reset_board_for(kind, size);
    fx.push(Effect::JoinChannel(g.clone()));
    s.phase = Phase::Waiting;
    s.pwd = None;
    s.invite_url = None;
    s.watch_url = None;
    s.answer_back_url = None;
    s.outgoing_challenge = None;
    match rtc_offer {
        Some(offer) => {
            // 跨设备一键直连：邀请链接自带 offer，受邀者生成 answer 后：
            // - 同源 answer 经 presence 自动回传；
            // - 跨设备弹窗展示回执链接，发回邀请者粘贴完成。
            s.phase = Phase::Waiting;
            fx.push(Effect::Notice(Some("邀请已受理，正在建立 P2P 直连…".into()), None));
            fx.push(Effect::Nav("/p2p".into()));
            s.rtc_peers.push(PeerSlot { tag: "main".into(), player: true, spectator: false, opened: false, offer_ready: false, offer_plain: None, awaiting_peer: None });
            s.my_host = Some(inviter_id.to_string());
            fx.push(Effect::FeedOffer { tag: "main".into(), offer, encrypted: true });
            fx.push(Effect::Emit);
        }
        None => {
            // 受邀者的 gameId 发给邀请者，邀请者接受后双方用受邀者的 gameId 建 channel。
            let name = s.display_name();
            fx.push(Effect::SendPresence(serde_json::json!({
                "t": "challenge", "from": s.user_id, "fromName": name, "to": inviter_id,
                "pwd": pwd, "kind": kind, "size": size, "gameId": g, "rtcAns": null,
            })));
            fx.push(Effect::Notice(
                Some(if pwd.is_some() { "已带邀请钥匙请求连接，等待邀请者自动确认…".into() } else { "已发送挑战，等待对方同意…".into() }),
                None,
            ));
            fx.push(Effect::Nav("/p2p".into()));
            fx.push(Effect::Emit);
        }
    }
    fx
}

/// 同源 presence 下行。
pub(crate) fn on_presence(s: &mut Session, ctx: &ReduceCtx, ev: PresenceEvt) -> Vec<Effect> {
    match ev {
        PresenceEvt::Peers { peers } => {
            // 服务器模式名册只信 serverChannel（两写者会互相闪烁）。
            if s.server_mode {
                return Vec::new();
            }
            s.peers = peers.into_iter().filter(|p| p.id != s.user_id).collect();
            vec![Effect::Emit]
        }
        PresenceEvt::Challenge { from, from_name, pwd, kind, size, game_id, rtc_ans } => {
            let mut fx = Vec::new();
            // 对局中 / 自己也在等待受邀：pwd 已失效或本局已满，回拒绝信——静默吞掉会让
            // 挑战方永远停在「等待对方同意」。
            if s.phase == Phase::Playing || (s.phase == Phase::Waiting && s.role == Role::Invitee) {
                fx.push(reject_presence(s, &from, &game_id));
                return fx;
            }
            if s.phase == Phase::Waiting {
                if s.pwd.is_some() && s.pwd == pwd {
                    // 带正确 pwd：自动同意（邀请钥匙语义）。
                    fx.extend(accept_challenge_with(s, ctx, &from, &kind, size, &game_id, rtc_ans, true));
                } else {
                    // waiting(inviter) 但 pwd 对不上：第三人拿旧 pwd，拒绝。
                    fx.push(reject_presence(s, &from, &game_id));
                }
                return fx;
            }
            // 主页：无 pwd 或 pwd 不对 → 弹窗手动确认。
            s.incoming = Some(Incoming { from, from_name, kind, size, game_id, rtc_ans });
            fx.push(Effect::Emit);
            fx
        }
        PresenceEvt::Accept { from, game_id } => {
            // 受邀者：自己的等待局被接受 → 进对局。
            if s.role == Role::Invitee && s.game_id.as_deref() == Some(game_id.as_str()) && s.phase == Phase::Waiting {
                let fx = enter_playing_as_invitee(s);
                let _ = from;
                return fx;
            }
            Vec::new()
        }
        PresenceEvt::Reject { from: _, game_id } => {
            if s.role == Role::Invitee && s.game_id.as_deref() == Some(game_id.as_str()) {
                let mut fx = vec![Effect::Notice(Some("对方拒绝了对局".into()), Some(3000))];
                fx.extend(s.do_back_home());
                fx
            } else {
                Vec::new()
            }
        }
    }
}

fn reject_presence(s: &Session, to: &str, game_id: &str) -> Effect {
    Effect::SendPresence(serde_json::json!({ "t": "reject", "from": s.user_id, "to": to, "gameId": game_id }))
}

/// 邀请者接受挑战（pwd 对自动 or 手动）。
pub(crate) fn accept_challenge(s: &mut Session, ctx: &ReduceCtx) -> Vec<Effect> {
    let Some(inc) = s.incoming.clone() else { return Vec::new() };
    s.incoming = None;
    accept_challenge_with(s, ctx, &inc.from, &inc.kind, inc.size, &inc.game_id, inc.rtc_ans, false)
}

fn accept_challenge_with(s: &mut Session, ctx: &ReduceCtx, from: &str, kind: &str, size: SizeT, invitee_game_id: &str, invitee_ans: Option<String>, auto: bool) -> Vec<Effect> {
    let _ = ctx;
    let mut fx = Vec::new();
    // 同源路径 answer 经 BC 送达，不阻塞进局；万一失败仅显示直连错误，棋局仍可下。
    if let Some(ans) = invitee_ans {
        if let Some(tag) = s.inviter_main.clone() {
            fx.push(Effect::AcceptAnswer { tag, answer: ans, encrypted: true });
        }
    }
    s.close_all_rtc();
    s.game_id = Some(invitee_game_id.to_string());
    s.role = Role::Inviter;
    s.phase = Phase::Playing;
    s.my_color = "black".into();
    s.peer_connected = false;
    fx.extend(s.reset_board_for(kind, size));
    s.pwd = None; // pwd 失效：两人满员
    s.invite_url = None;
    if !s.server_mode {
        s.watch_url = Some(links::watch_to_url(&s.share_origin, invitee_game_id));
    }
    let _name = s.display_name();
    fx.push(Effect::SendPresence(serde_json::json!({ "t": "accept", "from": s.user_id, "to": from, "gameId": invitee_game_id })));
    fx.push(Effect::Notice(Some(if auto { "邀请钥匙校验通过，已自动开始对局".into() } else { "已接受挑战，对局开始".into() }), None));
    fx.push(Effect::Nav("/p2p".into()));
    fx.push(Effect::Timer { id: "hello-delay", ms: 80 });
    fx.push(s.announce_effect());
    fx.push(Effect::Emit);
    fx
}

pub(crate) fn reject_challenge(s: &mut Session) -> Vec<Effect> {
    let Some(inc) = s.incoming.clone() else { return Vec::new() };
    s.incoming = None;
    vec![
        Effect::SendPresence(serde_json::json!({ "t": "reject", "from": s.user_id, "to": inc.from, "gameId": inc.game_id })),
        Effect::Notice(Some("已拒绝该挑战".into()), Some(2400)),
        Effect::Emit,
    ]
}

/// 受邀者进入对局（answer 被 accept / 直连 open 两条路径汇合）。
pub(crate) fn enter_playing_as_invitee(s: &mut Session) -> Vec<Effect> {
    if s.phase == Phase::Playing {
        return Vec::new();
    }
    s.phase = Phase::Playing;
    s.pwd = None;
    if !s.server_mode {
        s.watch_url = Some(links::watch_to_url(&s.share_origin, s.game_id.as_deref().unwrap_or("")));
    }
    vec![
        Effect::Notice(Some("对方已同意，对局开始（你执白）".into()), None),
        Effect::Timer { id: "hello-delay", ms: 80 },
        s.announce_effect(),
        Effect::Emit,
    ]
}

/// 同源观战（/watch/<gameId>，BroadcastChannel 只读）。
pub(crate) fn join_as_spectator_local(s: &mut Session, g: &str) -> Vec<Effect> {
    s.close_all_rtc();
    s.game_id = Some(g.to_string());
    s.role = Role::Spectator;
    s.phase = Phase::Playing;
    // 观战者没有执子颜色：占位 white 仅供 UI；消息判定一律用消息自带 by。
    s.my_color = "white".into();
    s.peer_connected = false;
    s.pwd = None;
    s.invite_url = None;
    s.watch_url = None;
    s.my_host = None;
    vec![
        Effect::JoinChannel(g.to_string()),
        Effect::Notice(Some("观战模式：只读同步，不可落子".into()), None),
        Effect::Timer { id: "sync-request", ms: 120 },
        s.announce_effect(),
        Effect::Emit,
    ]
}

/* ---------------- RTC 产物分派 ---------------- */

/// RTC 状态变化。
pub(crate) fn on_peer_state(s: &mut Session, tag: &str, opened: bool, gone: bool) -> Vec<Effect> {
    let mut fx = Vec::new();
    if let Some(p) = s.rtc_peers.iter_mut().find(|q| q.tag == tag) {
        if opened {
            p.opened = true;
            s.conn_lost = false;
            // 等待中的受邀者：跨设备时 presence 不可达，直连一通直接进对局。
            let should_enter = s.role == Role::Invitee && s.phase == Phase::Waiting && p.player;
            let _ = should_enter;
            let mut extra: Vec<Effect> = Vec::new();
            if should_enter {
                extra.extend(enter_playing_as_invitee(s));
            }
            s.peer_connected = true;
            fx.push(Effect::Emit);
            fx.extend(extra);
            return fx;
        }
        if gone {
            let was_open = p.opened;
            let is_player = p.player;
            s.rtc_peers.retain(|q| q.tag != tag);
            // 红灯只跟对手连接走：观战者离开不构成对局中断。
            if was_open && s.phase == Phase::Playing && is_player {
                s.conn_lost = true;
            }
            fx.push(Effect::Emit);
        }
    }
    fx
}

/// offer/answer 生成完成（transport 异步 gathering 完成）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_rtc_ready(
    s: &mut Session,
    ctx: &ReduceCtx,
    tag: &str,
    offer_plain: Option<String>,
    answer_plain: Option<String>,
    offer_enc: Option<String>,
    answer_enc: Option<String>,
) -> Vec<Effect> {
    let mut fx = Vec::new();
    // —— 受邀方向：answer 生成完成 ——
    if let Some(ans) = answer_plain.or(answer_enc) {
        if s.role == Role::Spectator {
            // 观战者：answer 回房主——服务器走信令，无服务器走观战回执链接。
            if s.server_mode {
                if let Some(t) = s.my_host.clone() {
                    fx.push(s.signal_effect(&t, "answer", serde_json::json!({ "name": s.display_name(), "answer": ans })));
                }
            } else {
                s.spec_answer = Some(ans.clone());
                let mut url = links::answer_to_url(&s.share_origin, s.my_host.as_deref().unwrap_or(tag), s.spec_pwd.as_deref().unwrap_or(""), &ans, s.game_id.as_deref(), Some(s.kind.as_str()), Some(s.size));
                url.push_str("&spec=1");
                s.answer_back_url = Some(url);
                fx.push(Effect::Notice(Some("观战回执已生成：请复制发给房主".into()), None));
            }
        } else if s.server_mode {
            // 受邀者（服务器）：answer 明文 signal 回房主。
            let t = s.opponent.clone().or_else(|| s.my_host.clone()).unwrap_or_else(|| tag.to_string());
            fx.push(s.signal_effect(&t, "answer", serde_json::json!({ "name": s.display_name(), "answer": ans })));
        } else {
            // 受邀者（无服务器）：同源经 presence 自动回传；跨设备展示回执链接。
            let host = s.my_host.clone().unwrap_or_else(|| tag.to_string());
            let gid = s.game_id.clone().unwrap_or_default();
            fx.push(Effect::SendPresence(serde_json::json!({
                "t": "challenge", "from": s.user_id, "fromName": s.display_name(), "to": host,
                "pwd": Option::<String>::None, "kind": s.kind, "size": s.size,
                "gameId": gid, "rtcAns": ans,
            })));
            let url = links::answer_to_url(&s.share_origin, &host, "", &ans, s.game_id.as_deref(), Some(s.kind.as_str()), Some(s.size));
            s.answer_back_url = Some(url);
        }
        fx.push(Effect::Emit);
        return fx;
    }
    // —— 邀请方向：offer 生成完成（offer_plain=服务器信令载荷；offer_enc=无服务器链接载荷）——
    if offer_plain.is_none() && offer_enc.is_none() {
        return fx;
    }
    let offer_plain = offer_plain.clone().or_else(|| offer_enc.clone()).unwrap_or_default();
    let enc = offer_enc.clone();
    // 观战受理流（pending_specs 优先匹配；服务器模式 tag=对端 ID，无服务器为 spec- 前缀）。
    if let Some(pos) = s.pending_specs.iter().position(|(t, _)| t == tag) {
        // 服务器模式受理流：加入名册 + 发 spec-offer + 广播名单。
        let (_, sname) = s.pending_specs.remove(pos);
        s.spectators.push(Spectator { id: tag.to_string(), name: sname, host: s.user_id.clone(), muted: false });
        fx.push(s.signal_effect(tag, "spec-offer", serde_json::json!({ "gameId": s.game_id.clone().unwrap_or_default(), "offer": offer_plain })));
        fx.extend(s.push_spec_sync());
        fx.push(Effect::Emit);
        return fx;
    }
    // 观战方向（tag 以 spec- 开头）。
    if tag.starts_with("spec-") {
        // 无服务器预生成：specrtc 直载明文 offer JSON（自动换新）。
        s.refresh_spec_url_with(ctx, tag, &offer_plain);
        fx.push(Effect::Emit);
        return fx;
    }
    // 邀请主连。
    let is_main = Some(tag.to_string()) == s.inviter_main;
    if let Some(p) = s.rtc_peers.iter_mut().find(|q| q.tag == tag) {
        p.offer_ready = true;
        p.offer_plain = Some(offer_plain.clone());
    }
    if is_main && s.phase == Phase::Waiting && s.role == Role::Inviter {
        if s.server_mode {
            // join 已受理（awaiting_peer 记录了对象）→ 立即发 offer；否则等 join。
            if let Some(p) = s.rtc_peers.iter().find(|q| q.tag == tag) {
                if let Some(peer) = p.awaiting_peer.clone() {
                    fx.extend(s.server_proceed_join(&peer, false));
                }
            }
        } else {
            // 无服务器：offer 编进邀请链接回填 + 预生成观战 offer。
            let p = s.pwd.clone().unwrap_or_default();
            match enc {
                Some(e) => {
                    s.invite_url = Some(links::invite_to_url(&s.share_origin, &s.user_id, &p, &s.kind, s.size, Some(&e)));
                    fx.push(Effect::Notice(None, None));
                }
                None => {
                    s.invite_url = Some(links::invite_to_url(&s.share_origin, &s.user_id, &p, &s.kind, s.size, None));
                    fx.push(Effect::Notice(Some("直连邀请生成失败：已生成同源链接（跨设备不可用），可取消后重开".into()), None));
                }
            }
            fx.push(Effect::CreatePeer { tag: "spec-pending".into(), inviter: true, spectator: true });
        }
    }
    fx.push(Effect::Emit);
    fx
}

impl Session {
    /// 无服务器模式：观战 offer 就绪后刷新 spec 链接（拿旧链接的观众直连会失败）。
    pub(crate) fn refresh_spec_url_with(&mut self, _ctx: &ReduceCtx, _spec_tag: &str, offer_plain: &str) {
        #[cfg(test)]
        eprintln!("[dbg-refresh] offer_plain_head={:?} spec_pwd={:?}", &offer_plain[..offer_plain.len().min(30)], self.spec_pwd);
        let sp = self.spec_pwd.clone().unwrap_or_default();
        let mut url = links::spec_link_url(&self.share_origin, &self.user_id, &sp);
        url.push_str("&specrtc=");
        url.push_str(&crate::codec::encode(offer_plain, &sp).unwrap_or_default());
        self.spec_url = Some(url);
    }
}

/* ---------------- 观战回执（无服务器跨设备） ---------------- */

/// 邀请者受理对局回执（粘贴路径）：校验钥匙 → 完成 answer 应用 → 切到受邀者
/// channel 进对局。观战回执（spectator=true）转 [`accept_spec_receipt`]。
/// 返回错误文案（None = 受理成功），由弹窗就地展示。
pub(crate) fn accept_receipt(s: &mut Session, ctx: &ReduceCtx, r: &AnswerIntent) -> Vec<Effect> {
    if r.spectator {
        // 观战回执自动识别分派（回执类型由链接属性自动判断，用户拍板）。
        return accept_spec_receipt(s, ctx, r);
    }
    if s.role != Role::Inviter || s.phase != Phase::Waiting {
        return vec![Effect::Notice(Some("当前不在等待对手状态，无法受理回执".into()), None)];
    }
    if r.inviter_id != s.user_id {
        return vec![Effect::Notice(Some(format!("回执是发给邀请者 {} 的，本页是 {}，不能代收", r.inviter_id, s.user_id)), None)];
    }
    if Some(r.pwd.as_str()) != s.pwd.as_deref() {
        return vec![Effect::Notice(Some("回执钥匙与本局不符，已拒绝".into()), None)];
    }
    let Some(tag) = s.inviter_main.clone() else {
        return vec![Effect::Notice(Some("本局邀请的直连信令未就绪（可能生成失败），无法受理回执；请取消等待后重新开战".into()), None)];
    };
    // answer 交给 transport 应用；应用结果经 PeerState open（直连建立）或失败提示呈现。
    let mut fx = vec![Effect::AcceptAnswer { tag, answer: r.rtc_ans.clone(), encrypted: true }];
    // await 间隙用户可能已取消等待：进入对局状态放 PeerState open 统一处理
    //（对齐 TS：直连建立才切 playing，坏回执保持 waiting 可重试）。
    let k = r.kind.clone().unwrap_or_else(|| s.kind.clone());
    let sz = r.size.unwrap_or(s.size);
    if let Some(gid) = r.game_id.clone() {
        // 切到受邀者的 game channel（两人同一 channel）。
        s.game_id = Some(gid.clone());
        fx.push(Effect::JoinChannel(gid.clone()));
        s.watch_url = Some(links::watch_to_url(&s.share_origin, &gid));
    }
    s.pwd = None; // pwd 失效：两人满员
    s.invite_url = None;
    s.incoming = None;
    let _ = (k, sz);
    fx.push(Effect::Notice(Some("回执已受理，直连建立中…".into()), None));
    fx.push(Effect::Nav("/p2p".into()));
    fx.push(Effect::Timer { id: "hello-delay", ms: 80 });
    fx.push(Effect::Emit);
    fx
}


/// 观众：从 spec 链接的 specrtc 参数解出 offer 并直连（生成观战回执链接）。
pub(crate) fn accept_spec_offer_serverless(s: &mut Session, ctx: &ReduceCtx, host_id: String, spec_pwd: Option<String>, specrtc_token: String) -> Vec<Effect> {
    let _ = ctx;
    // 解码 specrtc（G1，钥匙 = 观战钥匙）。
    let payload = crate::codec::decode(&specrtc_token, spec_pwd.as_deref().unwrap_or(""));
    let Ok(json) = payload else {
        return vec![Effect::Notice(Some("观战链接无法解码（可能不是最新的观战链接）".into()), Some(4200))];
    };
    // specrtc 解出的整个 JSON 就是 offer 载荷（明文 SDP 信封 {"s","t","r"}；单密钥设计，
    // 无 ["offer"] 包裹层——包裹层与直载两种口径曾不一致导致观众端解不出）。
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) else {
        return vec![Effect::Notice(Some("观战链接不完整：请向房主要求最新链接".into()), Some(4200))];
    };
    if !parsed.is_object() || parsed["s"].as_str().is_none() {
        return vec![Effect::Notice(Some("观战链接不完整：请向房主要求最新链接".into()), Some(4200))];
    }
    s.role = Role::Spectator;
    s.phase = Phase::Playing;
    s.my_color = "white".into();
    s.my_host = Some(host_id);
    s.spec_pwd = spec_pwd;
    s.spec_answer = None;
    vec![
        Effect::Nav("/p2p".into()),
        Effect::Notice(Some("正在连接对局观战…".into()), None),
        Effect::CreatePeer { tag: "spec-main".into(), inviter: false, spectator: true },
        Effect::FeedOffer { tag: "spec-main".into(), offer: json, encrypted: false },
        Effect::Emit,
    ]
}

/// 对局者：粘贴观战回执（answer）→ 受理直连 → 自动换发新观战链接。
pub(crate) fn accept_spec_receipt(s: &mut Session, ctx: &ReduceCtx, ans: &AnswerIntent) -> Vec<Effect> {
    // 钥匙校验（观战钥匙；整局有效）。
    if Some(ans.pwd.as_str()) != s.spec_pwd.as_deref() {
        return vec![Effect::Notice(Some("观战回执钥匙与本局不符，已拒绝".into()), Some(3600))];
    }
    // spec-pending 改名为 live 槽（腾出 pending 给下一个预生成），answer 喂给它。
    let live = format!("spec-live-{}", s.spec_live);
    s.spec_live += 1;
    let Some(slot) = s.rtc_peers.iter_mut().find(|q| q.tag == "spec-pending") else {
        return vec![Effect::Notice(Some("本局观战直连未就绪：请重新生成观战链接后再试".into()), Some(4200))];
    };
    slot.tag = live.clone();
    let mut fx = vec![
        Effect::RenamePeer { from: "spec-pending".into(), to: live.clone() },
        // 观众回执的 rtcAns 是明文 answer JSON（与邀请回执同形，单密钥设计）。
        Effect::AcceptAnswer { tag: live, answer: ans.rtc_ans.clone(), encrypted: false },
    ];
    // 受理即预生成下一份观战 offer（链接自动换新）。
    fx.push(Effect::CreatePeer { tag: "spec-pending".into(), inviter: true, spectator: true });
    fx.push(Effect::Notice(Some("观战回执已受理，直连建立中…".into()), None));
    fx.push(Effect::Emit);
    let _ = ctx;
    fx
}

/* ---------------- 主页设置 ---------------- */

pub(crate) fn set_name(s: &mut Session, name: &str) -> Vec<Effect> {
    let trimmed = name.trim().to_string();
    s.name = trimmed.clone();
    vec![
        Effect::SetStorage { key: "goptop:name".into(), value: Some(trimmed.clone()) },
        Effect::Notice(Some(if trimmed.is_empty() { "昵称已清空".into() } else { format!("昵称已保存：{trimmed}") }), Some(1600)),
        Effect::Emit,
    ]
}

pub(crate) fn set_avatar(s: &mut Session, data: Option<String>) -> Vec<Effect> {
    s.avatar = data.clone();
    vec![Effect::Emit]
}

/// 主页棋类切换（原子修正：五子棋固定 15；围棋原 15 落到 19）。
pub(crate) fn pick_kind(s: &mut Session, k: &str) -> Vec<Effect> {
    if s.phase != Phase::Home {
        return Vec::new();
    }
    s.kind = k.to_string();
    s.size = if k == "gomoku" {
        15
    } else if s.size == 15 {
        19
    } else {
        s.size
    };
    s.engine = goptop_core::game::GameState::new(make_engine_kind(&s.kind, s.size));
    let (board, _) = empty_board(s.size);
    s.board = board;
    s.to_move = "black".into();
    s.winner = None;
    s.history.clear();
    s.last_move = None;
    vec![Effect::Emit]
}

pub(crate) fn pick_size(s: &mut Session, sz: SizeT) -> Vec<Effect> {
    if s.phase != Phase::Home {
        return Vec::new();
    }
    s.size = sz;
    s.engine = goptop_core::game::GameState::new(make_engine_kind(&s.kind, sz));
    let (board, _) = empty_board(sz);
    s.board = board;
    s.to_move = "black".into();
    s.winner = None;
    s.history.clear();
    s.last_move = None;
    vec![Effect::Emit]
}
