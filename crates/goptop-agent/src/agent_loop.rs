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
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::compact::{compacted_messages, used_tokens, Compactor, KEEP_RECENT_TOKENS, SummaryCompactor};
use crate::llm::{
    Block, ChatRequest, LlmClient, LlmConfig, LlmError, Msg, Role, StopReason, ToolCall, ToolSpec,
};
use crate::player::GameEvent;
use crate::registry::{self, SubagentRunner, ToolCtx, ToolError};

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
    /// 系统提示词全文（AgentHub 以 [`crate::prompt::build_system_prompt`] 现算后
    /// 传入）。循环只持有成品字符串：PromptCfg 的输入（名字/执色/棋种/语言）归
    /// 装配层管，循环为拼提示词再背一份配置只会造出两处真相。骨架契约缺口在
    /// 本文件内最小补足（见实现报告）。
    pub system: String,
}

/// 循环结局 + 统计。
#[derive(Debug)]
pub struct LoopOutcome {
    pub stop: LoopStop,
    pub stats: LoopStats,
}

/// 开局引导。对话从空史开始而 Anthropic 不收空 `messages`——第一条 user 消息
/// 必须有；内容保持一般性，局面事实一律让模型自己读 `/game/*` 文件，这里不背
/// 第二份配置（棋种/执色只活在系统提示词与快照里）。
const OPENING_MESSAGE: &str = "A new game session is starting. Read /index and /game/status \
first, greet your opponent via /game/in/chat (write + submit), and take your first action \
through the tools.";

/// TextOnly 提醒（英文，与系统提示词同语言）。只回文字不是行动：注入后继续循环，
/// 绝不终止、绝不自动认输。
const TEXT_ONLY_REMINDER: &str = "Your previous reply was text only — text is NOT an action, \
and the session does not end for it. Act through the tools: write to stage (e.g. \
/game/in/move or /game/in/chat) and then submit the path. You are never forced to move \
every turn, but a round only counts when it ends in a tool call.";

/// 停车轮询拍长（用户拍板 2026-10-08，Claude Code 机制）：等待期**零 LLM 调用**——
/// 轮到对手且无未消费事件时循环挂起，150ms 一拍（自泵 + 复核），新事件 / 轮到我们 /
/// 终局 / 用户停止任一变化即唤醒。没有它「等待」就是 text-only 空转烧调用
///（实测：模型为了「以工具调用收尾」反复写记忆分析文件，33 次调用后出错）。
const PARK_POLL_MS: u64 = 150;

/// 收尾指令（winner 出现后的最后一轮：给模型一次写告别语的机会，随后循环收场）。
fn farewell_instruction(winner: &str) -> String {
    format!(
        "The game has ended — winner: {winner}. Send a short, graceful farewell: write it to \
/game/in/chat and submit the path. This is your final action; the session closes after it."
    )
}

