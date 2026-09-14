//! 对局会话状态机 — 传输与 UI 之外的「一切功能」（Elm 风格）。
//!
//! 架构：`reduce(Event) -> Vec<Effect>`。所有平台 IO（WebSocket/RTCPeerConnection/
//! BroadcastChannel/storage/剪贴板/随机数/时钟）不在本 crate——状态机产出
//! [`Effect`] 由 goptop-transport 执行，IO 结果以 [`Event`] 喂回。因此：
//! - 全部业务行为可在 native 下单元测试（直接驱动 reduce）；
//! - TS 侧只剩「渲染 [`Session::snapshot`] 快照 + 把 DOM 事件转成 UiCommand」。
//!
//! 行为对齐历史 useGameSession.tsx（2026-09-15 迁移基线），并内置三项增强：
//! 1. **协商弹窗队列**：confirm 队列化，未决请求不再被新请求覆盖（原单槽闭包
//!    续体改为数据化 [`ConfirmAction`]，队列逐个消费）；
//! 2. **围棋终局计分同步**：双 Pass 后死子标记（ScoreMark 全量集合）+ 双确认
//!    计分（ScoreConfirmReq/Ack），结果按中国规则区域法（`goptop-core::score_area`）；
//! 3. **无服务器跨设备观战**：spec 链接携带预生成观战 offer（`&specrtc=`），
//!    观众回观战回执（answerToUrl+spec=1），对局者粘贴受理后自动换发新链接。
//!
//! 随迁移顺带修正的审查项：join 无互斥（有对手时拒绝新 join）、双挑战对撞按
//! userId 字典序确定性 tiebreak、spec 转发目标一律用 opponent（不再 find(id!==from)）。

pub mod lobby;
pub mod matchplay;
pub mod server;
pub mod snapshot;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::dedup::{is_dedupable, DedupTable};
use crate::links::AnswerIntent;
use crate::protocol::{Color, CoordT, GameMsg, MsgKind, MoveT, SizeT};
use goptop_core::board::Stone;
use goptop_core::game::{GameKind, GameState, Move};
use goptop_core::go;

/* ---------------- 基础类型 ---------------- */

/// reduce 上下文：平台注入的时间与随机数（pwd/gameId 生成、时间戳）。
/// 每次调用 reduce 都传新鲜值；rand 数组按需取用（一次 reduce 内最多 4 个随机数）。
#[derive(Clone, Copy, Debug)]
pub struct ReduceCtx {
    pub now_ms: u64,
    pub rand: [u32; 4],
}

/// 会话阶段（TS Phase）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Home,
    Waiting,
    Playing,
}

/// 角色（TS Role）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Idle,
    Inviter,
    Invitee,
    Spectator,
}

/// 名册用户（服务器 peers / 同源 presence 共用）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    /// "idle" | "waiting" | "in-game"
    pub status: String,
    #[serde(rename = "gameId")]
    pub game_id: Option<String>,
}

/// 观战房间成员（双方对局者共同维护，spec-sync 同步）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Spectator {
    pub id: String,
    pub name: String,
    /// 受理自己的对局者（服务器 ID）——踢人/散场时按此路由。
    pub host: String,
    pub muted: bool,
}

/// 观战申请（pwd 错/无 → 私有申请，仅被申请的对局者可见）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SpecRequest {
    pub from: String,
    #[serde(rename = "fromName")]
    pub from_name: String,
}

/// 聊天记录条目。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ChatEntry {
    #[serde(rename = "userId")]
    pub user_id: String,
    pub name: String,
    pub text: String,
    pub ts: u64,
    #[serde(rename = "self")]
    pub self_sent: bool,
}

/// 协商/确认弹窗类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    Undo,
    Reset,
    Swap,
    SpecChat,
    WrongPwd,
    ScoreConfirm,
}

