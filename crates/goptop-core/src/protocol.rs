//! 联机消息 — 早期参考实现，**已被 `crates/goptop-net/src/protocol.rs` 取代**。
//!
//! 线格式唯一真源是那里的 `GameMsg`/`MsgKind`（字段名对齐历史 TS 线格式，
//! ScoreMark/ScoreConfirmReq 等新类型只在那里）；本模块除 lib.rs 的 re-export 外无消费者，
//! `frontend/src/net/protocol.ts` 现仅保留 UI 侧基础类型（StoneColor/Coord/GameKind/Size）。
//! core 的 GameKind/Stone serde 形状与线格式不同，两者不可混用。
//! 对接路线：链接信令 + WebRTC（见 .agents/docs/p2p-protocol.md），
//! 端到端加密由 WebRTC DTLS 提供（非 iroh）。

use crate::game::GameKind;
use serde::{Deserialize, Serialize};

/// 单调序号，由发送方维护，接收方可据此去重与校验顺序。
pub type Seq = u64;

/// 顶层联机消息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameMsg {
    /// 单调序号。
    pub seq: Seq,
    /// 消息体。
    pub kind: MsgKind,
}

/// 消息体枚举。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MsgKind {
    /// 握手：宣告游戏种类与昵称等（昵称可后加）。
    Hello { kind: GameKind },
    /// 落子/跳过/认输。
    Move(crate::game::Move),
    /// 悔棋请求（需对手同意，对手回 `UndoAck(true/false)`）。
    UndoReq,
    /// 悔棋应答。
    UndoAck(bool),
    /// 聊天文本（可选）。
    Chat(String),
    /// 心跳/保活。
    Ping,
    Pong,
}

impl GameMsg {
    /// 构造消息。
    #[must_use]
    pub fn new(seq: Seq, kind: MsgKind) -> Self {
        Self { seq, kind }
    }
}

/// 房间票据 — 占位类型。现行邀请/回执链接由前端 TS 构造（见 p2p-protocol.md），
/// Rust 侧对接时如仍需票据再实现，此前保持占位保证模块可独立编译。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomTicket {
    /// 票据的字符串表示（URL 安全 base64；编码方案见 `crates/goptop-net/src/codec.rs` 与前端 links）。
    pub ticket: String,
}

impl RoomTicket {
    #[must_use]
    pub fn new(ticket: String) -> Self {
        Self { ticket }
    }
}
