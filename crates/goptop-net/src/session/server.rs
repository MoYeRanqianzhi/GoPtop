//! 服务器信令域 — userId 链接 + pwd 客户端校验 + 观战房间管理 + 大厅挑战。
//!
//! 服务器是纯转发管道（hello/announce/peers/signal/relay/ping）：pwd 校验、
//! 邀请/观战同意逻辑全在本端状态机（与「服务器无储存」承诺一致）。
//! payload 为 serde_json::Value（服务器原样转发，本层按需取字段）。

use super::lobby;
use super::*;

impl Session {
    /* ---------- 名册与状态上报 ---------- */

    /// 阶段变化后的名册状态上报（与 presence 并行；重连重放在 transport）。
    pub(crate) fn announce(&self) -> Effect {
        let (status, gid) = match (self.phase, self.role) {
            (Phase::Home, _) => ("idle", None),
            (Phase::Waiting, _) => ("waiting", self.game_id.clone()),
            (Phase::Playing, Role::Spectator) => ("idle", self.game_id.clone()),
            (Phase::Playing, _) => ("in-game", self.game_id.clone()),
        };
        Effect::SendServer(serde_json::json!({ "t": "announce", "status": status, "gameId": gid }))
    }

    /// 服务器事件入口（reduce 顶层分发落点）。
    pub(crate) fn on_server(&mut self, ev: ServerEvt, ctx: &ReduceCtx) -> Vec<Effect> {
        let mut fx = Vec::new();
        match ev {
            ServerEvt::State { s, detail } => {
                self.server_state = s.clone();
                match s.as_str() {
                    "ready" => {
                        fx.push(Effect::Notice(None, None));
                        fx.extend(self.flush_pending_link());
                    }
                    "error" => {
                        fx.push(Effect::Notice(Some(format!("服务器连接失败：{}", detail.unwrap_or_else(|| "请检查设置页服务器配置".into()))), Some(3000)));
                    }
                    _ => {}
                }
                fx.push(Effect::Emit);
            }
            ServerEvt::Peers { users } => {
                self.peers = users.into_iter().filter(|u| u.id != self.user_id).collect();
                fx.push(Effect::Emit);
            }
            ServerEvt::Signal { from, kind, payload } => {
                fx.extend(self.on_server_signal(&from, &kind, &payload, ctx));
            }
            ServerEvt::Relayed { from: _, msg } => {
                fx.extend(self.on_net(msg, ctx));
            }
            ServerEvt::Error { msg, code } => {
                if code.as_deref() == Some("taken-over") {
                    // 本 ID 被别处顶替：transport 主动断开（manualClose），本层只报错。
                    self.server_state = "error".into();
                    fx.push(Effect::Notice(Some(msg), None));
                    fx.push(Effect::Emit);
                } else {
                    fx.push(Effect::Notice(Some(format!("服务器：{msg}")), Some(3000)));
                }
            }
        }
        fx
    }

    /// userId 链接意图就绪执行（连接 ready 后发 join/spec-join）。
    /// 不查名册判在线：ready 与首份名册广播的先后不确定，误判会把拿有效链接的人
    /// 弹回主页——直接发信号，无人受理由超时兜底。
    pub(crate) fn flush_pending_link(&mut self) -> Vec<Effect> {
        let Some(link) = self.pending_link.take() else { return Vec::new() };
        if self.server_state != "ready" {
            self.pending_link = Some(link);
            return Vec::new();
        }
        let name = self.display_name();
        if link.spec {
            self.role = Role::Spectator;
            self.my_host = Some(link.target.clone());
            vec![
                Effect::Nav("/p2p".into()),
                Effect::SendServer(serde_json::json!({ "t": "signal", "to": link.target, "kind": "spec-join", "payload": { "pwd": link.pwd.unwrap_or_default(), "name": name } })),
                Effect::Notice(Some("正在连接对局观战…".into()), None),
                Effect::Emit,
            ]
        } else {
            self.role = Role::Invitee;
            self.my_color = "white".into();
            self.phase = Phase::Waiting;
            vec![
                Effect::Nav("/p2p".into()),
                Effect::SendServer(serde_json::json!({ "t": "signal", "to": link.target, "kind": "join", "payload": { "pwd": link.pwd.unwrap_or_default(), "name": name } })),
                Effect::Notice(Some("正在请求加入对局…".into()), None),
                Effect::Timer { id: "join-timeout", ms: 15_000 },
                Effect::Emit,
            ]
        }
    }