/// 弹窗决定后的数据化动作（原 TS 闭包续体的 Rust 形态：队列消费时逐条解释执行）。
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    SendAck(&'static str, bool),
    /// 错钥匙的 join：同意走正常受理（accept+offer），拒绝回 reject 理由。
    WrongPwdProceed,
    WrongPwdReject { reason: String },
    /// 观战发言批准：向申请者回 ack；`relay` 为 Some 时同时把申请转发给对方对局者
    /// （第一 host 批准路径；第二 host 只回 ack 不转发，防 A↔B 循环）。
    SpecChatAck { applicant: String, ok: bool, relay: Option<SpecChatRelay> },
    ScoreConfirm { ok: bool },
}

/// 观战发言批准的转发目标（第一 host → 第二 host）。
#[derive(Clone, Debug, PartialEq)]
pub struct SpecChatRelay {
    pub to: String,
    pub from_name: String,
    pub applicant: String,
}

/// 一条待确认请求（队列化：未决时新请求排队而非覆盖）。
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmReq {
    pub kind: ConfirmKind,
    pub from: String,
    pub from_name: String,
    pub on_accept: ConfirmAction,
    pub on_reject: ConfirmAction,
}

/// 服务器挑战信（对方点名邀请）/同源挑战共用。
#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    pub from: String,
    pub from_name: String,
    pub kind: String,
    pub size: SizeT,
    pub game_id: String,
    pub rtc_ans: Option<String>,
}

/// userId 链接意图（服务器模式：连接 ready 后发 join/spec-join）。
#[derive(Clone, Debug, PartialEq)]
pub struct PendingLink {
    pub target: String,
    pub pwd: Option<String>,
    pub spec: bool,
}

/// 定时器标识（transport 按它回调 `Event::Timer`）。
pub type TimerId = &'static str;

/// RTC 连接槽位（transport 层的真实连接按 tag 对应；本层记账 + 暂存信令载荷）。
#[derive(Clone, Debug, PartialEq)]
pub struct PeerSlot {
    /// 连接标识：服务器模式主连创建时为 "main"（join 受理后改名为对端 ID）；
    /// 无服务器主连 "main"；观战预生成 "spec-pending"，受理后改名 "spec-live-N"。
    pub tag: String,
    pub player: bool,
    pub spectator: bool,
    /// 已 open 过（断开才算「中断」）。
    pub opened: bool,
    /// offer/answer 已生成（transport gathering 完成）。
    pub offer_ready: bool,
    /// 暂存的 offer/answer 明文（服务器模式信令 payload 需要；几 KB 内存无妨）。
    pub offer_plain: Option<String>,
    /// join 已受理但 offer 尚未就绪：记录待发对象（offer 就绪即发）。
    pub awaiting_peer: Option<String>,
}

/* ---------------- 会话状态 ---------------- */

/// 对局会话（唯一状态真源）。
pub struct Session {
    /* —— 身份 —— */
    /// 持久 userId（u- 前缀，链接与信令路由的同一套 ID）。
    pub user_id: String,
    /// 页面级随机 ID（GameMsg.sender；三链路去重键之一）。
    pub peer_id: String,
    pub name: String,
    pub avatar: Option<String>,

    /* —— 配置（启动时注入；设置页改动经重载生效） —— */
    pub server_mode: bool,
    /// 当前连接的服务器地址（None = 未连接/无服务器）。
    pub server_url: Option<String>,
    pub stun_urls: Vec<String>,
    /// 分享基地址（Web=location.origin；Tauri=云端部署常量；构造链接用）。
    pub share_origin: String,

    /* —— 规则（goptop-core 状态机） —— */
    pub kind: String,
    pub size: SizeT,
    /// 规则引擎；与 kind/size/board/to_move/winner/history 严格一致。
    pub engine: GameState,
    /// TS 侧镜像的棋盘/轮手/胜负（以 engine 的权威回复同步；board 为逻辑尺寸二维表）。
    pub board: Vec<Vec<Color>>,
    pub to_move: Color,
    pub winner: Option<Color>,
    pub history: Vec<crate::protocol::HistoryEntry>,
    pub last_move: Option<CoordT>,
    /// SyncState 回退纪元：本地悔棋/重开 +1 随快照广播（旧守卫双键之一）。
    pub sync_epoch: u32,

