//! 上下文压缩 —— 上限之下的弹性触发线（移植 `.ref/pi` compaction.ts 的参数语义）。
//!
//! 三个数（计划「compact」节拍板）：
//! - **上限** = 用户配置（默认 176_000，`goptop:agent-ctx-limit`，clamp [8k, 1M]）；
//! - **触发线** = `used > 上限 − [`RESERVE_TOKENS`]`——引擎内部的弹性提前量，
//!   **不是用户配置项**（对齐 pi 默认 compaction.ts:150）；
//! - **保留尾部** ≈ [`KEEP_RECENT_TOKENS`] tokens（对话近端不进摘要，对齐
//!   pi compaction.ts:151）。
//!
//! 计量：**provider usage 为主**（三协议 usage 已归一，[`crate::llm::Usage`]）+
//! 其后消息 chars/4 估算（[`estimate_tokens`]）——粗估只喂触发线判断，误差无害。
//!
//! 切点纪律：**只在 user 消息处切，绝不拆 assistant 工具调用与 tool_result 对**——
//! 拆开的对回给协议就是坏请求（Anthropic 的 tool_use 无配对 tool_result 直接 4xx）。
//!
//! 压缩后结构 = `[system] + [user: <compaction-summary> 摘要] + [retainedTail]`，
//! 重排由循环做，本模块只产出摘要文本与保留段切点。摘要：独立一次 LLM 请求，
//! 有 previousSummary 走增量更新（更新而非重写——早期的对手风格观察不能每轮换血）。
//! 超窗兜底：`LlmError::ContextWindowExceeded` → 生效上限取 min(用户上限, 模型实限)
//! → 强制压缩 → 重试一次 → 再失败 Fatal（判定与重试在循环，本模块供给判定参数）。

use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::{LlmClient, LlmError, Msg};

/// 触发线预留 tokens：`used > 上限 − 16_384` 即压缩（pi 默认 compaction.ts:150）。
pub const RESERVE_TOKENS: u64 = 16_384;
/// 压缩后保留的尾部 tokens（≈20_000；pi 默认 compaction.ts:151）。
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// 消息序列的 token 估算：chars/4（其后消息无 provider usage 可用时唯一的量尺）。
/// **粗估**——触发线判断允许一两个百分点的误差，精确计量等 provider usage 到账。
#[must_use]
pub fn estimate_tokens(messages: &[Msg]) -> u64 {
    todo!()
}

/// 压缩器抽象（Mock 可替换，供压缩单测不花钱）。
#[async_trait]
pub trait Compactor: Send + Sync {
    /// 触发线判定：`used > ctx_limit − RESERVE_TOKENS`。
    #[must_use]
    fn should_compact(&self, used_tokens: u64) -> bool;

    /// 切点：返回保留尾部的**起始消息下标**。
    ///
    /// 从尾部向前累计到 ≈`keep_recent_tokens` 处定切点，然后**回退到最近的 user
    /// 消息边界**（绝不拆 assistant 工具调用与 tool_result 对——见模块注；回退后
    /// 保留量可能略超 keep_recent，这是纪律的代价，可接受）。
    #[must_use]
    fn cut_point(&self, messages: &[Msg], keep_recent_tokens: u64) -> usize;

    /// 一次压缩：独立 LLM 请求产摘要文本（有 previousSummary 走增量更新）。
    /// 压缩后结构 `[system] + [user: 摘要] + [retainedTail]` 的重排由循环做。
    ///
    /// # Errors
    /// 摘要请求失败（透传 [`LlmError`]——循环按其分类决定重试/致命）。
    async fn summarize(
        &self,
        messages: &[Msg],
        previous_summary: Option<&str>,
    ) -> Result<String, LlmError>;
}

/// 默认实现：摘要请求走同一个 [`LlmClient`]（独立一次调用，不计入
/// `max_llm_calls` 预算——压缩是循环的维护动作，不是模型的一次「思考」）。
pub struct SummaryCompactor {
    pub llm: Arc<LlmClient>,
    /// 用户配置的上下文上限（触发线判定用；clamp 在 AgentHub 装配时做）。
    pub ctx_limit: u64,
}

#[async_trait]
impl Compactor for SummaryCompactor {
    fn should_compact(&self, used_tokens: u64) -> bool {
        todo!()
    }

    fn cut_point(&self, messages: &[Msg], keep_recent_tokens: u64) -> usize {
        todo!()
    }

    async fn summarize(
        &self,
        messages: &[Msg],
        previous_summary: Option<&str>,
    ) -> Result<String, LlmError> {
        todo!()
    }
}
