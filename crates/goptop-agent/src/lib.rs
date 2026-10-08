//! goptop-agent —— 「Agent 对战」：Agent 身份等同于玩家（非人类的模型玩家，
//! 真思考、会犯错），拥有人类玩家完整操作面（看棋盘、落子、停一手、悔棋请求/批复、
//! 聊天、认输、计分确认）。权威规格：`.agents/plan/2026-10-08-agent-battle.md`。
//!
//! 关键洞察（计划已一手验证）：`UiCommand` 唯一命令口、`Session::snapshot()` 唯一读口、
//! `tests/headless.rs` 已证明同进程双会话完整对局——**Agent 玩家 = 无头会话 + 决策层，
//! 协议零改动**。内置（应用配置的 LLM 循环）与 MCP（外部 Agent 经内嵌服务器驱动）
//! 两模式共用同一会话对与同一工具 registry，差别只在 B 席的驱动者。
//!
//! 模块地图（依赖方向自上而下，无环）：
//! - [`player`]：玩家席抽象（PlayerHandle）、EmitWatch/HookHost（事件源头）、
//!   GameEvent/EventQueue（事件队列与 /game/events 全量历史）、快照 diff 物化；
//! - [`pair`]：A'（人）与 B（Agent 无头会话）的进程内配对，方向随执子选择；
//! - [`store`]：/memory 的 VfsStore trait（双后端缝）+ 路径规范化与配额；
//! - [`vfs`]：一切皆文件——/game 动态合成、/game/in 两段式暂存、submit 分发；
//! - [`tools`]：9 个工具的纯数据定义（手写 JSON Schema，不引 schemars）；
//! - [`registry`]：execute(name,args,ctx) 一口分发 + ToolError（回模型/致命）；
//! - [`llm`]：三协议统一客户端（enum 免 async-trait 分发）与统一消息类型；
//! - [`agent_loop`]：内置模式决策循环（事件自动推送 / TextOnly 提醒 / 五退出条件）；
//! - [`compact`]：上下文压缩（RESERVE/KEEP_RECENT 参数语义移植 pi compaction.ts）；
//! - [`prompt`]：系统提示词（Claude Code 风格简化版）；
//! - [`mcp`]（feature "mcp"，仅桌面）：rmcp Streamable HTTP 出口——外部 Agent
//!   经内嵌 MCP 服务器认领 B 席（`McpServer::start(cfg)` / `stop()`）。
//! - [`time_compat`]：时间/随机的平台双臂（native=tokio/transport-native，
//!   wasm=setTimeout/`Date.now`）——wasm 上「禁 tokio time」的唯一正门；
//! - [`wasm`]（仅 wasm32）：web 平台臂——B 席宿主基座 `WebHost`（`window.goptop*`
//!   钩子）与时间/随机实现。
//!
//! 范围红线（阶段⑤起修订）：goptop-net / goptop-transport-native / src-tauri /
//! harmony 一行不改；**frontend 与 goptop-transport 自阶段⑤放行**（web 内置模式
//! 的 VfsStore 钩子、agent 出口与 per-core 会话路由在这两处落地）——机制靠
//! 「装饰 Host / PlatformHost」与「调 UiCommand / 读 snapshot」复用。

pub mod agent_loop;
pub mod compact;
pub mod llm;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod pair;
pub mod player;
pub mod prompt;
pub mod registry;
pub mod store;
#[cfg(target_arch = "wasm32")]
pub mod store_web;
pub mod time_compat;
pub mod tools;
pub mod vfs;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

/// 对手驱动方式 —— 同一会话对、同一棋盘、同一聊天，差别只在 B 席的驱动者。
///
/// serde 线上形态用小写（`"builtin"` / `"mcp"`）：它要经 agent_start 的 cfg JSON
/// 从前端传进来，与快照/命令的 camelCase 契约同一条 IPC 边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    /// 内置：应用配置的 LLM 循环驱动 B 席（桌面 + Web）。
    Builtin,
    /// MCP：外部 Agent 经内嵌 MCP 服务器认领席位（仅桌面；feature "mcp"，阶段④）。
    Mcp,
}

impl Driver {
    /// /memory 的命名空间：内置=`builtin`、MCP=`mcp`。
    ///
    /// **为什么按驱动分而不按局分**：记忆是「对手风格」级别的长期积累，同一个
    /// 外部 Agent 多次接入应读到自己的旧笔记；内置循环与外部 Agent 的记忆互不串味。
    #[must_use]
    pub fn memory_ns(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Mcp => "mcp",
        }
    }
}
