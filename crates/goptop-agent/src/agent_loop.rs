//! 内置模式决策循环 —— 驱动 B 席的「真思考、会犯错」的模型玩家。
//!
//! 一轮的形状（骨架对齐 `.ref/pi/packages/agent/src/agent-loop.ts`）：
//! `maybe_compact` → `llm.chat(ctx, tools)` → ToolCalls 逐个 `execute` 回填
//! （RespondToModel → isError 文本），全部 terminate 则 break。
//!
//! **事件自动推送**（内置模式无等待工具，与 MCP 的 wait_events 相对）：每轮工具
//! 执行完毕后，排空事件队列，把新事件以 `<event>` 块注入 user 消息再进下一轮 LLM
//! 调用——对手落子/消息/请求/终局**无需模型主动查询**；队列为空且轮到对手时，注入
//! 「等待对方行动中」心跳说明，模型可选择做别的（写记忆/委托 delegate 深思/聊天）
//! 或直接等。
//!
//! **TextOnly ≠ 行动**：只回文字、零工具调用**不是行动**，循环注入提醒「必须通过
//! 工具行动（write 暂存 + submit 提交皆可）」并继续——绝不因此终止、绝不自动认输。
//!
//! **退出仅五种**（除此之外任何情况都继续循环）：
//! 1. winner 出现（先 write `/game/in/chat` 收尾语 + submit，再 break）；
//! 2. `submit("/game/in/resign")` 的 terminate（用户停止按钮也走这条路：先 Resign
//!    再停，防孤儿局卡死 A'）;
//! 3. 用户 agent_stop（`LoopDeps.stop` 置位，在每轮 LLM 调用前检查）；
//! 4. Fatal（连接/配置损坏）；
//! 5. 硬预算 `max_llm_calls`（默认 240）打满。
//!
//! **不强制每轮落子**——每轮以工具调用收束即可。`stopReason=="length"` 时本轮
//! 工具调用作废、以错误文本回填（照 agent-loop.ts:474-500：半截调用的参数是
//! 截断的 JSON，执行必错）。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::llm::{LlmClient, LlmConfig};
use crate::registry::ToolCtx;

/// LLM 调用硬预算默认值（五退出条件之一；计划拍板 240——一局五子棋的思考余量
/// 绰绰有余，同时兜住失控循环的成本上限，计划 R2）。
pub const DEFAULT_MAX_LLM_CALLS: u32 = 240;
/// 上下文上限默认值（hard ceiling；用户可设置 `goptop:agent-ctx-limit`，
/// clamp [8k, 1M]——设置卡可改，MCP 模式同值不另设参数）。
pub const DEFAULT_CTX_LIMIT: u32 = 176_000;

/// 循环参数。
#[derive(Clone, Copy, Debug)]
pub struct LoopConfig {
    /// LLM 调用硬预算（五退出条件之五）。
    pub max_llm_calls: u32,
    /// 上下文上限 tokens（compact 触发线在它之下弹性提前——
    /// `used > ctx_limit − RESERVE_TOKENS`，见 compact.rs）。
    pub ctx_limit: u32,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self { max_llm_calls: DEFAULT_MAX_LLM_CALLS, ctx_limit: DEFAULT_CTX_LIMIT }
    }
}

/// 循环结局（AgentHub 转 agent_status 的 done/error 依据）。
#[derive(Debug)]
pub enum LoopStop {
    /// 终局（winner 已现，收尾语已 submit）。
    GameOver,
    /// 认输落地（submit /game/in/resign 的 terminate；用户停止按钮先 Resign 再停）。
    Resigned,
    /// 用户中止（stop 置位；不补认输——认输的落盘由 AgentHub 的 stop 序列负责）。
    Stopped,
    /// 预算打满（max_llm_calls 用尽，局没打完——按成本保护处理，不是错误）。
    BudgetExhausted,
    /// 致命错误（连接/配置损坏；文本进 agent_status.error）。
    Fatal(String),
}

/// 循环统计（agent_status 的 llm_calls/tokens/compactions 数据源）。
#[derive(Clone, Copy, Debug, Default)]
pub struct LoopStats {
    pub llm_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub compactions: u32,
}

/// 循环的装配：客户端、上下文、中止开关。
pub struct LoopDeps {
    pub llm: Arc<LlmClient>,
    /// LLM 配置（replyLang 的注入走 prompt.rs；这里主要给 max_output_tokens 与
    /// 协议能力判定）。
    pub llm_cfg: LlmConfig,
    pub cfg: LoopConfig,
    /// 工具执行上下文（B 席/暂存/事件/记忆）。
    pub tools: ToolCtx,
    /// 用户中止标志（agent_stop 置位；循环在**每轮 LLM 调用前**检查——
    /// 检查点放在调用前而非工具间隙，保证「点了停止就不再花钱」）。
    pub stop: Arc<AtomicBool>,
}

/// 循环结局 + 统计。
#[derive(Debug)]
pub struct LoopOutcome {
    pub stop: LoopStop,
    pub stats: LoopStats,
}

/// 跑完整局：阻塞到五退出条件之一。事件自动推送 / TextOnly 提醒 / 预算与压缩的
/// 语义见模块注。对话历史由循环自持（compact 后重排），从空对话开始——系统提示词
/// 由 [`crate::prompt::build_system_prompt`] 生成后作为 ChatRequest.system 常驻。
pub async fn run(deps: LoopDeps) -> LoopOutcome {
    todo!()
}
