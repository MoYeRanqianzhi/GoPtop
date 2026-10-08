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
//! 其后消息 chars/4 估算（[`used_tokens`]——循环在每轮请求后记录 usage 基线，
//! 基线之后入史的消息才落回估算；首请求前全量估算兜底）。粗估只喂触发线判断，
//! 误差无害。
//!
//! 切点纪律：**只在 user 消息处切，绝不拆 assistant 工具调用与 tool_result 对**——
//! 拆开的对回给协议就是坏请求（Anthropic 的 tool_use 无配对 tool_result 直接 4xx）。
//!
//! 压缩后结构 = `[system] + [user: <compaction-summary> 摘要] + [retainedTail]`，
//! 循环用 [`compacted_messages`] 一口装配（切点 + 摘要文本 → 新消息序列）。
//! 摘要：独立一次 LLM 请求，有 previousSummary 走增量更新（更新而非重写——早期的
//! 对手风格观察不能每轮换血）。超窗兜底：`LlmError::ContextWindowExceeded` →
//! 生效上限取 min(用户上限, 模型实限)（直接下调 [`SummaryCompactor::ctx_limit`]）
//! → 强制压缩 → 重试一次 → 再失败 Fatal（判定与重试在循环，本模块供给判定参数）。

use std::sync::Arc;

use crate::llm::{Block, ChatRequest, LlmClient, LlmError, Msg, Role};

/// 触发线预留 tokens：`used > 上限 − 16_384` 即压缩（pi 默认 compaction.ts:150）。
pub const RESERVE_TOKENS: u64 = 16_384;
/// 压缩后保留的尾部 tokens（≈20_000；pi 默认 compaction.ts:151）。
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// 图像块的 chars 折算（pi compaction.ts:251 `ESTIMATED_IMAGE_CHARS`）：base64
/// 体积不等于模型消耗，按视觉 token 的经验折算进 chars/4 估算即可。
const ESTIMATED_IMAGE_CHARS: u64 = 4_800;

/// 压缩摘要消息的包裹标签（计划拍板 `[user:<compaction-summary> 摘要]`；包裹句式
/// 沿 pi messages.ts 的 COMPACTION_SUMMARY_PREFIX/SUFFIX——模型对这段话的服从是
/// 实测过的，不自创句式）。
pub const COMPACTION_SUMMARY_TAG: &str = "compaction-summary";

/// 单条消息的 token 估算：chars/4 向上取整（对齐 pi estimateTokens 的逐条 ceil——
/// 逐条 ceil 再求和比整段一次 ceil 略保守，触发线判断宁可信多不可信少）。
fn estimate_msg_tokens(msg: &Msg) -> u64 {
    let mut chars: u64 = 0;
    for block in &msg.content {
        match block {
            Block::Text { text } => chars += text.chars().count() as u64,
            // 工具结果回执以字符串计入——它和文本一样整段进上下文。
            Block::ToolResult { content, .. } => chars += content.chars().count() as u64,
            // 工具调用本体（名字+参数）同样整段上线（tool_use/function_call 块），
            // 不计会低估触发线、压缩来得太晚。
            Block::ToolCall { call } => {
                chars += call.name.chars().count() as u64;
                chars += call.arguments.to_string().chars().count() as u64;
            }
            // thinking 原样块整段上线（Anthropic/Responses 逐字回传），计入触发线。
            Block::ThinkingRaw { data } => chars += data.to_string().chars().count() as u64,
            Block::Image { .. } => chars += ESTIMATED_IMAGE_CHARS,
        }
    }
    chars.div_ceil(4)
}

/// 消息序列的 token 估算：chars/4（其后消息无 provider usage 可用时唯一的量尺）。
/// **粗估**——触发线判断允许一两个百分点的误差，精确计量等 provider usage 到账。
#[must_use]
pub fn estimate_tokens(messages: &[Msg]) -> u64 {
    messages.iter().map(estimate_msg_tokens).sum()
}

/// 触发线计量：**provider usage 为主 + 其后消息估算**（计划 compact 节逐字）。
///
/// `usage_mark = (base, covered)`：最近一次响应上报的 prompt tokens（input +
/// cache_read + cache_write——三者都是模型真实读过的上下文）覆盖到 history 前
/// `covered` 条；其后再入史的消息（assistant 回复、工具回执、事件注入）没有
/// provider 计量，按 chars/4 估算接在 base 之后。无 usage 可用（首请求前的
/// 判定，或压缩换血后旧基线作废）→ 全量估算兜底；`covered` 超过现有长度
/// （理论上只在换血后出现，防御）→ 同样退回全量估算。
#[must_use]
pub fn used_tokens(messages: &[Msg], usage_mark: Option<(u64, usize)>) -> u64 {
    match usage_mark {
        Some((base, covered)) if covered <= messages.len() => {
            base.saturating_add(estimate_tokens(&messages[covered..]))
        }
        _ => estimate_tokens(messages),
    }
}