/// 跑完整局：阻塞到五退出条件之一。事件自动推送 / TextOnly 提醒 / 预算与压缩的
/// 语义见模块注。对话历史由循环自持（compact 后重排），从空对话开始——系统提示词
/// 由 [`crate::prompt::build_system_prompt`] 生成后作为 ChatRequest.system 常驻。
pub async fn run(deps: LoopDeps) -> LoopOutcome {
    let LoopDeps { llm, llm_cfg, cfg, tools, stop, system } = deps;
    let mut stats = LoopStats::default();
    // 对话历史由循环自持；assistant 工具调用与 tool_result 成对入史（compact 的
    // 切点纪律「绝不拆对」靠这个形状才可判定）。
    let mut history: Vec<Msg> = vec![Msg {
        role: Role::User,
        content: vec![Block::Text { text: OPENING_MESSAGE.to_string() }],
    }];
    // 连续 Transient 计数（≥3 → Fatal，骨架注的错误分类在循环侧的落点）。
    let mut transients: u32 = 0;
    // 超窗兜底只重试一次：ContextWindowExceeded → 强制压缩 → 同一轮重发。
    let mut emergency_compacted = false;
    // 生效上限（计划 compact 节）：初值=用户配置；超窗错误体解析出模型实限后取
    // min(用户上限, 模型实限)——此后触发线判定（含正常轮）都用生效上限。
    let mut ctx_limit = u64::from(cfg.ctx_limit);
    // 最近一次 provider usage 基线（触发线计量的主量，见 compact.rs 的
    // used_tokens）：(prompt tokens, 请求时的 history 长度)——其后入史的消息
    // 落回 chars/4 估算；压缩换血后基线作废（maybe_compact 重置）。
    let mut usage_mark: Option<(u64, usize)> = None;
    // 最近一次压缩产出的摘要（增量压缩的 previousSummary 来源；压缩后历史被换血，
    // 旧摘要不能再从 history 里反查，只能随身携带）。
    let mut prev_summary: Option<String> = None;
    // winner 已现、收尾轮已给——下一轮到 winner 判定即 GameOver。
    let mut ending = false;
    let tool_specs = build_tool_specs(&tools);

    loop {
        // 退出条件 3：用户中止。检查点在 LLM 调用前——点了停止就不再花钱。
        if stop.load(Ordering::Relaxed) {
            return LoopOutcome { stop: LoopStop::Stopped, stats };
        }
        // 退出条件 5：硬预算。
        if stats.llm_calls >= cfg.max_llm_calls {
            return LoopOutcome { stop: LoopStop::BudgetExhausted, stats };
        }

        // compact 挂点：触发线判定 + 摘要换血（compact.rs 的参数语义在循环侧落点）。
        if let Err(e) = maybe_compact(
            &mut history,
            &llm,
            ctx_limit,
            &mut stats,
            &mut prev_summary,
            &mut usage_mark,
            false,
        )
        .await
        {
            return LoopOutcome { stop: LoopStop::Fatal(e.message().to_string()), stats };
        }

        let sent_len = history.len();
        let req = ChatRequest {
            system: system.clone(),
            messages: history.clone(),
            tools: tool_specs.clone(),
            max_output_tokens: llm_cfg.max_output_tokens,
        };
        stats.llm_calls += 1;
        let resp = match llm.chat(req).await {
            Ok(r) => {
                transients = 0;
                // usage 基线：prompt 侧三个字段都是模型真实读过的上下文
                // （output 不计——它以 assistant 消息的身份重入史，由估算段接手）。
                usage_mark = Some((
                    r.usage.input_tokens + r.usage.cache_read_tokens + r.usage.cache_write_tokens,
                    sent_len,
                ));
                r
            }
            // 退出条件 4：Fatal（连接/配置损坏）。
            Err(LlmError::Fatal(m)) => {
                return LoopOutcome { stop: LoopStop::Fatal(m), stats };
            }
            Err(LlmError::ContextWindowExceeded { model_limit }) => {
                // 超窗兜底（计划 compact 节）：生效上限取 min(用户上限, 模型实限)
                // → 强制压缩 → 重试一次 → 再失败 Fatal。模型实限从错误体解析
                // （llm::parse_model_limit；解析不出=None，按用户上限照旧兜底）。
                // 强制压缩以「保留尾段=最近的合法切点」执行——换血幅度最大且不拆
                // 工具对；同窗重试一次仍超窗就 Fatal，不空转。
                if emergency_compacted {
                    return LoopOutcome {
                        stop: LoopStop::Fatal(
                            "context window exceeded even after emergency compaction".to_string(),
                        ),
                        stats,
                    };
                }
                if let Some(limit) = model_limit {
                    ctx_limit = ctx_limit.min(limit);
                }
                emergency_compacted = true;
                if let Err(e) = maybe_compact(
                    &mut history,
                    &llm,
                    ctx_limit,
                    &mut stats,
                    &mut prev_summary,
                    &mut usage_mark,
                    true,
                )
                .await
                {
                    return LoopOutcome { stop: LoopStop::Fatal(e.message().to_string()), stats };
                }
                continue;
            }
            Err(LlmError::Transient(m)) => {
                transients += 1;
                if transients >= 3 {
                    return LoopOutcome {
                        stop: LoopStop::Fatal(format!("LLM unavailable after retries: {m}")),
                        stats,
                    };
                }
                continue;
            }
        };
        stats.input_tokens += resp.usage.input_tokens;
        stats.output_tokens += resp.usage.output_tokens;

        // 测试模式（用户拍板）：思维链与输出全文进环（tool="thinking"/"say"）——
        // 诊断「Agent 疑似卡住」的唯一窗口；关着时不进（环里不留长文本）。
        if llm_cfg.debug {
            if !resp.thinking_text.is_empty() {
                ring_say(&tools, "thinking", &resp.thinking_text);
            }
            if !resp.content.is_empty() {
                ring_say(&tools, "say", &resp.content);
            }
        }

        // assistant 消息入史：文本与工具调用同一条（协议回放要求配对完整）；
        // thinking 原样块跟在后面（Anthropic/Responses 的回放强约束，见
        // Block::ThinkingRaw 的 doc）。
        let mut assistant_blocks: Vec<Block> = Vec::new();
        if !resp.content.is_empty() {
            assistant_blocks.push(Block::Text { text: resp.content.clone() });
        }
        for b in &resp.thinking_blocks {
            assistant_blocks.push(Block::ThinkingRaw { data: b.clone() });
        }
        for call in &resp.tool_calls {
            assistant_blocks.push(Block::ToolCall { call: call.clone() });
        }
        if !assistant_blocks.is_empty() {
            history.push(Msg { role: Role::Assistant, content: assistant_blocks });
        }

        // stopReason==length：半截调用的参数是截断的 JSON，执行必错——全部作废、
        // 以错误文本回填让模型重发（agent-loop.ts:474-500）。纯文本被截断不算
        // 「半截调用」，落到 TextOnly 提醒路径。
        if resp.stop == StopReason::MaxTokens && !resp.tool_calls.is_empty() {
            let blocks = resp
                .tool_calls
                .iter()
                .map(|c| Block::ToolResult {
                    call_id: c.id.clone(),
                    content: format!(
                        "Tool call \"{}\" was not executed: the response hit the output token \
limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                        c.name
                    ),
                    is_error: true,
                })
                .collect();
            history.push(Msg { role: Role::User, content: blocks });
            continue;
        }

        // 工具执行 → 回填（RespondToModel → isError 文本，循环继续；Fatal → 终止）。
        // 回执块与后面的注入文本并成**一条** user 消息：Anthropic 要求 user/assistant
        // 严格交替，且 tool_result 块必须先于文本（紧跟配对的 tool_use）。
        let mut blocks: Vec<Block> = Vec::new();
        let mut resigned = false;
        for call in &resp.tool_calls {
            let t0 = crate::time_compat::now_ms();
            // submit 的详情（暂存内容头）必须在 execute 前取——提交即清槽。
            let staged_head = registry::staged_head(&call.name, &call.arguments, &tools);
            let res = registry::execute(&call.name, &call.arguments, &tools).await;
            let ms = crate::time_compat::now_ms() - t0;
            if let Some(hook) = &tools.on_tool {
                // 全量记账（含失败与 submit）：事件流是唯一现场，失败的 submit
                // 红色换行展示报错——「模型 submit 记忆」这类误用必须可见。
                let head = registry::args_summary(&call.arguments);
                let detail = match &res {
                    Ok(_) => staged_head.as_deref(),
                    Err(ToolError::RespondToModel(m)) => Some(m.as_str()),
                    Err(ToolError::Fatal(m)) => Some(m.as_str()),
                };
                hook(&call.name, res.is_ok(), ms, &head, detail);
            }
            match res {
                Ok(v) => {
                    if is_resign_submit(call) {
                        resigned = true;
                    }
                    push_tool_result(&mut blocks, call, v, llm.supports_image_result());
                }
                Err(ToolError::RespondToModel(m)) => blocks.push(Block::ToolResult {
                    call_id: call.id.clone(),
                    content: m,
                    is_error: true,
                }),
                Err(ToolError::Fatal(m)) => {
                    return LoopOutcome { stop: LoopStop::Fatal(m), stats };
                }
            }
        }
        // 退出条件 2：resign 的 terminate（submit 成功即局终，无需再问模型）。
        if resigned {
            return LoopOutcome { stop: LoopStop::Resigned, stats };
        }

        // 事件自动推送：排空队列 + 读最新快照（winner/轮次/计分的判定源）。
        // 排空即上环（用户拍板：事件流也要在面板可见，否则不利于测试）——
        // 头=`事件`，详情=人话摘要（对手落子坐标/消息全文/请求/终局）。
        let mut events = drain_logged(&tools);
        let mut snap = tools.player.snapshot();

        // —— 停车（用户拍板 2026-10-08，Claude Code 机制）：等待期零 LLM 调用。——
        // 轮到对手且无未消费事件 → 挂起：150ms 一拍（自泵推进 transport 状态、
        // 复核谓词），新事件 / 轮到我们 / 终局任一出现即唤醒继续。
        // 「模型为凑工具调用而在等待期写记忆分析文件」的空转烧钱路径就此消失。
        // **暂存未提交不 park**：write 了 in/ 槽是一段写了一半的行动（两段式
        // write→submit 中间不许停），挂起会让 submit 迟到下一件外部事件。
        if events.is_empty() && !is_my_turn(&snap) && !ending && !tools.staging.any_staged() {
            loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                tools.player.pump();
                snap = tools.player.snapshot();
                events = drain_logged(&tools);
                let winner = snap.get("winner").and_then(Value::as_str).is_some();
                if !events.is_empty() || is_my_turn(&snap) || winner {
                    break;
                }
                crate::time_compat::delay(PARK_POLL_MS).await;
            }
        }

        // 退出条件 1：winner。先给一轮收尾（写告别语 + submit），随后收场。
        if let Some(winner) = snap.get("winner").and_then(Value::as_str) {
            if ending {
                return LoopOutcome { stop: LoopStop::GameOver, stats };
            }
            ending = true;
            let mut prose = String::new();
            if !events.is_empty() {
                prose.push_str(&event_block(&events));
            }
            prose.push_str(&farewell_instruction(winner));
            blocks.push(Block::Text { text: prose });
            history.push(Msg { role: Role::User, content: blocks });
            continue;
        }

        // 注入文本：事件块、TextOnly 提醒、计分自动确认回执——并入本条 user 消息。
        // （停车唤醒后 events 非空或轮到我们；stop 停车唤醒的空事件轮会被顶部
        // stop 检查收场，不会走到这里空烧。）
        let mut prose = String::new();
        if !events.is_empty() {
            prose.push_str(&event_block(&events));
        }
        if resp.tool_calls.is_empty() {
            // TextOnly ≠ 行动：提醒继续，绝不终止、绝不自动认输。
            prose.push_str(TEXT_ONLY_REMINDER);
        }
        // 围棋计分 v1 取舍（计划 AgentPage 节）：Agent 不标死子（工具集不含），
        // 循环见 scoring 自动 ConfirmScore，死子由人类单方标记。
        if needs_score_confirm(&snap) {
            match auto_confirm_score(&tools).await {
                Ok(receipt) => prose.push_str(&format!(
                    "\nScoring started — auto-confirmed on your behalf (dead stones are marked \
by the human player). Receipt: {receipt}\n"
                )),
                Err(ToolError::RespondToModel(m)) => {
                    prose.push_str(&format!("\nScoring started — auto-confirm failed: {m}\n"))
                }
                Err(ToolError::Fatal(m)) => {
                    return LoopOutcome { stop: LoopStop::Fatal(m), stats };
                }
            }
        }
        if !prose.is_empty() {
            blocks.push(Block::Text { text: prose });
        }
        // blocks 恒非空：有工具调用必有回执块；无工具调用必有 TextOnly 提醒。
        // 防空守卫挡的是「空 user 消息上线」这类协议级坏请求，不是死代码。
        if !blocks.is_empty() {
            history.push(Msg { role: Role::User, content: blocks });
        }
    }
}

