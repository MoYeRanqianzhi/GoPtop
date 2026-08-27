//! 联机消息 — 端到端明文 `GameMsg`，外层由 iroh 的端到端加密保护。
//!
//! 消息为小 JSON（每步 <1KB），含单调 `seq` 供去重/重放。relay 仅做 dumb forward，不解析内容。

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

/// 房间票据 — 包含 `NodeId` + relay 信息，编码为可复制链接/二维码。
/// 具体字段在 Phase 3 对接 iroh 时补齐，此处先占位，保证 `protocol` 模块可独立编译与测试。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomTicket {
    /// 票据的字符串表示（base64/URL-safe），Phase 3 由 `iroh::NodeAddr` 编码。
    pub ticket: String,
}

impl RoomTicket {
    #[must_use]
    pub fn new(ticket: String) -> Self {
        Self { ticket }
    }
}