/// 切点候选是否合法：user 消息**且不携带工具结果**。统一消息形态沿 Anthropic 语义
/// （工具结果装在 user 消息里），在此切会把前面 assistant 的工具调用与结果拆散——
/// 对应 pi compaction.ts:311-340 把 toolResult 条目排除出合法切点。
fn is_valid_cut(msg: &Msg) -> bool {
    msg.role == Role::User
        && !msg.content.iter().any(|b| matches!(b, Block::ToolResult { .. }))
}

/// 压缩摘要请求的独立系统提示词（对齐 pi SUMMARIZATION_SYSTEM_PROMPT：**只许输出
/// 摘要本身**，不许续写对话、不许回答对话里的问题——摘要文本原样进后续上下文，
/// 掺入续写就是毒数据）。
const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. You read a conversation between an LLM game player and its harness (tool calls, tool results, pushed opponent events), then produce a structured summary in the exact format requested.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// 摘要正文的目标格式（新建/增量两版提示词共用同一段格式定义，避免两处漂移）。
const SUMMARY_FORMAT: &str = "## Game state\n[Kind, board size, your color, move count, whose turn, winner/scoring status — exact numbers.]\n\n## Opponent\n[Name, observed style, habits, anything said in chat worth remembering — or \"(none)\".]\n\n## What happened\n[Key moves with coordinates and move numbers, requests and their outcomes, chat exchanged.]\n\n## In flight\n[Staged but unsubmitted actions, pending requests, unfinished plans — or \"(none)\".]\n\n## Strategy notes\n[Your own evaluation, candidate points, risks, plans worth carrying forward — or \"(none)\".]\n\nKeep each section concise. Preserve exact coordinates, move numbers, file paths, and quoted messages verbatim.";

/// 无 previousSummary 时的全量摘要指令（对齐 pi SUMMARIZATION_PROMPT 的
/// 「conversation to summarize」句式，格式段换成对局语义）。
const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint that another instance of the same player will use to continue the game.\n\nUse this EXACT format:\n\n";

/// 有 previousSummary 时的增量更新指令（对齐 pi UPDATE_SUMMARIZATION_PROMPT：
/// 保留旧信息、并入新信息、清掉已了结的事——**更新而非重写**）。
const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information that is still relevant (opponent observations, etiquette, strategy).\n- ADD new moves, events, chat and decisions from the new messages.\n- UPDATE Game state to the latest position — exact move count and turn.\n- UPDATE In flight: drop what has since been submitted or resolved.\n- If something is no longer relevant, you may remove it.\n\nUse this EXACT format:\n\n";

/// 把待摘要的消息序列摊平成纯文本对谈稿（对齐 pi serializeConversation 的
/// `[User]:` / `[Assistant]:` 行式——摘要请求只有一条 user 消息，对谈史全在里面）。
fn serialize_conversation(messages: &[Msg]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            match block {
                // 空文本块跳过（拼接空行只会干扰摘要模型）。
                Block::Text { text } => {
                    if !text.is_empty() {
                        let who = match msg.role {
                            Role::User => "User",
                            Role::Assistant => "Assistant",
                        };
                        parts.push(format!("[{who}]: {text}"));
                    }
                }
                // 工具结果回执进对谈稿（摘要模型需要知道工具侧发生过什么）。
                Block::ToolResult { call_id, content, is_error } => {
                    let mark = if *is_error { " (error)" } else { "" };
                    parts.push(format!("[Tool result for {call_id}{mark}]: {content}"));
                }
                // 模型发起的调用也要进对谈稿——只有结果没有调用的对谈史读不懂因果。
                Block::ToolCall { call } => parts.push(format!(
                    "[Tool call {} ({})]: {}",
                    call.name, call.id, call.arguments
                )),
                Block::Image { .. } => parts.push("[User]: (image attachment)".to_string()),
                // 思维链进对谈稿（摘要模型需要推理脉络；只取明文——回放签名不经过这里）。
                Block::ThinkingRaw { data } => {
                    if let Some(t) = data.get("thinking").and_then(|v| v.as_str()) {
                        parts.push(format!("[Assistant thinking]: {t}"));
                    }
                }
            }
        }
    }
    parts.join("\n")
}