    /// 服务器转发来的信令（kind 语义见各分支）。
    pub(crate) fn on_server_signal(&mut self, from: &str, kind: &str, pl: &serde_json::Value, ctx: &ReduceCtx) -> Vec<Effect> {
        match kind {
            // —— 受邀者请求加入：pwd 校验在本端；有对手/对局中一律拒绝（join 互斥）——
            "join" => self.server_on_join(from, pl),
            // —— answer 回达：路由到 tag 匹配的连接 ——
            "answer" => self.server_on_answer(from, pl),
            // —— 邀请者发来的 offer（受邀者视角）——
            "offer" => {
                let kind_s = pl["kind"].as_str().unwrap_or("gomoku").to_string();
                let size = sanitize_size(pl["size"].as_u64());
                let name = jstr(pl, "name", from);
                let game_id = jstr(pl, "gameId", "");
                let offer = jstr(pl, "offer", "");
                self.server_accept_offer(from, &name, &kind_s, size, &game_id, &offer)
            }
            "reject" => {
                let reason = jstr(pl, "reason", "未说明");
                let mut fx = vec![Effect::Notice(Some(format!("对方拒绝：{reason}")), Some(3600))];
                if self.phase == Phase::Waiting {
                    fx.extend(self.do_back_home());
                }
                fx.push(Effect::Emit);
                fx
            }
            // —— 大厅挑战 ——
            "challenge" => self.server_on_challenge(from, pl),
            // —— 挑战被接受：发起者（执黑）建局并送 offer ——
            "challenge-accepted" => {
                // 发起者发 challenge 时在主页；同意到达时仍是主页（或「先点开启对战
                // 又被接受」的 waiting 边缘序——保留该分支优先成局）。
                if self.phase != Phase::Home && !(self.phase == Phase::Waiting && self.role == Role::Inviter) {
                    return Vec::new();
                }
                let mut fx = self.server_admit_challenger(from, ctx);
                // 挑战者是从「在线用户」页发起挑战的，受理到达时他人还在名册页——
                // 必须把他带到对局页，否则只有一句「前往 P2P 页」的手动链接，
                // 而受理方是自动进入的（2026-09-18 实机测试发现的不一致）。
                fx.push(Effect::Nav("/p2p".into()));
                fx
            }
            // —— 观战请求：pwd 对自动同意，错/无转聊天区私有申请 ——
            "spec-join" => self.server_on_spec_join(from, pl, ctx),
            "spec-offer" => self.server_accept_spectator_offer(from, &jstr(pl, "offer", ""), &jstr(pl, "gameId", "")),
            // —— 观战申请批复 ——
            "spec-reply" => {
                if pl["ok"] == serde_json::json!(true) {
                    vec![Effect::Notice(Some("观战申请已通过，连接中…".into()), None), Effect::Emit]
                } else {
                    self.spec_denied = true;
                    let reason = jstr(pl, "reason", "未说明");
                    let mut fx = vec![Effect::Notice(Some(format!("观战申请被拒绝：{reason}（本局无法再次申请）")), Some(4200)), Effect::Emit];
                    if self.phase == Phase::Home && self.role == Role::Idle {
                        fx.extend(self.do_back_home());
                    }
                    fx
                }
            }
            "spec-pending" => vec![Effect::Notice(Some("观战申请已送达，等待对方处理…".into()), None), Effect::Emit],
            // —— 观战房间名单同步 ——
            "spec-sync" => self.server_on_spec_sync(pl),
            // —— 踢出对方直连的观战者（先通知被踢者，再移除）——
            "spec-kick" => {
                let id = jstr(pl, "id", "");
                let mut fx = Vec::new();
                if self.server_state == "ready" {
                    fx.push(Effect::SendServer(serde_json::json!({ "t": "signal", "to": id, "kind": "spec-kicked", "payload": { "name": self.display_name() } })));
                }
                if self.rtc_peers.iter().any(|q| q.tag == id) {
                    self.rtc_peers.retain(|q| q.tag != id);
                    self.spectators.retain(|s| s.id != id);
                    fx.extend(self.push_spec_sync());
                }
                fx.push(Effect::Emit);
                fx
            }
            // —— 被踢：断开回主页（通知在 backHome 之后，不被其清空动作覆盖）——
            "spec-kicked" => {
                self.my_host = None;
                self.spec_can_chat = false;
                let mut fx = vec![Effect::ClosePeers];
                fx.extend(self.do_back_home());
                fx.push(Effect::Notice(Some("你已被移出观战".into()), Some(3600)));
                fx
            }
            // —— 观战者聊天（经 host 转发进双方聊天区）——
            "spec-chat" => {
                let uid = jstr(pl, "userId", from);
                let name = jstr(pl, "name", from);
                let text = jstr(pl, "text", "");
                self.push_chat(&uid, &name, &text, false, ctx.now_ms);
                let mut fx = vec![Effect::Emit];
                // host 转发给另一位对局者（opponent，不再 find(id!==from)）；观战者之间互不可见。
                if self.role == Role::Inviter || self.role == Role::Invitee {
                    if let Some(opp) = self.opponent.clone() {
                        if opp != from && self.server_state == "ready" {
                            fx.push(Effect::SendServer(serde_json::json!({ "t": "signal", "to": opp, "kind": "spec-chat", "payload": pl.clone() })));
                        }
                    }
                }
                fx
            }
            // —— 观战者申请发言：弹窗批准，双方都同意才放开 ——
            "spec-chat-req" => self.server_on_spec_chat_req(from, pl),
            // —— 发言批准回执：集满双 ack 解锁 ——
            "spec-chat-ack" => {
                let name = jstr(pl, "name", "");
                if pl["ok"] == serde_json::json!(true) {
                    self.spec_chat_acks.insert(from.to_string());
                    if self.spec_chat_acks.len() >= 2 {
                        self.spec_denied = false;
                        self.spec_can_chat = true;
                    }
                    let got = self.spec_can_chat;
                    let mut fx = Vec::new();
                    if got {
                        fx.push(Effect::Notice(Some("双方已同意，你可以参与聊天了".into()), Some(3200)));
                    }
                    let _ = name;
                    fx.push(Effect::Emit);
                    fx
                } else {
                    self.spec_can_chat = false;
                    self.spec_denied = true;
                    vec![Effect::Notice(Some("发言申请被拒绝（本局无法再次申请）".into()), Some(3600)), Effect::Emit]
                }
            }
            // —— 观战者头像（经 host / 服务器转发）——
            "spec-avatar" => {
                let data = jstr(pl, "dataUrl", "");
                let uid = jstr(pl, "userId", from);
                if data.starts_with("data:image/") && data.len() < 20_000 {
                    self.peer_avatars.insert(uid, data);
                }
                vec![Effect::Emit]
            }
            // —— 对局者聊天（服务器模式不经 GameChannel，直接 signal）——
            "chat" => {
                let uid = jstr(pl, "userId", from);
                let name = jstr(pl, "name", from);
                let text = jstr(pl, "text", "");
                self.push_chat(&uid, &name, &text, false, ctx.now_ms);
                vec![Effect::Emit]
            }
            _ => Vec::new(),
        }
    }

