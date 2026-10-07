//! LLM 客户端 —— 三协议（Anthropic Messages / OpenAI Responses / OpenAI Chat
//! Completions）统一在一个 enum 后面（**非流式 v1**；Mock 供测试与无 key 演示）。
//!
//! **为什么 enum 免 trait 分发**：三个适配器的方法面完全同形（`chat(req) -> resp`），
//! enum + match 的分发比 trait object 少一层 indirection，且新增协议就是加变体
//! 加分支、编译器逼着补全所有 match——trait 的新实现则可以悄悄漏掉行为约定。
//!
//! 统一类型（[`ChatRequest`]/[`ChatResponse`]/[`Msg`]/[`Block`]/[`ToolCall`]）是
//! 循环与压缩的唯一工作形态；三协议的线格式差异全部关在适配器里（映射要点见
//! 计划「LLM 客户端」节：Anthropic 顶层 system / tool_result 是 user 消息 /
//! max_tokens 必填；OpenAI Chat 的 arguments 是 **JSON 字符串**需 parse、结果=
//! `{role:"tool"}` 独立消息；OpenAI Responses 顶层 instructions / 工具定义不嵌
//! function 壳 / v1 `store:false` 全量重传）。
//!
//! 错误分类：401/403/模型不存在 → [`LlmError::Fatal`]；错误特征
//! `context_length_exceeded` → [`LlmError::ContextWindowExceeded`]（紧急压缩重试）；
//! 429/5xx → 适配器内退避重试 2 次后回 [`LlmError::Transient`]（连续 3 轮失败→
//! error 态，判定在循环里）。HTTP 通道走 [`HttpChannel`] trait：native=reqwest；
//! web=TS fetch 钩子（`window.goptopAgentHttp`，阶段⑤）——循环体与通道解耦。

use std::sync::Arc;

use async_trait::async_trait;

/// 协议种别（store 键 `goptop:llm-config.protocol` 的值域；snake_case 线上形态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Anthropic Messages（`/v1/messages`）。
    Anthropic,
    /// OpenAI Responses（`/v1/responses`）。
    OpenAiResponses,
    /// OpenAI Chat Completions（`/v1/chat/completions`）。
    OpenAiChat,
}

/// LLM 连接配置（store 链 `goptop:llm-config`）。
///
/// **key 不在这里**：单独存 `goptop:llm-key`，明文（store.json 无加密，风险如实
/// 注明，计划 R1）——分开存是为了「测试连接」「改配置不重输 key」两条 UI 路径
/// 不用整包搬密钥。
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub protocol: Protocol,
    /// 端点基址（含版本段，如 `https://api.anthropic.com/v1`——适配器只拼尾路径）。
    pub base_url: String,
    /// 模型名（如 `claude-sonnet-4-5`）。
    pub model: String,
    /// 单次回复的输出预算（默认 1024）。
    pub max_output_tokens: u32,
    /// 回复语言：**任意字符串**原样注入系统提示词的 `# Language` 段
    /// （"简体中文"/"English"/"喵语"……不校验）；`None`=跟随 UI 语言
    /// （agent_start cfg 由前端传入当前 UI 语言作默认）。
    pub reply_lang: Option<String>,
}

/// 工具面在请求里的形态（由 tools.rs 的 `ToolDef` 装配而来）。
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// 手写 JSON Schema（反序列化成对象——三协议都要内嵌进各自请求体）。
    pub input_schema: serde_json::Value,
}

/// 消息角色（统一形态只有两个：三协议的 system 顶层单独传，不进消息数组）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// 内容块 —— 三协议内容形态的并集。
#[derive(Clone, Debug)]
pub enum Block {
    /// 纯文本。
    Text { text: String },
    /// 工具结果回填（is_error=true 时适配器按各协议的错误形态装——模型看到的是
    /// 错误文本，循环不因此停）。content 统一为字符串：OpenAI 两协议本就只收
    /// 字符串，Anthropic 侧适配器负责包成 text block。
    ToolResult { call_id: String, content: String, is_error: bool },
    /// 图像（base64）。仅 Anthropic 工具结果原生支持；OpenAI 两协议 → 占位符文本
    ///（承载能力矩阵见计划；判定走 [`LlmClient::supports_image_result`]）。
    Image { mime: String, data_base64: String },
}

/// 一条统一消息。
#[derive(Clone, Debug)]
pub struct Msg {
    pub role: Role,
    pub content: Vec<Block>,
}

