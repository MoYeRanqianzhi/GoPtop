//! UI 快照 — 状态机 → 渲染层的单一 JSON 契约。
//!
//! TS 侧只剩「渲染快照 + 发命令」：每次 [`Effect::Emit`] 后 transport 把
//! [`Session::snapshot`] 的字符串交给 UI（React 以 useSyncExternalStore 消费）。
//! 字段名 camelCase 对齐历史 TS 侧 hook 返回值，页面组件改动最小化。

use super::*;

/// 计分结果 DTO。
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreResult {
    pub black: f32,
    pub white: f32,
    pub winner: Color,
    pub dead_removed: u32,
}

impl Session {
    /// 全量状态快照（JSON 字符串）。规模：board ≤ 19×19 + chatLog ≤ 200 条，
    /// 每次对局事件后全量推送的成本可忽略。手工构建 Map（巨型 json! 宏会触发
    /// 递归限制，且字段分组注释在这里更有用）。
    #[must_use]
    pub fn snapshot(&self) -> String {
        use serde_json::{json, Map, Value};
        let mut m = Map::new();
        // 身份与配置。
        m.insert("userId".into(), json!(self.user_id));
        m.insert("peerId".into(), json!(self.peer_id));
        m.insert("name".into(), json!(self.name));
        m.insert("avatar".into(), json!(self.avatar));
        m.insert("serverMode".into(), json!(self.server_mode));
        m.insert("serverState".into(), json!(self.server_state));
        m.insert("shareOrigin".into(), json!(self.share_origin));
        // 规则面。
        m.insert("kind".into(), json!(self.kind));
        m.insert("size".into(), json!(self.size));
        m.insert("board".into(), json!(self.board));
        m.insert("toMove".into(), json!(self.to_move));
        m.insert("winner".into(), json!(self.winner));
        m.insert("history".into(), serde_json::to_value(&self.history).unwrap_or(Value::Array(vec![])));
        m.insert("lastMove".into(), json!(self.last_move));
        m.insert("moveCount".into(), json!(self.history.len()));
        m.insert("scoring".into(), json!(self.scoring));
        // 会话面。
        m.insert("phase".into(), json!(match self.phase { Phase::Home => "home", Phase::Waiting => "waiting", Phase::Playing => "playing" }));
        m.insert("role".into(), json!(match self.role { Role::Idle => "idle", Role::Inviter => "inviter", Role::Invitee => "invitee", Role::Spectator => "spectator" }));
        m.insert("gameId".into(), json!(self.game_id));
        m.insert("myColor".into(), json!(self.my_color));
        m.insert("peerConnected".into(), json!(self.peer_connected));
        m.insert("connLost".into(), json!(self.conn_lost));
        m.insert("pwd".into(), json!(self.pwd));
        m.insert("specPwd".into(), json!(self.spec_pwd));
        m.insert("spectateEnabled".into(), json!(self.spectate_enabled));
        m.insert("inviteUrl".into(), json!(self.invite_url));
        m.insert("watchUrl".into(), json!(self.watch_url));
        m.insert("answerBackUrl".into(), json!(self.answer_back_url));
        m.insert("specUrl".into(), json!(self.spec_url));
        m.insert("serverIncoming".into(), json!(self.incoming.as_ref().map(|i| json!({
            "from": i.from, "fromName": i.from_name, "kind": i.kind, "size": i.size,
        }))));
        // 连接面。
        m.insert("peers".into(), serde_json::to_value(&self.peers).unwrap_or(Value::Array(vec![])));
        m.insert("relayAvailable".into(), json!(self.relay_available()));
        // 观战房间。
        m.insert("spectators".into(), serde_json::to_value(&self.spectators).unwrap_or(Value::Array(vec![])));
        m.insert("specRequests".into(), serde_json::to_value(&self.spec_requests).unwrap_or(Value::Array(vec![])));
        m.insert("specCanChat".into(), json!(self.spec_can_chat));
        m.insert("specDenied".into(), json!(self.spec_denied));
        // 协商弹窗（队列首 + 队列深度）。
        m.insert("confirmReq".into(), json!(self.confirm_queue.front().map(|r| json!({
            "kind": match r.kind {
                ConfirmKind::Undo => "undo",
                ConfirmKind::Reset => "reset",
                ConfirmKind::Swap => "swap",
                ConfirmKind::SpecChat => "spec-chat",
                ConfirmKind::WrongPwd => "wrong-pwd",
                ConfirmKind::ScoreConfirm => "score-confirm",
            },
            "from": r.from,
            "fromName": r.from_name,
            "queued": self.confirm_queue.len(),
        }))));
        // 聊天/头像。
        m.insert("chatLog".into(), serde_json::to_value(&self.chat_log).unwrap_or(Value::Array(vec![])));
        m.insert("peerAvatars".into(), json!(self.peer_avatars));
        // 计分。
        m.insert("myDead".into(), json!(self.my_dead));
        m.insert("peerDead".into(), json!(self.peer_dead));
        m.insert("myScoreOk".into(), json!(self.my_score_ok));
        m.insert("peerScoreOk".into(), json!(self.peer_score_ok));
        m.insert("scoreResult".into(), json!(self.score_result.as_ref().map(|s| json!({
            "black": s.black, "white": s.white, "winner": s.winner, "deadRemoved": s.dead_removed,
        }))));
        Value::Object(m).to_string()
    }

    /// 对局页连接状态文案（p2pStatusText 对齐 TS 派生）。
    #[must_use]
    pub fn p2p_status_text(&self) -> String {
        let color = if self.my_color == "black" { "黑" } else { "白" };
        match self.phase {
            Phase::Home => {
                if self.role == Role::Spectator { "观战中".into() } else { "主页 · 选择对手或等待被挑战".into() }
            }
            Phase::Waiting => format!("等待对手 · 执{color}"),
            Phase::Playing => {
                if !self.peer_connected && self.relay_available() {
                    return format!("经服务器中转 · 执{color}");
                }
                if !self.peer_connected {
                    return format!("连接中 · 执{color}");
                }
                let turn = if self.to_move == self.my_color { " · 轮到你" } else { " · 等待对手" };
                let spec = if self.role == Role::Spectator { "（观战）" } else { "" };
                format!("已直连 · 执{color}{spec}{turn}")
            }
        }
    }

    /// 对局卡连接指示灯：绿=可对弈（直连或服务器中转），红=中断，橙=未就绪。
    #[must_use]
    pub fn link_lamp(&self) -> (String, String) {
        if self.conn_lost {
            return ("#b00020".into(), "已中断".into());
        }
        if self.phase == Phase::Playing && (self.peer_connected || self.relay_available()) {
            return ("#0a7a2e".into(), "已连接".into());
        }
        ("#FF8C1A".into(), "等待对手".into())
    }

    /// 棋盘是否禁用（boardDisabled 对齐 TS 守卫序）。
    #[must_use]
    pub fn board_disabled(&self) -> bool {
        if self.winner.is_some() || self.role == Role::Spectator {
            return true;
        }
        if self.phase != Phase::Playing {
            return self.phase == Phase::Waiting;
        }
        if !self.peer_connected && !self.relay_available() {
            return true;
        }
        self.to_move != self.my_color
    }
}