    /* ---------- join / offer / answer（对局者侧） ---------- */

    fn server_on_join(&mut self, from: &str, pl: &serde_json::Value) -> Vec<Effect> {
        // join 互斥（审查项修复）：对局中、非等待者、已有对手时拒绝——
        // 旧实现无互斥，两个受邀者同时 join 第二个会永久卡死。
        if self.phase != Phase::Waiting || self.role != Role::Inviter || self.inviter_main.is_none() || self.opponent.is_some() {
            let reason = if self.phase == Phase::Playing { "正在对局中" } else { "对方不在等待对局" };
            return vec![self.signal_effect(from, "reject", serde_json::json!({ "name": self.display_name(), "reason": reason }))];
        }
        let ok = pl["pwd"].as_str().is_some_and(|p| Some(p) == self.pwd.as_deref());
        if ok {
            return self.server_proceed_join(from, true);
        }
        // pwd 错误 → 弹窗询问（队列化；不再覆盖未决请求）。
        let from_name = jstr(pl, "name", from);
        self.enqueue_confirm(ConfirmReq {
            kind: ConfirmKind::WrongPwd,
            from: from.to_string(),
            from_name,
            on_accept: ConfirmAction::WrongPwdProceed,
            on_reject: ConfirmAction::WrongPwdReject { reason: "邀请钥匙不正确".into() },
        });
        vec![Effect::Emit]
    }

