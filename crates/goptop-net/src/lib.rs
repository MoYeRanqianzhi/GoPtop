//! goptop-net — 联机协议与对局会话状态机（纯逻辑，无平台依赖）。
//!
//! 分层定位（「Rust 承接一切功能，TS 只做 UI」总要求下的第二阶段产物）：
//! - [`protocol`]：对局线格式唯一真源（GameMsg/MsgKind，serde 对齐历史 TS 线格式，
//!   三链路同源 BC / WebRTC DC / 服务器 relay 传的都是它）；
//! - [`dedup`]：多链路重复送达的 (sender, seq) 单调去重；
//! - [`identity`]：userId/pwd/gameId 生成（随机源注入，wasm 侧接 crypto.getRandomValues）；
//! - [`codec`]：G1 信令编码（deflate-raw + pwd 派生 XOR + URL 安全 base64），
//!   与前端历史实现的 token 双向互通；
//! - [`links`]：邀请/回执/观战/主页链接的构造与解析（旧 query 风格兼容）；
//! - [`session`]：对局会话状态机（Elm 风格：Event 进 → State 变更 + Effect 出），
//!   含协商弹窗队列、观战房间、围棋计分同步与无服务器跨设备观战。
//!
//! 平台 IO（WebSocket/RTCPeerConnection/BroadcastChannel/storage/随机数）一律不在
//! 本 crate：状态机产出 [`session::Effect`] 交由 goptop-transport 执行，执行结果以
//! [`session::Event`] 喂回。native 单测因此可以覆盖全部业务行为。
#![allow(missing_docs)]

pub mod codec;
pub mod dedup;
pub mod identity;
pub mod protocol;

pub use protocol::{Color, CoordT, GameKindT, GameMsg, MoveT, MsgKind, SizeT};