    /* —— 会话 —— */
    pub phase: Phase,
    pub role: Role,
    pub game_id: Option<String>,
    pub my_color: Color,
    pub peer_connected: bool,
    pub conn_lost: bool,
    pub pwd: Option<String>,
    /// 观战钥匙（每局生成、整局有效）。
    pub spec_pwd: Option<String>,
    pub spectate_enabled: bool,
    pub invite_url: Option<String>,
    pub watch_url: Option<String>,
    pub answer_back_url: Option<String>,
    /// 无服务器跨设备观战：对局者预生成的观战链接（含 &specrtc=）。
    pub spec_url: Option<String>,

    /* —— 连接 —— */
    pub peers: Vec<PeerInfo>,
    /// 服务器模式的兜底中转目标（对手 + 观战者的服务器 ID）。
    pub relay_targets: BTreeSet<String>,
    /// 当前对手（服务器模式 = 其服务器 ID；优先于 relay_targets 判定）。
    pub opponent: Option<String>,
    /// RTC 槽位记账（真实连接在 transport；tag 对应）。
    pub rtc_peers: Vec<PeerSlot>,
    /// 邀请者的主对局连接 tag（等 answer 用）。
    pub inviter_main: Option<String>,
    /// 观战者连接的房主（服务器 ID / 无服务器 = "main"）。
    pub my_host: Option<String>,
    /// 服务器通道状态："off" | "connecting" | "ready" | "error"。
    pub server_state: String,

    /* —— 意图与信件 —— */
    pub pending_link: Option<PendingLink>,
    pub incoming: Option<Incoming>,
    /// 已发起的同源挑战目标（双向对撞 tiebreak 用）。
    pub outgoing_challenge: Option<String>,
    pub processed_href: Option<String>,
    pub invite_done_href: Option<String>,
    /// 无服务器观战流程：观众侧待回传的 answer（生成观战回执链接用）。
    pub spec_answer: Option<String>,

    /* —— 观战房间 —— */
    pub spectators: Vec<Spectator>,
    pub spec_requests: Vec<SpecRequest>,
    pub spec_can_chat: bool,
    pub spec_chat_acks: BTreeSet<String>,
    pub spec_denied: bool,
    /// 已受理但 offer 尚未生成完的观战（tag → 昵称）：RtcReady 时收编入名单。
    pub pending_specs: Vec<(String, String)>,
    /// 无服务器观战 live 槽计数（受理回执后 spec-pending 改名 spec-live-N）。
    pub spec_live: u32,

    /* —— 协商弹窗队列（增强：不再覆盖未决请求） —— */
    pub confirm_queue: VecDeque<ConfirmReq>,

    /* —— 聊天/头像 —— */
    pub chat_log: Vec<ChatEntry>,
    /// 对端头像：键 = 消息发送方 userId（与 chatLog.userId 一致）。
    pub peer_avatars: HashMap<String, String>,

    /* —— 围棋终局计分（增强） —— */
    pub scoring: bool,
    /// 本方标记的死子集合（全量覆盖语义，ScoreMark 同步）。
    pub my_dead: Vec<CoordT>,
    pub peer_dead: Vec<CoordT>,
    /// 双方各自确认计分。
    pub my_score_ok: bool,
    pub peer_score_ok: bool,
    /// 计分结果（双方确认后填充）。
    pub score_result: Option<snapshot::ScoreResult>,

    /* —— 去重与序号 —— */
    pub seq: u64,
    pub dedup: DedupTable,
}