    /// pwd 校验通过（或弹窗同意）后：accept + 发 offer（已就绪则立即，未就绪挂起等待）。
    pub(crate) fn server_proceed_join(&mut self, from: &str, send_accept: bool) -> Vec<Effect> {
        let Some(tag) = self.inviter_main.clone() else { return Vec::new() };
        let Some(slot) = self.rtc_peers.iter_mut().find(|q| q.tag == tag) else {
            return Vec::new();
        };
        let offer_ready = slot.offer_ready;
        let offer = slot.offer_plain.clone();
        if offer_ready {
            slot.tag = from.to_string();
            slot.awaiting_peer = None;
        } else {
            // offer 尚未 gathering 完：挂起，on_rtc_ready 就绪即发。
            slot.awaiting_peer = Some(from.to_string());
        }
        let mut fx = Vec::new();
        if send_accept {
            fx.push(self.signal_effect(from, "accept", serde_json::json!({})));
        }
        if offer_ready {
            // offer 已就绪：直接发并把主连改名为对端（answer 路由按对端 ID）。
            fx.push(self.signal_effect(from, "offer", serde_json::json!({
                "name": self.display_name(), "kind": self.kind, "size": self.size,
                "gameId": self.game_id.clone().unwrap_or_default(),
                "offer": offer.unwrap_or_default(),
            })));
            fx.push(Effect::RenamePeer { from: "main".into(), to: from.to_string() });
            self.opponent = Some(from.to_string());
            self.relay_targets.insert(from.to_string());
            self.inviter_main = Some(from.to_string());
        }
        fx.push(Effect::Emit);
        fx
    }

    fn server_on_answer(&mut self, from: &str, pl: &serde_json::Value) -> Vec<Effect> {
        // 服务器模式下 PeerSlot.tag 即对端 ID，按 tag 路由。
        if !self.rtc_peers.iter().any(|q| q.tag == from) {
            return Vec::new();
        }
        let answer = jstr(pl, "answer", "");
        let was_waiting = self.phase == Phase::Waiting && self.role == Role::Inviter;
        let mut fx = vec![Effect::AcceptAnswer { tag: from.to_string(), answer, encrypted: false }];
        if was_waiting {
            // 主连接受理完成：进对局、pwd 失效（两人满员）。
            self.phase = Phase::Playing;
            self.pwd = None;
            self.invite_url = None;
            self.peer_connected = false;
            fx.push(Effect::Notice(Some("对方已加入，对局开始".into()), None));
            fx.push(Effect::Timer { id: "hello-delay", ms: 80 });
            fx.push(Effect::Emit);
        }
        fx
    }

