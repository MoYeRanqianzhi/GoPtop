//! 对局域 — 落子/停一手/认输、协商（悔棋/重开/换棋，队列化弹窗）、聊天、
//! 围棋终局计分同步、定时器消费。

use super::*;
use goptop_core::board::{Coord, Stone};

impl Session {
    /// 对局消息统一入口（三链路汇合点）：去重后按类型分派。
    pub(crate) fn on_net(&mut self, msg: GameMsg, ctx: &ReduceCtx) -> Vec<Effect> {
        let mut fx = Vec::new();
        // 自己回流的消息（BC 会回声）直接丢弃。
        if msg.sender == self.peer_id {
            return fx;
        }
        let kind_type = match &msg.kind {
            MsgKind::Hello { .. } => "Hello",
            MsgKind::Move { .. } => "Move",
            MsgKind::SyncState { .. } => "SyncState",
            MsgKind::SyncRequest => "SyncRequest",
            MsgKind::Chat { .. } => "Chat",
            MsgKind::Ping => "Ping",
            MsgKind::Pong => "Pong",
            MsgKind::Reset { .. } => "Reset",
            MsgKind::UndoReq => "UndoReq",
            MsgKind::UndoAck { .. } => "UndoAck",
            MsgKind::ResetReq => "ResetReq",
            MsgKind::ResetAck { .. } => "ResetAck",
            MsgKind::SwapReq => "SwapReq",
            MsgKind::SwapAck { .. } => "SwapAck",
            MsgKind::ScoreMark { .. } => "ScoreMark",
            MsgKind::ScoreConfirmReq => "ScoreConfirmReq",
            MsgKind::ScoreConfirmAck { .. } => "ScoreConfirmAck",
            MsgKind::Avatar { .. } => "Avatar",
        };
        if !self.dedup.admit(&msg.sender, msg.seq, is_dedupable(kind_type)) {
            return fx;
        }
        // 观战者镜像转发：对手的数据面消息（落子/协商/聊天）经我补发给我名下的观战者
        // ——观战者与我不一定同源（BC 不可达），与对手也往往无直连。重复送达由
        // 观战者的 (sender, seq) 去重表兜底，绝不二次应用。
        let forward_to_specs = self.server_mode
            && matches!(self.role, Role::Inviter | Role::Invitee)
            && is_dedupable(kind_type)
            && !self.spectators.is_empty();
        if forward_to_specs {
            for s in &self.spectators {
                if s.host == self.user_id {
                    fx.push(Effect::SendServer(serde_json::json!({ "t": "relay", "to": s.id, "payload": msg })));
                }
            }
        }
        match msg.kind {
            MsgKind::SyncRequest => {
                self.peer_connected = true;
                fx.push(Effect::Broadcast(self.next_msg(self.sync_state_kind())));
            }
            MsgKind::SyncState { .. } => {
                self.peer_connected = true;
                fx.extend(self.apply_sync_state(&msg));
            }
            MsgKind::Hello { .. } => {
                self.peer_connected = true;
                // 我执黑则由我推全量快照（黑先权威）。
                if self.my_color == "black" && self.role != Role::Spectator {
                    fx.push(Effect::Broadcast(self.next_msg(self.sync_state_kind())));
                }
            }
            MsgKind::Move { move_, by } => fx.extend(self.on_move_net(msg.seq, move_, by)),
            MsgKind::Reset { kind, size } => {
                self.peer_connected = true;
                fx.extend(self.reset_board_for(&kind, size));
            }
            MsgKind::Chat { text } => {
                self.push_chat(&msg.user_id, &msg.sender, &text, false, ctx.now_ms);
                fx.push(Effect::Emit);
            }
            MsgKind::Avatar { data_url } => {
                if data_url.starts_with("data:image/") && data_url.len() < 20_000 {
                    self.peer_avatars.insert(msg.user_id.clone(), data_url);
                }
                fx.push(Effect::Emit);
            }
            // —— 协商：观战者绝不代为应允（Ack 与真实对手叠加会双重悔棋/重开）——
            MsgKind::UndoReq => {
                if self.phase == Phase::Playing && self.role != Role::Spectator {
                    self.enqueue_confirm(ConfirmReq {
                        kind: ConfirmKind::Undo,
                        from: msg.user_id.clone(),
                        from_name: self.resolve_name(&msg.user_id, &msg.sender),
                        on_accept: ConfirmAction::SendAck("UndoAck", true),
                        on_reject: ConfirmAction::SendAck("UndoAck", false),
                    });
                    fx.push(Effect::Emit);
                }
            }
            MsgKind::UndoAck { ok } => {
                if self.role == Role::Spectator {
                    return fx;
                }
                if ok {
                    fx.extend(self.apply_undo_local());
                    fx.push(Effect::Notice(Some("对方已同意悔棋".into()), Some(2400)));
                } else {
                    fx.push(Effect::Notice(Some("对方拒绝了悔棋".into()), Some(2400)));
                }
            }
            MsgKind::ResetReq => {
                if self.phase == Phase::Playing && self.role != Role::Spectator {
                    self.enqueue_confirm(ConfirmReq {
                        kind: ConfirmKind::Reset,
                        from: msg.user_id.clone(),
                        from_name: self.resolve_name(&msg.user_id, &msg.sender),
                        on_accept: ConfirmAction::SendAck("ResetAck", true),
                        on_reject: ConfirmAction::SendAck("ResetAck", false),
                    });
                    fx.push(Effect::Emit);
                }
            }
            MsgKind::ResetAck { ok } => {
                if self.role == Role::Spectator {
                    return fx;
                }
                if ok {
                    fx.extend(self.apply_reset_local());
                    fx.push(Effect::Notice(Some("对方已同意重开".into()), Some(2400)));
                } else {
                    fx.push(Effect::Notice(Some("对方拒绝了重开".into()), Some(2400)));
                }
            }
            MsgKind::SwapReq => {
                if self.phase == Phase::Playing && self.role != Role::Spectator {
                    self.enqueue_confirm(ConfirmReq {
                        kind: ConfirmKind::Swap,
                        from: msg.user_id.clone(),
                        from_name: self.resolve_name(&msg.user_id, &msg.sender),
                        on_accept: ConfirmAction::SendAck("SwapAck", true),
                        on_reject: ConfirmAction::SendAck("SwapAck", false),
                    });
                    fx.push(Effect::Emit);
                }
            }
            MsgKind::SwapAck { ok } => {
                if self.role == Role::Spectator {
                    return fx;
                }
                if ok {
                    fx.extend(self.apply_swap_local());
                    fx.push(Effect::Notice(Some("对方已同意换棋（黑白互换，已重开）".into()), Some(3000)));
                } else {
                    fx.push(Effect::Notice(Some("对方拒绝了换棋".into()), Some(2400)));
                }
            }
            // —— 围棋终局计分同步 ——
            MsgKind::ScoreMark { dead } => {
                self.peer_dead = dead;
                self.maybe_finish_score();
                fx.push(Effect::Emit);
            }
            MsgKind::ScoreConfirmReq => {
                if self.role != Role::Spectator && self.scoring {
                    // Req 语义 = 「对方已确认」：先记对方的确认位，弹窗只决定自己的。
                    self.peer_score_ok = true;
                    self.enqueue_confirm(ConfirmReq {
                        kind: ConfirmKind::ScoreConfirm,
                        from: msg.user_id.clone(),
                        from_name: self.resolve_name(&msg.user_id, &msg.sender),
                        on_accept: ConfirmAction::ScoreConfirm { ok: true },
                        on_reject: ConfirmAction::ScoreConfirm { ok: false },
                    });
                    self.maybe_finish_score();
                    fx.push(Effect::Emit);
                }
            }
            MsgKind::ScoreConfirmAck { ok } => {
                if ok {
                    self.peer_score_ok = true;
                    self.maybe_finish_score();
                    fx.push(Effect::Emit);
                } else {
                    // 对方拒绝：其确认位撤回，可继续调整死子后再次确认。
                    self.peer_score_ok = false;
                    self.my_score_ok = false;
                    fx.push(Effect::Notice(Some("对方拒绝了计分确认，可继续调整死子".into()), Some(3000)));
                }
            }
            MsgKind::Ping | MsgKind::Pong => {}
        }
        fx
    }