impl Session {
    /// 构建新会话（身份与配置由 transport 注入；engine 为默认五子棋 15 路占位，
    /// 主页会按用户选择 new_game）。
    #[must_use]
    pub fn new(user_id: String, peer_id: String, name: String, avatar: Option<String>, server_mode: bool, stun_urls: Vec<String>, share_origin: String) -> Self {
        let engine = GameState::new(GameKind::default_gomoku());
        let (board, _n) = empty_board(15);
        Self {
            user_id,
            peer_id,
            name,
            avatar,
            server_mode,
            server_url: None,
            stun_urls,
            share_origin,
            kind: "gomoku".into(),
            size: 15,
            engine,
            board,
            to_move: "black".into(),
            winner: None,
            history: Vec::new(),
            last_move: None,
            sync_epoch: 0,
            phase: Phase::Home,
            role: Role::Idle,
            game_id: None,
            my_color: "black".into(),
            peer_connected: false,
            conn_lost: false,
            pwd: None,
            spec_pwd: None,
            spectate_enabled: true,
            invite_url: None,
            watch_url: None,
            answer_back_url: None,
            spec_url: None,
            peers: Vec::new(),
            relay_targets: BTreeSet::new(),
            opponent: None,
            rtc_peers: Vec::new(),
            inviter_main: None,
            my_host: None,
            server_state: if server_mode { "connecting".into() } else { "off".into() },
            pending_link: None,
            incoming: None,
            outgoing_challenge: None,
            processed_href: None,
            invite_done_href: None,
            spec_answer: None,
            spectators: Vec::new(),
            spec_requests: Vec::new(),
            spec_can_chat: false,
            spec_chat_acks: BTreeSet::new(),
            spec_denied: false,
            pending_specs: Vec::new(),
            spec_live: 0,
            confirm_queue: VecDeque::new(),
            chat_log: Vec::new(),
            peer_avatars: HashMap::new(),
            scoring: false,
            my_dead: Vec::new(),
            peer_dead: Vec::new(),
            my_score_ok: false,
            peer_score_ok: false,
            score_result: None,
            seq: 0,
            dedup: DedupTable::default(),
        }
    }

    /// 服务器模式是否已具备转发通路（relay 兜底判定与落子门槛共用）。
    #[must_use]
    pub fn relay_available(&self) -> bool {
        self.server_mode && self.server_state == "ready" && !self.relay_targets.is_empty()
    }

    /// 生成下一条 GameMsg（seq 单调递增）。
    pub fn next_msg(&mut self, kind: MsgKind) -> GameMsg {
        self.seq += 1;
        GameMsg {
            seq: self.seq,
            sender: self.peer_id.clone(),
            user_id: self.user_id.clone(),
            kind,
        }
    }

    /// 推送聊天（统一入口；self 标记与 200 条上限对齐 TS）。
    pub fn push_chat(&mut self, user_id: &str, name: &str, text: &str, self_sent: bool, now_ms: u64) {
        self.chat_log.push(ChatEntry {
            user_id: user_id.to_string(),
            name: name.to_string(),
            text: text.to_string(),
            ts: now_ms,
            self_sent,
        });
        if self.chat_log.len() > 200 {
            self.chat_log.drain(0..self.chat_log.len() - 200);
        }
    }

    /// 入队协商请求（去重：同一来源同一类型的未决请求不重复入队）。
    fn enqueue_confirm(&mut self, req: ConfirmReq) {
        if self.confirm_queue.iter().any(|r| r.kind == req.kind && r.from == req.from) {
            return;
        }
        self.confirm_queue.push_back(req);
    }

    /// 弹出队首（UI 展示用；approve/decline 时消费）。
    #[must_use]
    pub fn confirm_head(&self) -> Option<&ConfirmReq> {
        self.confirm_queue.front()
    }
}

/* ---------------- 事件 ---------------- */

/// 输入事件（IO 回调 + UI 命令统一入口）。
#[derive(Clone, Debug)]
pub enum Event {
    /// 应用启动：地址栏意图处理（transport 在 mount 时喂一次）。
    Boot { href: String },
    /// UI 命令（按钮点击/输入提交）。
    Ui(UiCommand),
    /// 服务器 WS 下行（transport 解析后的语义事件）。
    Server(ServerEvt),
    /// 对局消息三链路统一入口（BC / DataChannel / relay 已在此汇合，去重在本层）。
    Net(GameMsg),
    /// 同源 presence 下行（announce 名册在 transport 侧直接进 ServerEvt::Peers？——
    /// presence 是独立通道，单独事件）。
    Presence(PresenceEvt),
    /// RTC 连接状态变化。
    PeerState { tag: String, opened: bool, closed: bool, failed: bool },
    /// RTC 异步产物就绪（offer/answer 已由 transport 生成完成）。
    RtcReady { tag: String, offer_plain: Option<String>, answer_plain: Option<String>, offer_enc: Option<String>, answer_enc: Option<String> },
    /// 定时器到期。
    Timer(TimerId),
}