/// 工具面 → 协议工具定义（清单过滤与 delegate 开关的判定都在 [`crate::tools`]）。
fn build_tool_specs(ctx: &ToolCtx) -> Vec<ToolSpec> {
    crate::tools::tools_for(ctx.driver, ctx.subagent_enabled)
        .into_iter()
        .map(|def| ToolSpec {
            name: def.name.to_owned(),
            description: def.description.to_owned(),
            input_schema: serde_json::from_str(def.input_schema)
                .expect("ToolDef.input_schema 是编译期常量，坏 JSON 属于骨架 bug，立即炸出"),
        })
        .collect()
}

/// 这次工具调用是不是「submit /game/in/resign」——resign 的 terminate 由调用面
/// 直接判（registry 的回执没有 terminate 字段，等快照 winner 会多绕一轮）。
///
/// 路径做与 registry 判别表同款的宽容归一（trim + 补前导 `/`）：模型写
/// `"game/in/resign"` 在 registry 照样执行成功，这里若按字面比对就漏判 terminate，
/// 局已认输而循环还在空转。
fn is_resign_submit(call: &ToolCall) -> bool {
    if call.name != crate::tools::TOOL_SUBMIT.name {
        return false;
    }
    let path = call.arguments.get("path").and_then(Value::as_str).map(str::trim).unwrap_or("");
    let path = path.strip_prefix('/').unwrap_or(path);
    path == crate::vfs::InFile::Resign.path().trim_start_matches('/')
}

