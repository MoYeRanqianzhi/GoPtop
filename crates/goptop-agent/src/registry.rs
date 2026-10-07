//! 工具执行 —— `execute(name, args, ctx)` 一口分发（内置循环与 MCP 出口共用）。
//!
//! 错误两型（计划「工具清单」节）：[`ToolError::RespondToModel`] 是**业务错误**
//! （占点/未轮到/路径不存在/格式错），文本回给模型当工具结果（isError 语义），
//! 循环继续——模型读错误改下一步是正常工作方式；[`ToolError::Fatal`] 是**环境损坏**
//! （连接断/配置坏/存储不可用），循环终止进 error 态。**判断标准：模型能不能靠自己
//! 修复**——能，回模型；不能，致命。
//!
//! 路径解析规则（全工具共用）：`/game` → 动态合成器；`/game/in/*` → 暂存结构；
//! `/memory` → VfsStore；未知路径 → 错误并附 `/index` 提示。

use std::sync::Arc;

use crate::player::{EmitWatch, EventQueue, PlayerHandle};
use crate::store::VfsStore;
use crate::vfs::Staging;
use crate::Driver;

/// 工具执行错误的两型。
#[derive(Debug, Clone)]
pub enum ToolError {
    /// 业务错误——文本原样回给模型（isError 文本），循环继续。
    RespondToModel(String),
    /// 致命——连接/配置损坏，循环终止进 error 态（文本进 agent_status.error）。
    Fatal(String),
}

impl ToolError {
    /// 取错误文本（循环回填 isError 与状态上报共用，避免 match 两遍）。
    #[must_use]
    pub fn message(&self) -> &str {
        todo!()
    }
}

/// 工具执行上下文 —— execute 需要的一切，由 AgentHub/循环装配一次。
///
/// 字段全是 `Arc`/句柄：同一个 ctx 在整局内被反复借用，深拷贝既慢又会撕裂一致性
/// （暂存区尤其不能有两份）。
pub struct ToolCtx {
    /// B 席（Agent 自己）——submit 的动作落点、read/grep 的快照来源。
    pub player: Arc<dyn PlayerHandle>,
    /// B 席的快照推送流（wait_events 的阻塞等待读它；内置模式不消费——
    /// 事件自动推送走 [`Self::events`] 的排空）。
    pub watch: EmitWatch,
    /// 事件队列（/game/events 的数据源；内置模式工具间隙自动排空）。
    pub events: Arc<EventQueue>,
    /// in/ 暂存区（write 落槽、submit 取槽、read 回看，全走这一份）。
    pub staging: Arc<Staging>,
    /// /memory 的存储后端（native=rusqlite；web=IndexedDB 钩子，阶段⑤）。
    pub memory: Arc<dyn VfsStore>,
    /// 记忆命名空间：内置=`builtin`、MCP=`mcp`（[`Driver::memory_ns`] 的预取值——
    /// execute 在热路径上，不值得每跳一次 enum）。
    pub memory_ns: &'static str,
    /// 驱动方式（wait_events/game_* 只对 Mcp 在册；工具清单过滤在循环装配时做，
    /// 这里只挡「外部 Agent 点名调用不在册工具」的越权口）。
    pub driver: Driver,
    /// 子代理开关（delegate 仅内置可选启用，默认关）。
    pub subagent_enabled: bool,
}

/// 工具执行的一口分发。
///
/// 名字不在册（含「在册但本驱动不可用」——如内置模式点 wait_events）→
/// RespondToModel（列出可用工具名，模型自纠）；参数形状不合 schema →
/// RespondToModel；业务拒绝（占点/未轮到/只读路径写/配额超限）→ RespondToModel；
/// 环境损坏 → Fatal。返回值是**序列化好的回执 JSON**（计划样例的返回要点列），
/// 协议适配器只负责装进各自的工具结果壳，不再解释内容。
///
/// game_start/game_leave 的会话对生命周期接线在 MCP handler-state 层（阶段④）——
/// 本函数只做工具面语义，pair() 由 handler 直接调，不从这里穿针。
///
/// # Errors
/// 见 [`ToolError`] 两型的分工。
pub async fn execute(
    name: &str,
    args: &serde_json::Value,
    ctx: &ToolCtx,
) -> Result<serde_json::Value, ToolError> {
    todo!()
}