/// UI 命令。
#[derive(Clone, Debug)]
pub enum UiCommand {
    /// 主页开启对战（waiting）。
    CreateInvite,
    /// 受邀者：粘贴链接/点用户主页发起连接（pwd 可空）。
    AcceptInvite { inviter_id: String, pwd: Option<String>, kind: String, size: SizeT, rtc: Option<String> },
    /// 邀请者粘贴回执（对局/观战自动识别）。
    AcceptReceipt(Box<AnswerIntent>),
    /// 同源/挑战弹窗：接受与拒绝。
    AcceptChallenge,
    RejectChallenge,
    /// 服务器大厅挑战。
    ServerChallenge(String),
    ServerAcceptChallenge,
    ServerRejectChallenge,
    /// 落子/停一手/认输（围棋）。
    Place { x: u16, y: u16 },
    Pass,
    Resign,
    /// 协商发起。
    RequestUndo,
    RequestReset,
    RequestSwap,
    /// 弹窗决定。
    ConfirmApprove,
    ConfirmDecline,
    /// 聊天。
    SendChat(String),
    /// 主页设置。
    SetName(String),
    SetAvatar(Option<String>),
    PickKind(String),
    PickSize(SizeT),
    /// 观战房间管理（对局者权限）。
    ApproveSpec(String),
    RejectSpec(String),
    KickSpec(String),
    MuteSpec(String, bool),
    DisableSpectate,
    /// 观战者申请发言。
    RequestSpecChat,
    /// 计分：翻转死子标记 / 确认计分。
    ToggleDead { x: u16, y: u16 },
    ConfirmScore,
    /// 对局者粘贴观战回执（无服务器跨设备观战）。
    AcceptSpecReceipt(Box<AnswerIntent>),
    /// 回主页（离开对局/取消等待）。
    BackHome,
    /// 站内导航意图（popstate / nav 后重处理）。
    Navigate { href: String },
}

/// 服务器 WS 下行语义事件。
#[derive(Clone, Debug)]
pub enum ServerEvt {
    State { s: String, detail: Option<String> },
    Peers { users: Vec<PeerInfo> },
    Signal { from: String, kind: String, payload: serde_json::Value },
    Relayed { from: String, msg: GameMsg },
    Error { msg: String, code: Option<String> },
}

/// 同源 presence 下行。
#[derive(Clone, Debug)]
pub enum PresenceEvt {
    Peers { peers: Vec<PeerInfo> },
    Challenge { from: String, from_name: String, pwd: Option<String>, kind: String, size: SizeT, game_id: String, rtc_ans: Option<String> },
    Accept { from: String, game_id: String },
    Reject { from: String, game_id: String },
}

/* ---------------- 效应 ---------------- */

/// 输出效应（transport 执行）。
#[derive(Clone, Debug)]
pub enum Effect {
    /// 状态快照已变：向 UI 推送 snapshot()。
    Emit,
    /// 提示条（None = 清除；ms = 自动清除毫秒）。
    Notice(Option<String>, Option<u32>),
    /// SPA 导航。
    Nav(String),
    /// WS 上行（hello/announce/signal/relay/ping 的完整 JSON）。
    SendServer(serde_json::Value),
    /// 连接/断开服务器。
    ServerConnect(String),
    ServerClose,
    /// 同源 presence 上行（announce/challenge/accept/reject/bye 的完整 JSON）。
    SendPresence(serde_json::Value),
    /// 对局消息广播：BC + 全部 open 的 DataChannel + relay 兜底（transport 执行三链路）。
    Broadcast(GameMsg),
    /// 建 RTC 连接（inviter 方向 transport 自行 createOffer 并回 RtcReady）。
    CreatePeer { tag: String, inviter: bool, spectator: bool },
    /// 受邀方向：把远端 offer 喂给已有连接（明文=服务器模式，加密=无服务器链接）。
    FeedOffer { tag: String, offer: String, encrypted: bool },
    /// 连接改名（join 受理后 main → 对端 ID；观战受理后 spec-pending → spec-live-N），
    /// transport 侧句柄键同步。
    RenamePeer { from: String, to: String },
    /// 邀请方向：把远端 answer 喂给已有连接（明文/加密同上）。
    AcceptAnswer { tag: String, answer: String, encrypted: bool },
    /// 关闭全部 RTC 连接。
    ClosePeers,
    /// 对局数据 channel 加入（同源 BC）。
    JoinChannel(String),
    /// 离开对局数据 channel。
    LeaveChannel,
    /// 定时器（毫秒；重复 id 应重置）。
    Timer { id: TimerId, ms: u64 },
    /// localStorage/sessionStorage 写。
    SetStorage { key: String, value: Option<String> },
    /// 剪贴板复制（成功提示文案一并给出）。
    Copy { text: String, ok_msg: String },
}