/// 是否该由循环自动确认计分：进入计分态、结果未出、我这席还没确认。
/// （快照缺字段按「无需确认」处理——判定只在真实快照契约上有意义。）
fn needs_score_confirm(snap: &Value) -> bool {
    snap.get("scoring").and_then(Value::as_bool) == Some(true)
        && snap.get("myScoreOk").and_then(Value::as_bool) == Some(false)
        && snap.get("scoreResult").is_none_or(Value::is_null)
}

/// 轮到我否（心跳的判定条件之一：队列空且**不**轮到我才等）。
fn is_my_turn(snap: &Value) -> bool {
    let to_move = snap.get("toMove").and_then(Value::as_str);
    let my_color = snap.get("myColor").and_then(Value::as_str);
    to_move.is_some() && to_move == my_color
}

/// 排空的事件 → `<event>` 块文本（每行一条 JSONL，与 /game/events 文件同行形）。
fn event_block(events: &[GameEvent]) -> String {
    let mut s = String::from("Game events since your last turn:\n");
    for ev in events {
        let line = serde_json::to_string(ev).expect("GameEvent 派生 Serialize，不可失败");
        s.push_str(&format!("<event>{line}</event>\n"));
    }
    s
}

/// 测试模式的思维链/输出进环（tool="thinking"/"say"；UI 渲染为非操作行）。
/// 钩子没接（测试台）就静默——展示面缺失不该影响对局。
fn ring_say(ctx: &ToolCtx, tool: &str, text: &str) {
    if let Some(hook) = &ctx.on_tool {
        hook(tool, true, 0, "", Some(text));
    }
}

