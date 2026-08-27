#![allow(missing_docs)]
//! goptop-transport — P2P 传输抽象。
//!
//! - `Transport` trait 统一 `create_room`/`join`/`send`/`recv` 语义，Web WASM 与 Tauri 原生共用。
//! - `memory` 仅用于测试与同页联调（无网络）。
//! - `iroh` 模块在 Phase 3 引入，封装官方 relay 上的真 P2P（Web 经 relay WS，原生 QUIC+relay 回退）。
//!
//! 约束：很长时间内无自建服务器，全程使用 iroh 官方公有 relay（dumb forward，端到端加密）。

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