    /// 当前局面全量快照消息。
    pub(crate) fn sync_state_kind(&self) -> MsgKind {
        MsgKind::SyncState {
            sv: Some(self.sync_epoch),
            board: self.board.clone(),
            to_move: self.to_move.clone(),
            winner: self.winner.clone(),
            history: self.history.clone(),
            last_move: self.last_move,
            kind: self.kind.clone(),
            size: self.size,
        }
    }

    /// 应用远端全量快照（(sv, history.len) 双键守卫：合法回退靠 sv，旧快照靠双键）。
    fn apply_sync_state(&mut self, msg: &GameMsg) -> Vec<Effect> {
        let MsgKind::SyncState { sv, board, to_move, winner, history, last_move, kind, size } = &msg.kind else {
            return Vec::new();
        };
        let sv = sv.unwrap_or(0);
        if sv < self.sync_epoch || (sv == self.sync_epoch && history.len() < self.history.len()) {
            return Vec::new();
        }
        if *kind != self.kind || *size != self.size {
            self.engine = goptop_core::game::GameState::new(make_engine_kind(kind, *size));
            self.kind = kind.clone();
            self.size = *size;
        }
        self.board = board.clone();
        self.to_move = to_move.clone();
        self.winner = winner.clone();
        self.history = history.clone();
        self.last_move = *last_move;
        // wasm 引擎同步采纳快照：后续落子/悔棋的规则判定基于它。
        let board_json = serde_json::to_string(&board).unwrap_or_else(|_| "[]".into());
        let winner_str = winner.clone().unwrap_or_else(|| "null".into());
        let history_json = serde_json::to_string(history).unwrap_or_else(|_| "[]".into());
        let ok = self.adopt_snapshot(&board_json, to_move, &winner_str, &history_json);
        if !ok {
            // 远端脏快照（重放矛盾）：拒绝并提示——绝不带病采纳。
            return vec![Effect::Notice(Some("收到的对局快照异常，已忽略".into()), Some(3000)), Effect::Emit];
        }
        self.scoring = self.engine.scoring;
        vec![Effect::Emit]
    }

