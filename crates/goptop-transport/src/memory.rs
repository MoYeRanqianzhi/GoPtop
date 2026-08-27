//! 内存传输 — 仅用于测试与同页双实例联调，无网络。
//!
//! 通过 `tokio::sync::mpsc` 在同一进程内模拟 P2P 通道，验证 `GameMsg` 的序号、重放与胜负判定
//! 不依赖真实 relay。生产路径不使用此模块。

use goptop_core::protocol::GameMsg;
use tokio::sync::mpsc;

use crate::{RoomTicket, Transport, TransportError};

/// 内存通道的一端，另一端由 `pair()` 产生。
pub struct MemoryTransport {
    tx: Option<mpsc::UnboundedSender<GameMsg>>,
    rx: Option<mpsc::UnboundedReceiver<GameMsg>>,
    connected: bool,
}

impl MemoryTransport {
    /// 创建一对互联的内存传输（A→B，B→A），用于单页双棋盘联调。
    #[must_use]
    pub fn pair() -> (Self, Self) {
        let (a_tx, b_rx) = mpsc::unbounded_channel();
        let (b_tx, a_rx) = mpsc::unbounded_channel();
        (
            Self {
                tx: Some(a_tx),
                rx: Some(a_rx),
                connected: true,
            },
            Self {
                tx: Some(b_tx),
                rx: Some(b_rx),
                connected: true,
            },
        )
    }

    /// 创建未连接的实例（需 `create_room`/`join` 后才视为已连接，此处为占位）。
    #[must_use]
    pub fn new_unconnected() -> Self {
        Self {
            tx: None,
            rx: None,
            connected: false,
        }
    }
}

#[allow(async_fn_in_trait)]
impl Transport for MemoryTransport {
    async fn create_room(&mut self) -> Result<RoomTicket, TransportError> {
        // 内存模式无真实网络，返回固定票据占位；`pair()` 已建立通道，`is_connected` 保持 true。
        self.connected = true;
        Ok("memory://room".to_string())
    }

    async fn join(&mut self, _ticket: RoomTicket) -> Result<(), TransportError> {
        self.connected = true;
        Ok(())
    }

    async fn send(&mut self, msg: GameMsg) -> Result<(), TransportError> {
        let tx = self.tx.as_ref().ok_or(TransportError::NotConnected)?;
        tx.send(msg).map_err(|e| TransportError::Send(e.to_string()))?;
        Ok(())
    }

    async fn recv(&mut self) -> Option<GameMsg> {
        let rx = self.rx.as_mut()?;
        rx.recv().await
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