/// 排空事件队列并把人话摘要写进日志环（tool="event"，head=事件，detail=摘要）。
/// 面板可见性是测试需求（用户拍板 2026-10-08）；钩子未接时只排空不上环。
fn drain_logged(ctx: &ToolCtx) -> Vec<crate::player::GameEvent> {
    let events = ctx.events.drain();
    if let Some(hook) = &ctx.on_tool {
        for ev in &events {
            hook("event", true, 0, "事件", Some(&ev.summary()));
        }
    }
    events
}

/// 计分自动确认：走 write 暂存 + submit，与模型动作完全同一条路，不绕过 registry
///（回执/错误文案、格式校验、settle 节奏全部复用工具面的那份真相）。
async fn auto_confirm_score(ctx: &ToolCtx) -> Result<Value, ToolError> {    let path = crate::vfs::InFile::Score.path();
    let stage = serde_json::json!({ "path": path, "content": "ok" });
    let _staged =
        registry::execute(crate::tools::TOOL_WRITE.name, &stage, ctx).await?;
    let commit = serde_json::json!({ "path": path });
    registry::execute(crate::tools::TOOL_SUBMIT.name, &commit, ctx).await
}

/// compact 挂点 —— 触发线判定与摘要换血（契约见 [`crate::compact`]）。
///
/// 流程：`used_tokens(history, usage_mark)` 过触发线（`used > ctx_limit −
/// RESERVE_TOKENS`；provider usage 为主 + 其后消息估算）→ `cut_point` 定保留尾段
/// （绝不拆 assistant 工具调用与 tool_result 对）→ `summarize`（独立一次 LLM
/// 请求，不计入 `max_llm_calls`——压缩是维护动作不是模型的一次思考）→ history
/// 重排为 `[user: 摘要] + retainedTail`、计数 +1。`prev_summary` 随身携带：有旧
/// 摘要走增量更新（对手风格观察不换血）。
///
/// 非 force 且切点为 0 时放弃：切 0 =「摘要空集 + 全量保留」，多花一次摘要请求
/// 还让上下文更长，违背压缩本意（触发线此后每轮都过，但判定本身零成本）。
/// `force`（超窗兜底）无视收益必须真减量：保留尾段收到「最近的合法切点」
/// （keep=1 token），结构一定变小。压缩换血后 `usage_mark` 重置——旧基线的
/// 覆盖下标对新史作废，下一轮真实响应到账后重建。
async fn maybe_compact(
    history: &mut Vec<Msg>,
    llm: &Arc<LlmClient>,
    ctx_limit: u64,
    stats: &mut LoopStats,
    prev_summary: &mut Option<String>,
    usage_mark: &mut Option<(u64, usize)>,
    force: bool,
) -> Result<(), LlmError> {
    let compactor = SummaryCompactor { llm: Arc::clone(llm), ctx_limit };
    if !force && !compactor.should_compact(used_tokens(history, *usage_mark)) {
        return Ok(());
    }
    let cut = compactor.cut_point(history, if force { 1 } else { KEEP_RECENT_TOKENS });
    if cut == 0 && !force {
        return Ok(());
    }
    let summary = compactor.summarize(history, prev_summary.as_deref()).await?;
    *history = compacted_messages(history, &summary, cut);
    *prev_summary = Some(summary);
    *usage_mark = None;
    stats.compactions += 1;
    Ok(())
}