    /// adopt 引擎封装（engine.adopt 的 JSON 形态，见 goptop-core wasm 契约）。
    fn adopt_snapshot(&mut self, board_json: &str, to_move: &str, winner: &str, history_json: &str) -> bool {
        adopt_into_engine(&mut self.engine, board_json, to_move, winner, history_json)
    }

    /// 应用远端落子（by 为准；乱序/重放/越界防御）。
    /// 落子消息入口：轮次对上就立即应用，对不上（乱序先到）先暂存，等轮次到了补应用。
    ///
    /// 为什么必须暂存而不是丢弃：`on_net` 在分发**之前**就把 (sender, seq) 记进了去重表，
    /// 被行棋方守卫拒收的那一手因此再也等不到「另一条路径的重传」——副本会被当重复吃掉，
    /// 这一手**永久丢失**。观战者受害最明显：它同时收「房主自己的手（直连 + 中转）」和
    /// 「房主转发的对手手（中转）」，两路交织时乱序概率最高
    /// （2026-09-18 实机测试：快节奏对局下观战者永久少 2 手，20s 后仍不同步）。
    pub(crate) fn on_move_net(&mut self, seq: u64, mv: MoveT, by: Color) -> Vec<Effect> {
        // 认输不参与轮次：它不是落子/停手，任何时候都必须立即生效（否则会被暂存到天荒地老）。
        if matches!(mv, MoveT::Resign) {
            return self.apply_move(mv, by);
        }
        if by == self.to_move {
            let mut fx = self.apply_move(mv, by);
            fx.extend(self.drain_pending_moves());
            fx
        } else {
            // 上限保护：正常乱序只有一两手，攒多了说明对端行为异常，丢弃最早的。
            if self.pending_moves.len() >= 16 {
                self.pending_moves.remove(0);
            }
            self.pending_moves.push((seq, mv, by));
            Vec::new()
        }
    }