    /// 受邀者：收到 offer（join/challenge 流共用）——受理进等待并回 answer。
    pub(crate) fn server_accept_offer(&mut self, from: &str, from_name: &str, kind: &str, size: SizeT, game_id: &str, offer: &str) -> Vec<Effect> {
        if self.phase == Phase::Playing {
            return Vec::new();
        }
        self.close_all_rtc();
        let mut fx = self.reset_board_for(kind, size);
        self.game_id = Some(game_id.to_string());
        self.role = Role::Invitee;
        self.my_color = "white".into();
        self.phase = Phase::Waiting;
        self.pwd = None;
        self.invite_url = None;
        self.watch_url = None;
        self.answer_back_url = None;
        self.opponent = Some(from.to_string());
        self.relay_targets = BTreeSet::from([from.to_string()]);
        fx.extend([
            Effect::JoinChannel(game_id.to_string()),
            Effect::Notice(Some(format!("接受 {from_name} 的邀请，正在建立直连…")), None),
            Effect::Nav("/p2p".into()),
            Effect::CreatePeer { tag: from.to_string(), inviter: false, spectator: false },
            // offer 喂给刚建的 PC（transport 生成 answer 后回 RtcReady）。
            Effect::FeedOffer { tag: from.to_string(), offer: offer.to_string(), encrypted: false },
            Effect::Emit,
        ]);
        fx
    }

    /* ---------- 挑战 ---------- */

    fn server_on_challenge(&mut self, from: &str, pl: &serde_json::Value) -> Vec<Effect> {
        if self.phase == Phase::Playing {
            return vec![self.signal_effect(from, "reject", serde_json::json!({ "name": self.display_name(), "reason": "正在对局中" }))];
        }
        if self.phase != Phase::Home {
            return vec![self.signal_effect(from, "reject", serde_json::json!({ "name": self.display_name(), "reason": "当前不可接受挑战" }))];
        }
        // 双挑战对撞 tiebreak（确定性）：双方互发挑战时，字典序大的一方自动让位拒收，
        // 小的一方弹窗——两端决策必然一致，不再死锁。
        if self.outgoing_challenge.as_deref() == Some(from) {
            if self.user_id.as_str() > from {
                return vec![self.signal_effect(from, "reject", serde_json::json!({ "name": self.display_name(), "reason": "双方同时发起挑战，已让位对方" }))];
            }
            self.outgoing_challenge = None;
        }
        let kind_s = pl["kind"].as_str().unwrap_or("gomoku").to_string();
        let size = sanitize_size(pl["size"].as_u64());
        self.incoming = Some(Incoming {
            from: from.to_string(),
            from_name: jstr(pl, "name", from),
            kind: kind_s,
            size,
            game_id: String::new(),
            rtc_ans: None,
        });
        vec![Effect::Emit]
    }

    /// 大厅挑战被接受后：发起者（执黑）建局并送 offer。建局全流程复用
    /// create_invite（gameId/pwd/specPwd 由 ctx 随机源生成），随后按挑战流发 offer。
    pub(crate) fn server_admit_challenger(&mut self, from: &str, ctx: &ReduceCtx) -> Vec<Effect> {
        let mut fx = lobby::create_invite(self, ctx);
        // 挑战流不发 accept（对方收到的是 challenge-accepted 已隐含同意）。
        fx.extend(self.server_proceed_join(from, false));
        fx
    }

    /// 大厅挑战（服务器模式）：直接向对方发挑战信（无 pwd，需对方手动同意）。
    pub fn server_challenge_peer(&mut self, to: &str) -> Vec<Effect> {
        if self.phase != Phase::Home {
            return vec![Effect::Notice(Some("当前状态不可发起挑战".into()), Some(2400))];
        }
        self.outgoing_challenge = Some(to.to_string());
        vec![
            self.signal_effect(to, "challenge", serde_json::json!({ "name": self.display_name(), "kind": self.kind, "size": self.size })),
            Effect::Notice(Some("挑战已发出，等待对方同意…".into()), Some(8000)),
            Effect::Emit,
        ]
    }

    /// 被挑战者同意：回 challenge-accepted，等对方建局送 offer。
    pub fn server_accept_challenge(&mut self) -> Vec<Effect> {
        let Some(inc) = self.incoming.clone() else { return Vec::new() };
        self.incoming = None;
        vec![
            self.signal_effect(&inc.from, "challenge-accepted", serde_json::json!({ "name": self.display_name() })),
            Effect::Notice(Some("已接受挑战，等待对方建立直连…".into()), Some(6000)),
            Effect::Emit,
        ]
    }