/// 工具回执 → 结果块（承载能力矩阵的循环侧落点，计划）：图像回执按协议能力
/// 分派——支持视觉（Anthropic 原生 image block；Mock 同形）：文本回执剥掉
/// base64（上下文只留元信息，PNG 体积不进 chars/4 估算），图像以 [`Block::Image`]
/// 随行；不支持（OpenAI 两协议工具结果纯字符串）：占位文本（权威文案）。
/// 其余回执原样成块。
fn push_tool_result(blocks: &mut Vec<Block>, call: &ToolCall, receipt: Value, supports_images: bool) {
    let call_id = call.id.clone();
    match registry::image_part(&receipt) {
        Some((mime, data)) if supports_images => {
            let mut slim = receipt;
            if let Some(obj) = slim.as_object_mut() {
                obj.remove("image");
                obj.insert("note".into(), Value::String("PNG attached as an image block".into()));
            }
            blocks.push(Block::ToolResult { call_id, content: slim.to_string(), is_error: false });
            blocks.push(Block::Image { mime, data_base64: data });
        }
        Some(_) => blocks.push(Block::ToolResult {
            call_id,
            content: crate::vfs::syn_image_placeholder().to_string(),
            is_error: false,
        }),
        None => {
            blocks.push(Block::ToolResult { call_id, content: receipt.to_string(), is_error: false });
        }
    }
}

/* ---------------- delegate 子代理：真实的「同 LLM 独立上下文小循环」 ---------------- */

/// 子代理的独立小预算默认值（计划「工具清单」delegate 行：独立小预算
/// maxLlmCalls）。独立于父循环的 [`DEFAULT_MAX_LLM_CALLS`]——深思是额外开销，
/// 小预算封顶；几次 read/grep 外加收束足够一次「候选点评估」。
pub const SUBAGENT_MAX_LLM_CALLS: u32 = 12;

/// 子代理单轮回复的输出预算（与主循环默认 maxOutputTokens 同量级；结论不需要长文）。
const SUBAGENT_MAX_OUTPUT_TOKENS: u32 = 1024;

/// 子代理的系统提示词：只读分析员——读得到整局文件、动不了任何东西，结论以
/// 纯文本收束（零工具调用的那一轮即最终结论）。
const SUBAGENT_SYSTEM: &str = "You are a read-only analysis subagent for an LLM board-game \
player. The virtual filesystem /game/* holds the live game (read /index first if needed). \
Investigate the given task with the `read` and `grep` tools, think, then reply with your \
final conclusion as PLAIN TEXT — that text is delivered to the parent agent as the \
delegate result. You cannot act: no moves, no chat, no writes, no delegation.";

/// 深度 1 的执行口白名单：子循环的 LLM 工具面只有这两个（清单里没有 delegate，
/// 结构上无从嵌套）；执行口再拦一道——模型越权点名 write/submit/delegate 等
/// 一律**不执行**、回错误文本让它改道（不依赖模型守约）。
const SUBAGENT_TOOLS: [&str; 2] = ["read", "grep"];

/// delegate 的真实执行体（计划「工具清单」delegate 行 + 测试计划「启用后 Mock
/// 子循环回结论、深度 1 不嵌套」）：**同 LLM**、**独立上下文**（从任务文本空史
/// 起跑，深思不污染主上下文）、**只读工具面**（read/grep）、**深度 1 不嵌套**、
/// **独立小预算**。装配层（AgentHub）在 `subagent_enabled` 时把本结构注入
/// [`ToolCtx::subagent`]；未注入时 delegate 回装配缺口说明（registry）。
pub struct SubagentLoop {
    /// 与父代理同一个 LLM 客户端（同协议同模型——「深思」是同脑另开一间房）。
    pub llm: Arc<LlmClient>,
    /// 与父代理共享的工具上下文——read/grep 看同一局棋、同一 /memory。
    pub tools: ToolCtx,
    /// 独立小预算（LLM 调用次数；打满即以 Err 回父代理）。
    pub max_llm_calls: u32,
}