    /// 把暂存里所有「轮次已对上」的手补应用（每轮至少移出一个元素，必然终止）。
    fn drain_pending_moves(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        while let Some(pos) = self.pending_moves.iter().position(|(_, _, by)| *by == self.to_move) {
            let (_, mv, by) = self.pending_moves.remove(pos);
            fx.extend(self.apply_move(mv, by));
        }
        fx
    }

    fn apply_move(&mut self, mv: MoveT, by: Color) -> Vec<Effect> {
        self.peer_connected = true;
        if by != "black" && by != "white" {
            return Vec::new();
        }
        let mut fx = Vec::new();
        match mv {
            MoveT::Place { coord } => {
                if self.winner.is_some() || by != self.to_move {
                    return Vec::new();
                }
                let n = self.board.len() as u16;
                if coord.x >= n || coord.y >= n || self.board[coord.y as usize][coord.x as usize] != "empty" {
                    return Vec::new();
                }
                match self.engine.try_play(Move::Place(Coord::new(coord.x as u8, coord.y as u8))) {
                    Ok(_) => {
                        sync_mirror_from_engine(self);
                        fx.push(Effect::Emit);
                    }
                    Err(_) => {
                        // wasm 拒绝（守卫口径外或快照有偏差）：丢弃本手——双保险。
                    }
                }
            }
            MoveT::Pass => {
                if by != self.to_move || self.role == Role::Spectator {
                    return Vec::new();
                }
                if self.engine.try_play(Move::Pass).is_ok() {
                    sync_mirror_from_engine(self);
                    if self.scoring {
                        // 双 Pass 终局：进入计分阶段（提示双方标记死子）。
                        fx.push(Effect::Notice(Some("双方连续停一手，进入终局计分：点击棋子标记死子，确认后计分".into()), Some(5200)));
                    }
                    fx.push(Effect::Emit);
                }
            }
            MoveT::Resign => {
                // 认输者 = by，胜者是其对手（观战者/发送方回流都正确）。
                self.winner = Some(if by == "black" { "white".into() } else { "black".into() });
                fx.push(Effect::Emit);
            }
        }
        fx
    }

    /// 撤销最后一手（引擎全量重放）并补发快照对齐观战者。
    pub(crate) fn apply_undo_local(&mut self) -> Vec<Effect> {
        if self.history.is_empty() {
            return Vec::new();
        }
        if undo_engine(&mut self.engine).is_none() {
            return Vec::new();
        }
        self.sync_epoch += 1;
        sync_mirror_from_engine(self);
        // 悔棋后轮次已变：暂存的乱序手相对新局面全是过期手，必须丢弃。
        self.pending_moves.clear();
        let snapshot = self.next_msg(self.sync_state_kind());
        vec![Effect::Broadcast(snapshot), Effect::Emit]
    }

    /// 协商重开的本地执行。
    pub(crate) fn apply_reset_local(&mut self) -> Vec<Effect> {
        self.engine = goptop_core::game::GameState::new(make_engine_kind(&self.kind.clone(), self.size));
        self.sync_epoch += 1;
        sync_mirror_from_engine(self);
        self.scoring = false;
        self.reset_score_state();
        let snapshot = self.next_msg(self.sync_state_kind());
        vec![Effect::Broadcast(snapshot), Effect::Emit]
    }

