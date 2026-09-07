#![allow(missing_docs)]
//! goptop-transport — P2P 传输抽象（**当前无任何调用者，属未接线代码**）。
//!
//! - `Transport` trait 统一 `create_room`/`join`/`send`/`recv` 语义，预留 Rust 侧复用。
//! - `memory` 仅用于测试与同页联调（无网络）。
//!
//! 现行传输：前端 TS 的 WebRTC DataChannel（STUN-only，禁中转）+ 同源 BroadcastChannel，
//! 见 `.agents/docs/p2p-protocol.md`。本 crate 不依赖 iroh；早期"官方 relay"路线已弃，
//! 引入任何中转都违背项目「无服务器、数据不过第三方」红线（用户拍板，见 .agents/memory/）。

pub mod memory;

use goptop_core::protocol::GameMsg;
use thiserror::Error;

/// 房间票据的字符串表示（URL-safe），由 `create_room` 产生，供对手 `join` 使用。
/// Phase 3 将由 `iroh::NodeAddr`/`Ticket` 编码，此处先以 `String` 占位。
pub type RoomTicket = String;

/// 传输错误。
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("not connected")]
    NotConnected,
    #[error("send failed: {0}")]
    Send(String),
    #[error("recv failed: {0}")]
    Recv(String),
    #[error("room error: {0}")]
    Room(String),
    #[error("{0}")]
    Other(String),
}

/// 统一传输接口。`Send` 约束便于在 `tokio`/`Tauri` 异步上下文中持有。
#[allow(async_fn_in_trait)]
pub trait Transport: Send {
    /// 创建房间，返回可分享的票据（编码后为邀请链接）。
    async fn create_room(&mut self) -> Result<RoomTicket, TransportError>;
    /// 加入房间（通过票据直连对端）。
    async fn join(&mut self, ticket: RoomTicket) -> Result<(), TransportError>;
    /// 发送消息。
    async fn send(&mut self, msg: GameMsg) -> Result<(), TransportError>;
    /// 接收消息（`None` 表示对端已断开或通道关闭）。
    async fn recv(&mut self) -> Option<GameMsg>;
    /// 是否已连接。
    fn is_connected(&self) -> bool;
}