/// 顶层 reduce：Event 进 →（状态变更 + Effect 列表）出。
/// CreatePeer 效应在这里统一补记账槽位（各域只发效应、不管账本）。
pub fn reduce(s: &mut Session, ev: Event, ctx: &ReduceCtx) -> Vec<Effect> {
    let fx = dispatch(s, ev, ctx);
    for e in &fx {
        if let Effect::CreatePeer { tag, inviter, spectator } = e {
            if !s.rtc_peers.iter().any(|q| q.tag == *tag) {
                s.rtc_peers.push(PeerSlot {
                    tag: tag.clone(),
                    player: !*spectator,
                    spectator: *spectator,
                    opened: false,
                    offer_ready: false,
                    offer_plain: None,
                    awaiting_peer: None,
                });
                let _ = inviter;
            }
        }
    }
    fx
}

fn dispatch(s: &mut Session, ev: Event, ctx: &ReduceCtx) -> Vec<Effect> {
    match ev {
        Event::Boot { href } => lobby::on_boot(s, &href),
        Event::Ui(cmd) => match cmd {
            UiCommand::CreateInvite => lobby::create_invite(s, ctx),
            UiCommand::AcceptInvite { inviter_id, pwd, kind, size, rtc } => {
                lobby::accept_invite(s, ctx, &inviter_id, pwd, &kind, size, rtc)
            }
            UiCommand::AcceptReceipt(ans) => lobby::accept_receipt(s, ctx, &ans),
            UiCommand::AcceptSpecReceipt(ans) => lobby::accept_spec_receipt(s, ctx, &ans),
            UiCommand::AcceptChallenge => lobby::accept_challenge(s, ctx),
            UiCommand::RejectChallenge => lobby::reject_challenge(s),
            UiCommand::ServerChallenge(to) => s.server_challenge_peer(&to),
            UiCommand::ServerAcceptChallenge => s.server_accept_challenge(),
            UiCommand::ServerRejectChallenge => s.server_reject_challenge(),
            UiCommand::Place { x, y } => matchplay::place(s, ctx, x, y),
            UiCommand::Pass => matchplay::pass(s, ctx),
            UiCommand::Resign => matchplay::resign(s, ctx),
            UiCommand::RequestUndo => matchplay::request_undo(s),
            UiCommand::RequestReset => matchplay::request_reset(s),
            UiCommand::RequestSwap => matchplay::request_swap(s),
            UiCommand::ConfirmApprove => matchplay::confirm_resolve(s, ctx, true),
            UiCommand::ConfirmDecline => matchplay::confirm_resolve(s, ctx, false),
            UiCommand::SendChat(text) => matchplay::send_chat(s, ctx, &text),
            UiCommand::SetName(name) => lobby::set_name(s, &name),
            UiCommand::SetAvatar(a) => lobby::set_avatar(s, a),
            UiCommand::PickKind(k) => lobby::pick_kind(s, &k),
            UiCommand::PickSize(sz) => lobby::pick_size(s, sz),
            UiCommand::ApproveSpec(id) => s.approve_spec_request(&id),
            UiCommand::RejectSpec(id) => s.reject_spec_request(&id),
            UiCommand::KickSpec(id) => s.kick_spectator(&id),
            UiCommand::MuteSpec(id, muted) => s.mute_spectator(&id, muted),
            UiCommand::DisableSpectate => s.disable_spectate(),
            UiCommand::RequestSpecChat => s.request_spec_chat(),
            UiCommand::ToggleDead { x, y } => matchplay::toggle_dead(s, x, y),
            UiCommand::ConfirmScore => matchplay::confirm_score(s),
            UiCommand::BackHome => s.do_back_home(),
            UiCommand::Navigate { href } => lobby::on_navigate(s, &href),
        },
        Event::Server(sev) => s.on_server(sev, ctx),
        Event::Net(msg) => s.on_net(msg, ctx),
        Event::Presence(pev) => lobby::on_presence(s, ctx, pev),
        Event::PeerState { tag, opened, closed, failed } => lobby::on_peer_state(s, &tag, opened, closed || failed),
        Event::RtcReady { tag, offer_plain, answer_plain, offer_enc, answer_enc } => {
            lobby::on_rtc_ready(s, ctx, &tag, offer_plain, answer_plain, offer_enc, answer_enc)
        }
        Event::Timer(id) => matchplay::on_timer(s, id),
    }
}