    /// 协商换棋：黑白互换并重开（观战者不参与，无颜色可换）。
    pub(crate) fn apply_swap_local(&mut self) -> Vec<Effect> {
        if self.role == Role::Inviter || self.role == Role::Invitee {
            self.my_color = if self.my_color == "black" { "white".into() } else { "black".into() };
        }
        self.apply_reset_local()
    }

    /// 内部 userId 解析显示名：聊天记录 → 名册 → 「对方」。
    pub(crate) fn resolve_name(&self, user_id: &str, fallback: &str) -> String {
        if let Some(c) = self.chat_log.iter().rev().find(|c| c.user_id == user_id) {
            return c.name.clone();
        }
        if let Some(p) = self.peers.iter().find(|p| p.id == user_id) {
            return p.name.clone();
        }
        if fallback == user_id {
            "对方".into()
        } else {
            fallback.to_string()
        }
    }
}

/* ---------------- UI 命令 ---------------- */

/// 落子门槛（对齐 TS handlePlace 守卫序）。
fn can_place(s: &Session, x: u16, y: u16) -> bool {
    if s.winner.is_some() || s.role == Role::Spectator || s.phase != Phase::Playing {
        return false;
    }
    let n = s.board.len() as u16;
    if x >= n || y >= n || s.board[y as usize][x as usize] != "empty" {
        return false;
    }
    if !s.peer_connected && !s.relay_available() {
        return false;
    }
    s.to_move == s.my_color
}

pub(crate) fn place(s: &mut Session, ctx: &ReduceCtx, x: u16, y: u16) -> Vec<Effect> {
    let _ = ctx;
    if !can_place(s, x, y) {
        return Vec::new();
    }
    let mover = s.to_move.clone();
    match s.engine.try_play(Move::Place(Coord::new(x as u8, y as u8))) {
        Ok(_) => {
            sync_mirror_from_engine(s);
            let msg = s.next_msg(MsgKind::Move {
                move_: MoveT::Place { coord: CoordT { x, y } },
                by: mover,
            });
            vec![Effect::Broadcast(msg), Effect::Emit]
        }
        Err(_) => Vec::new(),
    }
}

pub(crate) fn pass(s: &mut Session, ctx: &ReduceCtx) -> Vec<Effect> {
    let _ = ctx;
    if s.winner.is_some() || s.role == Role::Spectator || s.phase != Phase::Playing || s.to_move != s.my_color {
        return Vec::new();
    }
    let mover = s.to_move.clone();
    if s.engine.try_play(Move::Pass).is_err() {
        return Vec::new();
    }
    sync_mirror_from_engine(s);
    let mut fx = vec![Effect::Broadcast(s.next_msg(MsgKind::Move { move_: MoveT::Pass, by: mover }))];
    if s.scoring {
        fx.push(Effect::Notice(Some("双方连续停一手，进入终局计分：点击棋子标记死子，确认后计分".into()), Some(5200)));
    }
    fx.push(Effect::Emit);
    fx
}

pub(crate) fn resign(s: &mut Session, ctx: &ReduceCtx) -> Vec<Effect> {
    let _ = ctx;
    if s.winner.is_some() || s.role == Role::Spectator || s.phase != Phase::Playing {
        return Vec::new();
    }
    let me = s.my_color.clone();
    if s.engine.try_play(Move::Resign).is_err() {
        return Vec::new();
    }
    sync_mirror_from_engine(s);
    // 引擎的 Resign 语义是「当前行棋方认输」（它不知道是谁点的）——认输者未必正在行棋，
    // 必须按自己的执色重定胜负，否则会出现「我认输、却判我赢」（2026-09-18 实机测试发现）。
    s.winner = Some(if me == "black" { "white".into() } else { "black".into() });
    vec![Effect::Broadcast(s.next_msg(MsgKind::Move { move_: MoveT::Resign, by: me })), Effect::Emit]
}

/* ---------------- 协商发起（对方同意制） ---------------- */