    pub fn server_reject_challenge(&mut self) -> Vec<Effect> {
        let Some(inc) = self.incoming.clone() else { return Vec::new() };
        self.incoming = None;
        vec![
            self.signal_effect(&inc.from, "reject", serde_json::json!({ "name": self.display_name(), "reason": "已拒绝挑战" })),
            Effect::Notice(Some("已拒绝该挑战".into()), Some(2400)),
            Effect::Emit,
        ]
    }

    /* ---------- 观战房间 ---------- */

    fn server_on_spec_join(&mut self, from: &str, pl: &serde_json::Value, _ctx: &ReduceCtx) -> Vec<Effect> {
        if !self.spectate_enabled {
            return vec![self.signal_effect(from, "spec-reply", serde_json::json!({ "name": self.display_name(), "ok": false, "reason": "房主已关闭观战" }))];
        }
        if self.phase == Phase::Home {
            return vec![self.signal_effect(from, "spec-reply", serde_json::json!({ "name": self.display_name(), "ok": false, "reason": "对局不存在" }))];
        }
        let ok = pl["pwd"].as_str().is_some_and(|p| !p.is_empty() && Some(p) == self.spec_pwd.as_deref());
        if ok {
            self.server_admit_spectator(from, &jstr(pl, "name", from))
        } else {
            if !self.spec_requests.iter().any(|x| x.from == from) {
                self.spec_requests.push(SpecRequest { from: from.to_string(), from_name: jstr(pl, "name", from) });
            }
            vec![
                self.signal_effect(from, "spec-pending", serde_json::json!({ "name": self.display_name() })),
                Effect::Emit,
            ]
        }
    }

    /// 对局者：受理一次观战（建 spectator 直连；offer 生成完成后在 RtcReady
    /// 分支加入名册、发 spec-offer 并广播名单）。
    pub(crate) fn server_admit_spectator(&mut self, sid: &str, sname: &str) -> Vec<Effect> {
        if self.phase == Phase::Home {
            return Vec::new();
        }
        self.relay_targets.insert(sid.to_string());
        self.pending_specs.push((sid.to_string(), sname.to_string()));
        vec![
            Effect::CreatePeer { tag: sid.to_string(), inviter: true, spectator: true },
            Effect::Emit,
        ]
    }

    /// 观战者：收到房主 offer——进观战态并回 answer。
    pub(crate) fn server_accept_spectator_offer(&mut self, from: &str, offer: &str, game_id: &str) -> Vec<Effect> {
        if self.role != Role::Spectator {
            return Vec::new();
        }
        let mut fx = Vec::new();
        if !game_id.is_empty() {
            self.game_id = Some(game_id.to_string());
            fx.push(Effect::JoinChannel(game_id.to_string()));
        }
        if self.phase != Phase::Playing {
            self.phase = Phase::Playing;
            self.pwd = None;
            self.invite_url = None;
            self.watch_url = None;
            fx.push(Effect::Notice(Some("观战模式：只读同步，不可落子".into()), None));
        }
        fx.push(Effect::CreatePeer { tag: from.to_string(), inviter: false, spectator: true });
        fx.push(Effect::FeedOffer { tag: from.to_string(), offer: offer.to_string(), encrypted: false });
        fx.push(Effect::Emit);
        fx
    }