/// 装配摘要请求（独立于 `summarize` 的纯函数——增量路径的有/无 previousSummary
/// 分支在这里可被单元测试钉住，不必真调 LLM）。
///
/// 请求形态：单条 user 消息 = `<conversation>` 对谈稿 +（可选）`<previous-summary>`
/// + 指令；**tools 为空**——摘要是一次维护动作，绝不允许摘要模型去动棋。
fn summarize_request(messages: &[Msg], previous_summary: Option<&str>) -> ChatRequest {
    let mut prompt_text = format!("<conversation>\n{}\n</conversation>\n\n", serialize_conversation(messages));
    if let Some(prev) = previous_summary {
        prompt_text.push_str(&format!("<previous-summary>\n{prev}\n</previous-summary>\n\n"));
    }
    prompt_text.push_str(if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    });
    prompt_text.push_str(SUMMARY_FORMAT);
    // 输出预算对齐 pi generateSummaryWithRequest 的 min(0.8*reserve, ...)：摘要
    // 是压缩动作的内部开销，给满 RESERVE 的八成，吃不掉也不占对局回复预算。
    let max_output_tokens = u32::try_from(RESERVE_TOKENS.saturating_mul(4) / 5).unwrap_or(u32::MAX);
    ChatRequest {
        system: SUMMARIZATION_SYSTEM_PROMPT.to_string(),
        messages: vec![Msg { role: Role::User, content: vec![Block::Text { text: prompt_text }] }],
        tools: Vec::new(),
        max_output_tokens,
    }
}

/// 压缩摘要 user 消息的正文（`compacted_messages` 的第 0 条；包裹句式沿 pi
/// messages.ts 的 COMPACTION_SUMMARY_PREFIX/SUFFIX，标签按计划用 compaction-summary）。
#[must_use]
pub fn summary_message_text(summary: &str) -> String {
    let tag = COMPACTION_SUMMARY_TAG;
    format!("The conversation history before this point was compacted into the following summary:\n\n<{tag}>\n{summary}\n</{tag}>")
}

/// 压缩后消息结构装配：`[user: 摘要] + messages[cut..]`（system 由循环常驻在
/// [`crate::llm::ChatRequest::system`]，不进消息序列）。
///
/// `cut` 越界按整体保留处理（截到 `messages.len()`）——调用方传本模块 [`Compactor::cut_point`]
/// 的产出，正常不会越界；防御只为装配函数自身永远给得出合法切片。
#[must_use]
pub fn compacted_messages(messages: &[Msg], summary: &str, cut: usize) -> Vec<Msg> {
    let cut = cut.min(messages.len());
    let mut out = Vec::with_capacity(1 + messages.len() - cut);
    out.push(Msg {
        role: Role::User,
        content: vec![Block::Text { text: summary_message_text(summary) }],
    });
    out.extend(messages[cut..].iter().cloned());
    out
}

/// 压缩器抽象（Mock 可替换，供压缩单测不花钱）。
// ?Send 界的取舍见 llm/mod.rs HttpChannel 注（wasm 走 spawn_local，future 非 Send）。
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
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