pub(crate) fn request_undo(s: &mut Session) -> Vec<Effect> {
    if s.phase != Phase::Playing || s.history.is_empty() {
        return Vec::new();
    }
    vec![
        Effect::Broadcast(s.next_msg(MsgKind::UndoReq)),
        Effect::Notice(Some("已请求悔棋，等待对方同意…".into()), Some(6000)),
    ]
}

pub(crate) fn request_reset(s: &mut Session) -> Vec<Effect> {
    if s.phase != Phase::Playing {
        return Vec::new();
    }
    vec![
        Effect::Broadcast(s.next_msg(MsgKind::ResetReq)),
        Effect::Notice(Some("已请求重开，等待对方同意…".into()), Some(6000)),
    ]
}

pub(crate) fn request_swap(s: &mut Session) -> Vec<Effect> {
    if s.phase != Phase::Playing {
        return Vec::new();
    }
    vec![
        Effect::Broadcast(s.next_msg(MsgKind::SwapReq)),
        Effect::Notice(Some("已请求换棋（交换黑白并重开），等待对方同意…".into()), Some(6000)),
    ]
}

/// 弹窗决定（队列消费）：同意/拒绝按数据化动作解释执行。
pub(crate) fn confirm_resolve(s: &mut Session, ctx: &ReduceCtx, approve: bool) -> Vec<Effect> {
    let Some(req) = s.confirm_queue.pop_front() else {
        return Vec::new();
    };
    let action = if approve { req.on_accept } else { req.on_reject };
    let mut fx = match action {
        ConfirmAction::SendAck(kind, ok) => {
            let msg_kind = match (kind, ok) {
                ("UndoAck", ok) => MsgKind::UndoAck { ok },
                ("ResetAck", ok) => MsgKind::ResetAck { ok },
                _ => MsgKind::SwapAck { ok },
            };
            let mut fx = vec![Effect::Broadcast(s.next_msg(msg_kind))];
            // 同意后本地执行确定性操作（与对方互为镜像）。
            if approve {
                match kind {
                    "UndoAck" => fx.extend(s.apply_undo_local()),
                    "ResetAck" => fx.extend(s.apply_reset_local()),
                    _ => fx.extend(s.apply_swap_local()),
                }
            }
            fx
        }
        ConfirmAction::WrongPwdProceed => s.server_proceed_join(&req.from, true),
        ConfirmAction::WrongPwdReject { reason } => {
            vec![s.signal_effect(&req.from, "reject", serde_json::json!({ "name": s.display_name(), "reason": reason }))]
        }
        ConfirmAction::SpecChatAck { applicant, ok, relay } => {
            let mut fx = vec![s.signal_effect(&applicant, "spec-chat-ack", serde_json::json!({ "ok": ok, "name": s.display_name() }))];
            if let Some(r) = relay {
                if approve {
                    fx.push(s.signal_effect(&r.to, "spec-chat-req", serde_json::json!({
                        "name": r.from_name, "relay": true, "applicant": r.applicant,
                    })));
                }
            }
            fx
        }
        ConfirmAction::ScoreConfirm { ok } => {
            if ok {
                s.my_score_ok = true;
                s.maybe_finish_score();
                vec![
                    Effect::Broadcast(s.next_msg(MsgKind::ScoreConfirmAck { ok: true })),
                ]
            } else {
                vec![Effect::Broadcast(s.next_msg(MsgKind::ScoreConfirmAck { ok: false }))]
            }
        }
    };
    fx.push(Effect::Emit);
    let _ = ctx;
    fx
}

/* ---------------- 聊天 ---------------- */