impl SubagentLoop {
    /// 默认预算装配。
    #[must_use]
    pub fn new(llm: Arc<LlmClient>, tools: ToolCtx) -> Self {
        Self { llm, tools, max_llm_calls: SUBAGENT_MAX_LLM_CALLS }
    }

    /// 子循环的工具面：只有 read/grep 的协议定义（深度 1 的结构保证——清单里
    /// 没有 delegate，模型连嵌套入口都看不到）。
    fn tool_specs() -> Vec<ToolSpec> {
        SUBAGENT_TOOLS
            .iter()
            .map(|&name| crate::tools::by_name(name).expect("白名单里的工具必然在册"))
            .map(|def| ToolSpec {
                name: def.name.to_owned(),
                description: def.description.to_owned(),
                input_schema: serde_json::from_str(def.input_schema)
                    .expect("ToolDef.input_schema 是编译期常量，坏 JSON 属于骨架 bug，立即炸出"),
            })
            .collect()
    }
}

// ?Send 界的取舍见 llm/mod.rs HttpChannel 注（wasm 走 spawn_local，future 非 Send）。
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SubagentRunner for SubagentLoop {
    async fn run(&self, task: String) -> Result<String, String> {
        // 独立上下文：history 从任务文本起跑，与父代理的对话史零共享。
        let mut history = vec![Msg {
            role: Role::User,
            content: vec![Block::Text { text: task }],
        }];
        let tool_specs = Self::tool_specs();
        for _ in 0..self.max_llm_calls {
            let req = ChatRequest {
                system: SUBAGENT_SYSTEM.to_string(),
                messages: history.clone(),
                tools: tool_specs.clone(),
                max_output_tokens: SUBAGENT_MAX_OUTPUT_TOKENS,
            };
            let resp = self.llm.chat(req).await.map_err(|e| e.message().to_string())?;
            // 零工具调用 = 结论：最终文本原样回父代理。子代理没有「行动」语义
            // （read-only），不需要主循环那套 TextOnly 提醒的续命纪律。
            if resp.tool_calls.is_empty() {
                return Ok(if resp.content.is_empty() {
                    "(no conclusion text)".to_string()
                } else {
                    resp.content
                });
            }
            let mut assistant_blocks: Vec<Block> = Vec::new();
            if !resp.content.is_empty() {
                assistant_blocks.push(Block::Text { text: resp.content.clone() });
            }
            // 子循环同受回放强约束：thinking 原样块一并入史。
            for b in &resp.thinking_blocks {
                assistant_blocks.push(Block::ThinkingRaw { data: b.clone() });
            }
            for call in &resp.tool_calls {
                assistant_blocks.push(Block::ToolCall { call: call.clone() });
            }
            history.push(Msg { role: Role::Assistant, content: assistant_blocks });

            // 工具执行：白名单外拒绝执行（深度 1 的执行口），回执成对入史。
            let mut blocks: Vec<Block> = Vec::new();
            for call in &resp.tool_calls {
                if !SUBAGENT_TOOLS.contains(&call.name.as_str()) {
                    blocks.push(Block::ToolResult {
                        call_id: call.id.clone(),
                        content: format!(
                            "tool \"{}\" is not available to the subagent (read/grep only — \
                             depth 1, no actions, no nesting).",
                            call.name
                        ),
                        is_error: true,
                    });
                    continue;
                }
                match registry::execute(&call.name, &call.arguments, &self.tools).await {
                    Ok(v) => blocks.push(Block::ToolResult {
                        call_id: call.id.clone(),
                        content: v.to_string(),
                        is_error: false,
                    }),
                    Err(ToolError::RespondToModel(m)) => blocks.push(Block::ToolResult {
                        call_id: call.id.clone(),
                        content: m,
                        is_error: true,
                    }),
                    // 子代理视角的环境损坏按失败文本回父代理——父代理该知道原因
                    // 并继续（registry 的 delegate 契约），不是把整局打成 Fatal。
                    Err(ToolError::Fatal(m)) => return Err(m),
                }
            }
            history.push(Msg { role: Role::User, content: blocks });
        }
        Err(format!(
            "subagent budget exhausted after {max} LLM calls without a final conclusion",
            max = self.max_llm_calls
        ))
    }
}