    fn server_on_spec_sync(&mut self, pl: &serde_json::Value) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Some(list) = pl["list"].as_array() {
            self.spectators = list
                .iter()
                .map(|s| Spectator {
                    id: s["id"].as_str().unwrap_or("").to_string(),
                    name: s["name"].as_str().unwrap_or("").to_string(),
                    host: s["host"].as_str().unwrap_or("").to_string(),
                    muted: s["muted"].as_bool().unwrap_or(false),
                })
                .collect();
            // 被踢检测：名单里没有我但我仍有 host（仍在观战流程）。
            let me_still = self.spectators.iter().any(|s| s.id == self.user_id);
            if !me_still && self.my_host.is_some() && self.role == Role::Spectator {
                self.my_host = None;
                self.spec_can_chat = false;
                fx.push(Effect::ClosePeers);
                fx.extend(self.do_back_home());
                fx.push(Effect::Notice(Some("你已被移出观战".into()), Some(3600)));
            }
        }
        // enabled 必须同写：只写展示态会被本端下次名单同步覆盖回去。
        if let Some(enabled) = pl["enabled"].as_bool() {
            self.spectate_enabled = enabled;
        }
        fx.push(Effect::Emit);
        fx
    }

    fn server_on_spec_chat_req(&mut self, from: &str, pl: &serde_json::Value) -> Vec<Effect> {
        if self.phase != Phase::Playing {
            return Vec::new();
        }
        let is_relay = pl["relay"] == serde_json::json!(true);
        let applicant = if is_relay { jstr(pl, "applicant", from) } else { from.to_string() };
        let from_name = jstr(pl, "name", from);
        let relay = if is_relay {
            None
        } else {
            self.opponent.clone().map(|opp| SpecChatRelay { to: opp, from_name: from_name.clone(), applicant: applicant.clone() })
        };
        self.enqueue_confirm(ConfirmReq {
            kind: ConfirmKind::SpecChat,
            from: from.to_string(),
            from_name,
            on_accept: ConfirmAction::SpecChatAck { applicant: applicant.clone(), ok: true, relay },
            on_reject: ConfirmAction::SpecChatAck { applicant, ok: false, relay: None },
        });
        vec![Effect::Emit]
    }

    /* ---------- 观战房间管理（对局者权限） ---------- */

    /// 批准聊天区里的观战申请。
    pub fn approve_spec_request(&mut self, from: &str) -> Vec<Effect> {
        let Some(req) = self.spec_requests.iter().find(|x| x.from == from) else {
            return Vec::new();
        };
        let name = req.from_name.clone();
        self.spec_requests.retain(|x| x.from != from);
        self.server_admit_spectator(from, &name)
    }

    pub fn reject_spec_request(&mut self, from: &str) -> Vec<Effect> {
        let Some(req) = self.spec_requests.iter().find(|x| x.from == from) else {
            return Vec::new();
        };
        let name = req.from_name.clone();
        self.spec_requests.retain(|x| x.from != from);
        vec![
            self.signal_effect(from, "spec-reply", serde_json::json!({ "name": self.display_name(), "ok": false, "reason": "房主拒绝了观战申请" })),
            Effect::Notice(Some(format!("已拒绝 {name} 的观战申请")), Some(2400)),
            Effect::Emit,
        ]
    }

    /// 踢出观战者：自己的直接关连接；对方直连的经 spec-kick 转移处理。
    /// 先发 spec-kicked 通知再移除——移除后 spec-sync 就送不到被踢者了。
    pub fn kick_spectator(&mut self, id: &str) -> Vec<Effect> {
        self.kick_spectator_why(id, "kick")
    }

    pub(crate) fn kick_spectator_why(&mut self, id: &str, why: &str) -> Vec<Effect> {
        let Some(target) = self.spectators.iter().find(|s| s.id == id) else {
            return Vec::new();
        };
        let host = target.host.clone();
        let mut fx = Vec::new();
        if host == self.user_id {
            if self.server_state == "ready" {
                fx.push(self.signal_effect(id, "spec-kicked", serde_json::json!({ "name": self.display_name() })));
            }
            self.rtc_peers.retain(|q| q.tag != id);
        } else if self.server_state == "ready" {
            fx.push(self.signal_effect(&host, "spec-kick", serde_json::json!({ "name": self.display_name(), "id": id })));
        }
        self.spectators.retain(|s| s.id != id);
        fx.extend(self.push_spec_sync_why(why));
        fx.push(Effect::Emit);
        fx
    }

    pub fn mute_spectator(&mut self, id: &str, muted: bool) -> Vec<Effect> {
        for s in &mut self.spectators {
            if s.id == id {
                s.muted = muted;
            }
        }
        let mut fx = self.push_spec_sync();
        fx.push(Effect::Emit);
        fx
    }

    /// 关闭观战：本局所有人无法观战，全部踢出。
    pub fn disable_spectate(&mut self) -> Vec<Effect> {
        self.disable_spectate_why("disable")
    }

    pub(crate) fn disable_spectate_why(&mut self, why: &str) -> Vec<Effect> {
        self.spectate_enabled = false;
        self.spec_pwd = None;
        let hosts: Vec<(String, String)> = self.spectators.iter().map(|s| (s.id.clone(), s.host.clone())).collect();
        let mut fx = Vec::new();
        for (sid, host) in hosts {
            if host == self.user_id {
                if self.server_state == "ready" {
                    fx.push(self.signal_effect(&sid, "spec-kicked", serde_json::json!({ "name": self.display_name() })));
                }
                self.rtc_peers.retain(|q| q.tag != sid);
            } else if self.server_state == "ready" {
                fx.push(self.signal_effect(&host, "spec-kick", serde_json::json!({ "name": self.display_name(), "id": sid })));
            }
        }
        self.spectators.clear();
        self.spec_url = None;
        fx.extend(self.push_spec_sync_why(why));
        fx.push(Effect::Notice(Some("已关闭本局观战".into()), Some(3000)));
        fx.push(Effect::Emit);
        fx
    }

    /// 观战者：申请发言（一次机会，双 host 均批准才可发言）。
    pub fn request_spec_chat(&mut self) -> Vec<Effect> {
        if self.role != Role::Spectator {
            return Vec::new();
        }
        if self.spec_denied {
            return vec![Effect::Notice(Some("本局发言申请已被拒绝，无法再次申请".into()), Some(3000))];
        }
        match self.my_host.clone() {
            Some(host) if self.server_state == "ready" => vec![
                self.signal_effect(&host, "spec-chat-req", serde_json::json!({ "name": self.display_name() })),
                Effect::Notice(Some("发言申请已发送，等待双方同意…".into()), Some(6000)),
                Effect::Emit,
            ],
            _ => Vec::new(),
        }
    }

    /* ---------- 工具 ---------- */

    /// 房间名单变化后向对方对局者与自己的观战者广播。
    pub(crate) fn push_spec_sync(&self) -> Vec<Effect> {
        self.push_spec_sync_why("")
    }

    pub(crate) fn push_spec_sync_why(&self, why: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        let payload = serde_json::json!({
            "list": self.spectators,
            "enabled": self.spectate_enabled,
            "_why": why,
        });
        if let Some(opp) = self.opponent.as_ref() {
            if self.server_state == "ready" {
                fx.push(self.signal_effect(opp, "spec-sync", payload.clone()));
            }
        }
        for s in &self.spectators {
            // 只发给「挂在我名下」的观战者；对方直连的由对方发（避免双发覆盖）。
            if s.host == self.user_id && self.server_state == "ready" {
                fx.push(self.signal_effect(&s.id, "spec-sync", payload.clone()));
            }
        }
        fx
    }

    /// 通用 signal 上行封装。
    pub(crate) fn signal_effect(&self, to: &str, kind: &str, payload: serde_json::Value) -> Effect {
        Effect::SendServer(serde_json::json!({ "t": "signal", "to": to, "kind": kind, "payload": payload }))
    }
}

/* ---------------- 辅助函数 ---------------- */

/// 从 payload 取字符串字段（缺省回退）。
pub(crate) fn jstr(v: &serde_json::Value, key: &str, fallback: &str) -> String {
    v[key].as_str().unwrap_or(fallback).to_string()
}

/// size 白名单净化（9/13/15/19 之外回 15）。
pub(crate) fn sanitize_size(v: Option<u64>) -> SizeT {
    match v {
        Some(9) => 9,
        Some(13) => 13,
        Some(19) => 19,
        _ => 15,
    }
}