/// 空棋盘（逻辑尺寸的二维颜色表）。
#[must_use]
pub fn empty_board(n: SizeT) -> (Vec<Vec<Color>>, SizeT) {
    (vec![vec!["empty".to_string(); n as usize]; n as usize], n)
}

/// board 颜色 ↔ core Stone 换算。
pub(crate) fn color_to_stone(c: &str) -> Stone {
    match c {
        "black" => Stone::Black,
        "white" => Stone::White,
        _ => Stone::Empty,
    }
}

#[allow(dead_code)]
pub(crate) fn stone_to_color(s: Stone) -> Color {
    match s {
        Stone::Black => "black".into(),
        Stone::White => "white".into(),
        _ => "empty".into(),
    }
}

/// 从 engine 重建镜像（board/to_move/winner/history/last_move/scoring/ko）。
pub(crate) fn sync_mirror_from_engine(s: &mut Session) {
    let n = s.engine.kind.size() as SizeT;
    let mut board = vec![vec!["empty".to_string(); n as usize]; n as usize];
    for y in 0..n as usize {
        for x in 0..n as usize {
            let st = s.engine.board.get(goptop_core::board::Coord::new(x as u8, y as u8)).unwrap_or(Stone::Empty);
            board[y][x] = stone_to_color(st);
        }
    }
    s.board = board;
    s.to_move = stone_to_color(s.engine.to_move);
    s.winner = s.engine.winner.map(stone_to_color);
    s.scoring = s.engine.scoring;
    s.history = s
        .engine
        .history
        .iter()
        .map(|m| match m {
            Move::Place(c) => crate::protocol::HistoryEntry::Place(CoordT { x: u16::from(c.x), y: u16::from(c.y) }),
            Move::Pass => crate::protocol::HistoryEntry::Pass("pass".into()),
            Move::Resign => crate::protocol::HistoryEntry::Pass("pass".into()),
        })
        .collect();
    // lastMove 语义是最后一颗落子：向前找最近坐标。
    s.last_move = s.history.iter().rev().find_map(|h| match h {
        crate::protocol::HistoryEntry::Place(c) => Some(*c),
        _ => None,
    });
}

/// kind/size → core GameKind（非法组合回退默认，边界校验职责）。
/// transport 构造会话时也用它，故 pub。
pub fn make_engine_kind(kind: &str, size: SizeT) -> GameKind {
    match (kind, size) {
        ("go", 9) => GameKind::Go { size: 9 },
        ("go", 13) => GameKind::Go { size: 13 },
        ("go", 19) => GameKind::Go { size: 19 },
        ("gomoku", 15) => GameKind::default_gomoku(),
        // 其余组合按棋类回默认尺寸（对齐 TS pickKind 原子修正）。
        ("go", _) => GameKind::Go { size: 19 },
        _ => GameKind::default_gomoku(),
    }
}

/// 快照模块（UI 渲染契约）——见 snapshot.rs。
pub use snapshot::ScoreResult;