pub(crate) fn send_chat(s: &mut Session, ctx: &ReduceCtx, text: &str) -> Vec<Effect> {
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    let (uid, name) = (s.user_id.clone(), s.display_name());
    s.push_chat(&uid, &name, t, true, ctx.now_ms);
    let mut fx = vec![Effect::Emit];
    if s.role == Role::Spectator {
        // 观战者只与自己 host 通信（服务器模式）；无服务器观战者暂无聊天信道。
        if s.server_mode {
            if let Some(host) = s.my_host.clone() {
                if s.server_state == "ready" {
                    fx.push(s.signal_effect(&host, "spec-chat", serde_json::json!({
                        "userId": s.user_id, "name": s.display_name(), "text": t,
                    })));
                }
            }
        }
    } else if s.server_mode {
        if let Some(opp) = s.opponent.clone() {
            if s.server_state == "ready" {
                fx.push(s.signal_effect(&opp, "chat", serde_json::json!({
                    "userId": s.user_id, "name": s.display_name(), "text": t,
                })));
            }
        }
    } else {
        fx.push(Effect::Broadcast(s.next_msg(MsgKind::Chat { text: t.to_string() })));
    }
    fx
}

/* ---------------- 围棋终局计分 ---------------- */

/// 翻转死子标记（本方集合；同步给对端）。
pub(crate) fn toggle_dead(s: &mut Session, x: u16, y: u16) -> Vec<Effect> {
    if !s.scoring || s.role == Role::Spectator || s.winner.is_some() {
        return Vec::new();
    }
    let c = CoordT { x, y };
    if let Some(pos) = s.my_dead.iter().position(|d| *d == c) {
        s.my_dead.remove(pos);
    } else if s.board.get(y as usize).and_then(|r| r.get(x as usize)).map(String::as_str) != Some("empty") {
        s.my_dead.push(c);
    }
    // 确认过的一方再改动标记：重置双方确认（避免以旧确认计新盘）。
    s.my_score_ok = false;
    s.peer_score_ok = false;
    s.score_result = None;
    let fx = vec![
        Effect::Broadcast(s.next_msg(MsgKind::ScoreMark { dead: s.my_dead.clone() })),
        Effect::Emit,
    ];
    fx
}

/// 确认计分（双确认制）。
pub(crate) fn confirm_score(s: &mut Session) -> Vec<Effect> {
    if !s.scoring || s.role == Role::Spectator {
        return Vec::new();
    }
    s.my_score_ok = true;
    let mut fx = vec![Effect::Broadcast(s.next_msg(MsgKind::ScoreConfirmReq))];
    if !s.maybe_finish_score_inner() {
        fx.push(Effect::Notice(Some("已确认，等待对方确认…".into()), Some(4000)));
    }
    fx.push(Effect::Emit);
    fx
}

impl Session {
    /// 双方都确认（或对方确认到达补齐）后计分。
    pub(crate) fn maybe_finish_score(&mut self) {
        self.maybe_finish_score_inner();
    }

    fn maybe_finish_score_inner(&mut self) -> bool {
        if !(self.my_score_ok && self.peer_score_ok) {
            return false;
        }
        // 死子交集：双方都标记的才算死。
        let dead: Vec<CoordT> = self.my_dead.iter().filter(|d| self.peer_dead.contains(d)).copied().collect();
        let dead_json = serde_json::to_string(&dead).unwrap_or_else(|_| "[]".into());
        let _ = dead_json;
        let n = self.engine.kind.size();
        let mut dead_coords: Vec<Coord> = dead.iter().map(|c| Coord::new(c.x as u8, c.y as u8)).collect();
        dead_coords.sort_by_key(|c| (c.x, c.y));
        dead_coords.dedup();
        let result = match &self.engine.board {
            goptop_core::board::BoardVariant::B15(b) => go::score_area(b, &dead_coords, go::KOMI, n),
            goptop_core::board::BoardVariant::B19(b) => go::score_area(b, &dead_coords, go::KOMI, n),
        };
        if let Ok(score) = result {
            self.score_result = Some(snapshot::ScoreResult {
                black: score.black,
                white: score.white,
                winner: stone_to_color(score.winner),
                dead_removed: score.dead_removed,
            });
            self.winner = Some(stone_to_color(score.winner));
            return true;
        }
        false
    }
}