/// 一次工具调用（模型发起）：三协议的 id/name/arguments 归一。
/// `arguments` 统一为**已解析的对象**——OpenAI Chat 线上的 JSON 字符串由适配器 parse。
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// usage 归一（三协议字段名不同、语义对齐：cache 只 Anthropic 有，其余填 0）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    /// 本轮合计消耗（token 用量上报与 compact 计量的主来源）。
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// 停止原因归一。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// 模型自然收束（无工具调用）。
    EndTurn,
    /// 模型要调工具（循环继续的主通道）。
    ToolUse,
    /// 输出被 max_output_tokens 截断——**本轮工具调用作废回错误**
    ///（照 agent-loop.ts:474-500：半截的调用执行了必出错）。
    MaxTokens,
    /// 其余（stop_sequence 等；循环按 EndTurn 同类处理，不特判）。
    Other,
}

/// 一次统一对话请求。
#[derive(Clone, Debug)]
pub struct ChatRequest {
    /// 系统提示词（三协议各自安放：Anthropic 顶层 system、OpenAI Chat 的
    /// messages[0]、Responses 的 instructions）。
    pub system: String,
    pub messages: Vec<Msg>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
}

/// 一次统一对话响应。
///
/// `content` 是**文本部分的拼接**（多 text block 按序拼接）——工具调用与图像
/// 走独立字段，循环的大多数分支只关心「有没有字、有没有调用」两个事实。
#[derive(Clone, Debug)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop: StopReason,
    pub usage: Usage,
}

/// LLM 错误三型（见模块注的错误分类）。
#[derive(Debug, Clone)]
pub enum LlmError {
    /// 401/403/模型不存在/连接配置坏——循环终止进 error 态。
    Fatal(String),
    /// 错误特征 context_length_exceeded——紧急压缩（生效上限取 min(用户上限,
    /// 模型实限)）后重试一次，再失败才 Fatal。
    ContextWindowExceeded,
    /// 429/5xx 退避重试 2 次后仍失败——循环里连续 3 轮 Transient → error 态。
    Transient(String),
}

impl LlmError {
    /// 错误文本（状态上报共用）。
    #[must_use]
    pub fn message(&self) -> &str {
        todo!()
    }
}

/// HTTP POST 通道抽象 —— native=reqwest；web=TS fetch 钩子（阶段⑤）。
///
/// **通道不解释业务**：非 2xx 也原样带回状态码+体，协议适配器据状态码与错误体
/// 分类成 [`LlmError`]——分类规则（401→Fatal 等）只写一遍，且能对着真实错误体
/// 写测试。头由调用方给（各协议的鉴权头不同：x-api-key / Bearer）。
#[async_trait]
pub trait HttpChannel: Send + Sync {
    /// POST JSON，回 `(状态码, 响应体文本)`。传输层故障（连接失败/超时）回 Err。
    ///
    /// # Errors
    /// 网络故障/超时（60s 超时约定在实现里）。
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String), String>;
}

/// Mock 剧本：按序弹出预置响应。
///
/// 弹尽即 Fatal（`mock script exhausted`）——测试剧本少写一步是测试 bug，
/// 静默循环会把它藏成「偶发卡死」。`Mutex` 而非 `&mut`：剧本被 enum 内持有，
/// `chat(&self)` 的借用面要求内部可变性。
#[derive(Default)]
pub struct MockScript(std::sync::Mutex<Vec<ChatResponse>>);

impl MockScript {
    #[must_use]
    pub fn new(script: Vec<ChatResponse>) -> Self {
        Self(std::sync::Mutex::new(script))
    }

    /// 剩余步数（测试断言「剧本恰好用完」用——用不尽说明循环提前退出或漏调）。
    #[must_use]
    pub fn remaining(&self) -> usize {
        todo!()
    }
}

/// 统一客户端。三个真协议变体的字段就是适配器要的全部（无全局状态——
/// 连接池生命周期归 HttpChannel 实现）。
pub enum LlmClient {
    /// 确定性剧本桩：无头测试与无 key 演示。
    Mock(Arc<MockScript>),
    /// Anthropic Messages。
    Anthropic { cfg: LlmConfig, api_key: String, http: Arc<dyn HttpChannel> },
    /// OpenAI Responses（v1 `store:false` 全量重传）。
    OpenAiResponses { cfg: LlmConfig, api_key: String, http: Arc<dyn HttpChannel> },
    /// OpenAI Chat Completions。
    OpenAiChat { cfg: LlmConfig, api_key: String, http: Arc<dyn HttpChannel> },
}

impl LlmClient {
    /// 一次非流式对话。超时 60s；429/5xx 退避重试 2 次（适配器内做，调用方只见
    /// 最终结果）。
    ///
    /// # Errors
    /// [`LlmError`] 三型（分类规则见模块注）。
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        todo!()
    }

    /// 协议是否支持工具结果带图（Anthropic ✓ / OpenAI 两协议 ✗）。
    /// registry 据此决定回图像块还是 [`crate::vfs::syn_image_placeholder`]。
    #[must_use]
    pub fn supports_image_result(&self) -> bool {
        todo!()
    }
}