// ?Send 界的取舍见 llm/mod.rs HttpChannel 注（wasm 走 spawn_local，future 非 Send）。
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Compactor for SummaryCompactor {
    fn should_compact(&self, used_tokens: u64) -> bool {
        // saturating：ctx_limit 被 clamp 到下限 8k < RESERVE 时差值为 0，任何非空
        // 上下文都判压缩——窗口本来就不够用，压缩永不嫌早，没有更聪明的小预算策略。
        used_tokens > self.ctx_limit.saturating_sub(RESERVE_TOKENS)
    }

    fn cut_point(&self, messages: &[Msg], keep_recent_tokens: u64) -> usize {
        let valid: Vec<usize> =
            (0..messages.len()).filter(|&i| is_valid_cut(&messages[i])).collect();
        if valid.is_empty() {
            // 连一个合法切点都没有：整段保留（压缩退化为纯摘要替换也做不到）——
            // 正常对局史第一手前必有 user 文本消息，此分支只护畸形输入不 panic。
            return 0;
        }
        // 从尾部向前累计到 ≈keep_recent_tokens 处定试探切点（对齐 pi findCutPoint
        // 的倒序累计；全程不足 keep_recent 则按 pi 落到首个合法切点=几乎全保留）。
        let mut accumulated = 0u64;
        let mut tentative = None;
        for i in (0..messages.len()).rev() {
            accumulated += estimate_msg_tokens(&messages[i]);
            if accumulated >= keep_recent_tokens {
                tentative = Some(i);
                break;
            }
        }
        match tentative {
            // 回退到试探点之前最近的合法 user 边界（保留量略超 keep_recent，可接受）。
            // 回退撞不到任何合法边界时取首个合法切点而非 0：切 0 =「摘要空集+全量
            // 保留」，上下文不减反增，违背压缩本意（对齐 pi findCutPoint 落
            // cutPoints[0] 的行为——保留量可少于 keep_recent，结构必须真正变小）。
            Some(i) => valid.iter().rev().copied().find(|&v| v <= i).unwrap_or(valid[0]),
            None => valid[0],
        }
    }

    async fn summarize(
        &self,
        messages: &[Msg],
        previous_summary: Option<&str>,
    ) -> Result<String, LlmError> {
        let req = summarize_request(messages, previous_summary);
        let resp = self.llm.chat(req).await?;
        Ok(resp.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条纯文本消息的便捷构造（测试专用）。
    fn text_msg(role: Role, text: &str) -> Msg {
        Msg { role, content: vec![Block::Text { text: text.to_string() }] }
    }

    /// usage 计量：chars/4 逐条向上取整求和；工具结果按内容计、图像按 4800 折算。
    #[test]
    fn usage_measurement_chars_over_four() {
        // 40 chars → 恰好 10 tokens（不取整的场景）。
        let msgs = [text_msg(Role::User, &"x".repeat(40))];
        assert_eq!(estimate_tokens(&msgs), 10);

        // 逐条 ceil 语义：两条 5-char 各 ceil(5/4)=2、合计 4；整段一次 ceil(10/4)
        // 只得 3——断言 4 钉住「逐条」。
        let two_short = [text_msg(Role::User, "12345"), text_msg(Role::Assistant, "12345")];
        assert_eq!(estimate_tokens(&two_short), 4, "逐条 ceil(5/4)=2，合计 4");

        // 工具结果按 content 文本计入；is_error 不影响计量。
        let tool = Msg {
            role: Role::User,
            content: vec![Block::ToolResult {
                call_id: "c1".into(),
                content: "y".repeat(80),
                is_error: true,
            }],
        };
        assert_eq!(estimate_tokens(std::slice::from_ref(&tool)), 20);

        // 图像按 4800 chars 折算 → 1200 tokens。
        let img = Msg {
            role: Role::User,
            content: vec![Block::Image { mime: "image/png".into(), data_base64: "aa".into() }],
        };
        assert_eq!(estimate_tokens(std::slice::from_ref(&img)), 1_200);

        // 空序列为 0。
        assert_eq!(estimate_tokens(&[]), 0);
    }

    /// 切点纪律：试探点落在工具调用与结果的对上（assistant 消息或工具结果 user
    /// 消息）时，回退到之前的合法 user 文本边界——工具对绝不被拆开。全程手数
    /// tokens（每条 ≤1），切点逐例可心算验证。
    #[test]
    fn cut_point_never_splits_tool_pair() {
        // 每条恰 1 token：[0] user 文本 / [1] assistant / [2] 工具结果（user 但非法）/
        // [3] user 文本。
        let messages = vec![
            text_msg(Role::User, "wxyz"),
            text_msg(Role::Assistant, "abcd"),
            Msg {
                role: Role::User,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: "r".into(),
                    is_error: false,
                }],
            },
            text_msg(Role::User, "efgh"),
        ];
        let c = SummaryCompactor { llm: mock_client(), ctx_limit: 176_000 };

        // keep=1：从尾部第一条（[3]）就破线 → 切点 3。
        assert_eq!(c.cut_point(&messages, 1), 3);
        // keep=3：累计到 [1] 才破线 → 试探点 1；回退越过工具对（[1]/[2]）到 [0]，
        // 绝不落在 1 或 2——落 2 就把 assistant 的调用与其结果拆散了。
        assert_eq!(c.cut_point(&messages, 3), 0);
        // keep=2：累计到 [2] 破线 → 试探点 2（非法）；回退同样到 0。
        assert_eq!(c.cut_point(&messages, 2), 0);
        // keep 大于全史：无破线点 → 首个合法切点。
        assert_eq!(c.cut_point(&messages, 99), 0);
        // 空序列护底。
        assert_eq!(c.cut_point(&[], 1), 0);
    }

    /// 回退撞不到合法边界（试探点之前全是工具对）时取首个合法切点而非 0——
    /// 切 0 = 摘要空集 + 全量保留，上下文不减反增，违背压缩本意。
    #[test]
    fn cut_point_falls_back_to_first_valid_not_zero() {
        let messages = vec![
            text_msg(Role::Assistant, "abcd"), // 1 token
            Msg {
                role: Role::User,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: "r".into(),
                    is_error: false,
                }],
            }, // 1 token
            text_msg(Role::User, "efgh"), // 1 token，唯一合法切点
        ];
        let c = SummaryCompactor { llm: mock_client(), ctx_limit: 176_000 };
        // keep=2：累计到 [1] 破线 → 试探点 1，其前无合法边界 → 首个合法切点 2。
        assert_eq!(c.cut_point(&messages, 2), 2);
    }

    /// 真实体量的对局史：破线点落在长 assistant 消息上时回退到事件注入的 user
    /// 边界；全史不足 keep_recent 时几乎全保留（对齐 pi 落首个合法切点）。
    #[test]
    fn cut_point_keeps_approximate_recent_budget() {
        let messages = vec![
            text_msg(Role::User, "game start: you are white, 15x15 gomoku"), // 0 合法
            text_msg(Role::Assistant, "reading the board"),                  // 1
            Msg {
                role: Role::User,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: "board json…".into(),
                    is_error: false,
                }],
            }, // 2 工具结果（user 但非法）
            text_msg(Role::User, "<event> opponent moved (7,7)"), // 3 合法
            text_msg(Role::Assistant, &"a".repeat(30_000)),       // 4 ≈7_500 tokens
            text_msg(Role::User, "tail"),                         // 5 合法但太近尾部
        ];
        let c = SummaryCompactor { llm: mock_client(), ctx_limit: 176_000 };
        // keep=5_000：从尾部累计到 [4] 破线 → 试探点 4；回退到最近合法边界=3，
        // 绝不落 2（工具结果消息）。
        assert_eq!(c.cut_point(&messages, 5_000), 3);
        assert!(is_valid_cut(&messages[3]));
        // 全史 ≈7_526 tokens 不足 keep → 几乎全保留。
        assert_eq!(c.cut_point(&messages, 100_000), 0);
    }

    /// 无任何合法切点（无 user 文本消息）时退回 0，不 panic、不产生越界切点。
    #[test]
    fn cut_point_without_valid_boundary_keeps_everything() {
        let messages = vec![
            text_msg(Role::Assistant, &"b".repeat(1_000)),
            Msg {
                role: Role::User,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: "result".into(),
                    is_error: false,
                }],
            },
        ];
        let c = SummaryCompactor { llm: mock_client(), ctx_limit: 176_000 };
        assert_eq!(c.cut_point(&messages, 10), 0);
        assert_eq!(c.cut_point(&[], 10), 0);
    }

    /// previousSummary 增量路径：有旧摘要 → 请求带 <previous-summary> 且换增量指令；
    /// 无旧摘要 → 全量指令、无 previous-summary 段。两版都裸对谈稿、空工具面。
    #[test]
    fn summarize_request_incremental_path() {
        let convo = vec![
            text_msg(Role::User, "you are black"),
            text_msg(Role::Assistant, "understood"),
        ];

        // 全量路径：<conversation> 在、<previous-summary> 不在、指令是全量版。
        let fresh = summarize_request(&convo, None);
        assert_eq!(fresh.messages.len(), 1, "摘要请求只有一条 user 消息");
        assert!(fresh.tools.is_empty(), "摘要模型不得携带工具面");
        assert!(fresh.max_output_tokens > 0);
        let text = match &fresh.messages[0].content[0] {
            Block::Text { text } => text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert!(text.starts_with("<conversation>"));
        assert!(text.contains("[User]: you are black"));
        assert!(text.contains("[Assistant]: understood"));
        assert!(text.contains("</conversation>"));
        assert!(!text.contains("<previous-summary>"));
        assert!(text.contains("conversation to summarize"));
        assert!(text.contains("## Game state"), "格式段必须在场");

        // 增量路径：旧摘要原文进 <previous-summary>，指令换成增量版（保留旧信息）。
        let incr = summarize_request(&convo, Some("old summary: opponent plays corners"));
        let text = match &incr.messages[0].content[0] {
            Block::Text { text } => text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert!(text.contains("<previous-summary>\nold summary: opponent plays corners\n</previous-summary>"));
        assert!(text.contains("Update the existing structured summary"));
        assert!(!text.contains("conversation to summarize"));
        assert_eq!(incr.system, SUMMARIZATION_SYSTEM_PROMPT);
    }

    /// 摘要后消息结构：第 0 条 = 带 <compaction-summary> 包裹的 user 消息，
    /// 其后逐条等于保留尾段（cut 起到末尾），顺序与内容零改动。
    #[test]
    fn compacted_messages_structure() {
        let messages = vec![
            text_msg(Role::User, "early"),
            text_msg(Role::Assistant, "mid"),
            text_msg(Role::User, "recent tail"),
        ];
        let out = compacted_messages(&messages, "the summary", 2);
        assert_eq!(out.len(), 2);
        match &out[0] {
            Msg { role: Role::User, content } => match &content[0] {
                Block::Text { text } => {
                    assert!(text.contains("compacted into the following summary"));
                    assert!(text.contains("<compaction-summary>\nthe summary\n</compaction-summary>"));
                }
                other => panic!("expected text block, got {other:?}"),
            },
            other => panic!("expected user msg, got {other:?}"),
        }
        // 尾段零改动：Block 未实现 PartialEq（llm 契约面不归本模块改），用 Debug
        // 串比对做全等断言（字段名+值逐字一致，等价于结构相等）。
        assert_eq!(format!("{:?}", out[1]), format!("{:?}", messages[2]));

        // cut=0：整段保留在摘要之后（顺序不变）。
        let full = compacted_messages(&messages, "s", 0);
        assert_eq!(full.len(), 4);
        assert_eq!(format!("{:?}", full[1]), format!("{:?}", messages[0]));
        assert_eq!(format!("{:?}", full[3]), format!("{:?}", messages[2]));
    }

    /// 触发线：`used > 上限 − 16_384`，边界值两侧各断一次；上限低于 RESERVE 时
    /// saturating 到 0（任何非零用量都判压缩）。
    #[test]
    fn should_compact_uses_reserve() {
        let c = SummaryCompactor { llm: mock_client(), ctx_limit: 176_000 };
        assert_eq!(176_000 - RESERVE_TOKENS, 159_616);
        assert!(!c.should_compact(159_616));
        assert!(c.should_compact(159_617));

        let small = SummaryCompactor { llm: mock_client(), ctx_limit: 8_192 };
        assert!(small.should_compact(1), "8k 上限 < RESERVE：差值饱和为 0，恒判压缩");
    }

    /// 触发线计量（计划测试项「usage 计量」的混合形）：provider usage 为主，
    /// 其后消息 chars/4 接账；无 usage / 基线越界退回全量估算。
    #[test]
    fn used_tokens_usage_first_then_estimate() {
        // 已覆盖 2 条（provider 报了 1000），其后一条 40 chars 文本 → 1000 + 10。
        let msgs = [
            text_msg(Role::User, "covered by usage"),
            text_msg(Role::Assistant, "covered too"),
            text_msg(Role::User, &"x".repeat(40)),
        ];
        assert_eq!(used_tokens(&msgs, Some((1_000, 2))), 1_010, "usage 为主 + 其后估算");

        // usage 为 0 也照用（某些端点 usage 缺省 0 是合法基线，不是「无 usage」）。
        assert_eq!(used_tokens(&msgs, Some((0, 3))), 0, "覆盖到末尾 = 其后无消息可估");

        // 无 usage（首请求前 / 压缩换血后基线作废）→ 全量估算。
        assert_eq!(used_tokens(&msgs, None), estimate_tokens(&msgs));
        // 基线越界（换血后旧 covered 比新史还长）→ 同样退回全量估算。
        assert_eq!(used_tokens(&msgs, Some((1_000, 99))), estimate_tokens(&msgs));
    }

    /// 测试用客户端桩：本文件的测试只走纯函数路径（cut_point/请求装配/结构装配），
    /// 不真调 `chat`——`LlmClient::Mock` 在此仅用于把 `SummaryCompactor` 拼成形。
    fn mock_client() -> Arc<LlmClient> {
        Arc::new(LlmClient::Mock(Arc::new(crate::llm::MockScript::default())))
    }
}