/* ---------------- 定时器 ---------------- */

pub(crate) fn on_timer(s: &mut Session, id: TimerId) -> Vec<Effect> {
    match id {
        // hello + sync request：对局建立的握手序列。
        "hello-delay" => {
            let mut fx = vec![Effect::Broadcast(s.next_msg(MsgKind::Hello { kind: s.kind.clone(), size: s.size }))];
            fx.push(Effect::Broadcast(s.next_msg(MsgKind::SyncRequest)));
            fx
        }
        "sync-request" => vec![Effect::Broadcast(s.next_msg(MsgKind::SyncRequest))],
        // join 无应答兜底。
        "join-timeout" => {
            if s.role == Role::Invitee && s.phase == Phase::Waiting && s.game_id.is_none() {
                vec![Effect::Notice(Some("对方不在线或未响应，请确认链接是否最新".into()), Some(6000))]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/* ---------------- 引擎 JSON 封装（与 wasm 契约同形的 native 直调） ---------------- */

/// adopt：按历史重放重建引擎状态（与 goptop-core wasm adopt 行为一致——
/// native 侧直接调用 core 的等价逻辑，不经过 wasm-bindgen）。
fn adopt_into_engine(engine: &mut GameState, board_json: &str, to_move: &str, winner: &str, history_json: &str) -> bool {
    // 与 wasm.rs::adopt 相同的契约：解析 → 校验 → 全量重放 → 一致性检查 → 采纳。
    let Ok(rows) = serde_json::from_str::<Vec<Vec<String>>>(board_json) else {
        return false;
    };
    let n = engine.kind.size();
    if rows.len() != n || rows.iter().any(|r| r.len() != n) {
        return false;
    }
    fn stone_from(s: &str) -> Option<Stone> {
        match s {
            "black" => Some(Stone::Black),
            "white" => Some(Stone::White),
            "empty" => Some(Stone::Empty),
            _ => None,
        }
    }
    let mut variant = goptop_core::board::BoardVariant::new(n);
    for (y, row) in rows.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            let Some(st) = stone_from(cell) else { return false };
            variant.set(Coord::new(x as u8, y as u8), st);
        }
    }
    let Some(tm) = stone_from(to_move).filter(|s| !s.is_empty()) else {
        return false;
    };
    let winner = match winner {
        "null" | "" => None,
        "black" => Some(Stone::Black),
        "white" => Some(Stone::White),
        _ => return false,
    };
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Place(Coord),
        Pass(String),
    }
    let Ok(entries) = serde_json::from_str::<Vec<Entry>>(history_json) else {
        return false;
    };
    let mut moves = Vec::with_capacity(entries.len());
    for e in entries {
        match e {
            Entry::Place(c) => {
                if c.x as usize >= n || c.y as usize >= n {
                    return false;
                }
                moves.push(Move::Place(c));
            }
            Entry::Pass(s) => {
                if s != "pass" {
                    return false;
                }
                moves.push(Move::Pass);
            }
        }
    }
    let kind = engine.kind.clone();
    let mut fresh = GameState::new(kind);
    for mv in &moves {
        if fresh.try_play(mv.clone()).is_err() {
            return false;
        }
    }
    if fresh.board != variant || fresh.to_move != tm || (fresh.winner != winner && fresh.winner.is_some()) {
        return false;
    }
    fresh.board = variant;
    fresh.to_move = tm;
    fresh.winner = winner;
    fresh.history = moves;
    *engine = fresh;
    true
}

/// undo：弹出最后一手并全量重放（与 wasm undo_last 一致：重放失败保持原状态）。
fn undo_engine(engine: &mut GameState) -> Option<()> {
    let mut moves = engine.history.clone();
    moves.pop()?;
    let kind = engine.kind.clone();
    let mut fresh = GameState::new(kind);
    for mv in moves {
        fresh.try_play(mv).ok()?;
    }
    *engine = fresh;
    Some(())
}
