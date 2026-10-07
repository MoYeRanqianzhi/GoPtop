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
//! error 态，判定在循环里）。HTTP 通道走 [`HttpChannel`] trait：native=reqwest
//! （[`NativeHttp`]）；web=TS fetch 钩子（`window.goptopAgentHttp`，阶段⑤）——
//! 循环体与通道解耦。
//!
//! 子模块：[`mock`]（剧本桩）/ [`anthropic`] / [`openai_chat`] / [`openai_responses`]
//! （三协议适配器）/ [`native_http`](reqwest 通道)——全部私有，外界只经
//! [`LlmClient`] 与 [`NativeHttp`] 进出。

mod anthropic;
mod mock;
mod native_http;
mod openai_chat;
mod openai_responses;

use std::sync::Arc;

use async_trait::async_trait;

pub use native_http::NativeHttp;

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
    /// 模型侧工具调用（assistant 消息里的发起）。**必须有这个变体**：三协议都要求
    /// 工具结果与发起调用在线上配对出现（Anthropic 的 tool_result 无配对 tool_use
    /// 直接 4xx；OpenAI Chat 的 role:"tool" 必须挂在前面的 tool_calls 下；Responses
    /// 的 function_call_output 配 function_call 的 call_id）——统一形态表达不了
    /// 「assistant 发起了调用」，回放历史就是坏请求。arguments 已是解析后的对象。
    ToolCall { call: ToolCall },
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
        match self {
            LlmError::Fatal(m) | LlmError::Transient(m) => m,
            // 无载荷变体：文本只用于状态展示，措辞与 compact 路径的判定字段对齐。
            LlmError::ContextWindowExceeded => "context window exceeded",
        }
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
        self.0.lock().expect("锁中毒即 bug（与 EventQueue 同一语义）").len()
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
        match self {
            LlmClient::Mock(script) => mock::chat(script, req).await,
            LlmClient::Anthropic { cfg, api_key, http } => {
                anthropic::chat(cfg, api_key, http, req).await
            }
            LlmClient::OpenAiResponses { cfg, api_key, http } => {
                openai_responses::chat(cfg, api_key, http, req).await
            }
            LlmClient::OpenAiChat { cfg, api_key, http } => {
                openai_chat::chat(cfg, api_key, http, req).await
            }
        }
    }

    /// 协议是否支持工具结果带图（Anthropic ✓ / OpenAI 两协议 ✗）。
    /// registry 据此决定回图像块还是 [`crate::vfs::syn_image_placeholder`]。
    /// Mock 按 Anthropic 同形取 true——剧本桩走最富路径，演示与图像流测试才有意义。
    #[must_use]
    pub fn supports_image_result(&self) -> bool {
        matches!(self, LlmClient::Mock(_) | LlmClient::Anthropic { .. })
    }
}

/* ---------------- 适配器共享件（退避/分类/截断） ---------------- */

/// 429/5xx（与传输层故障）的退避间隔：首次失败后重试 2 次（共 3 次尝试）。
/// 计划只拍「退避 2 次」不给间隔——1s/2s 是对限流窗口的温和让步，再长会把
/// 一局的等待体验拖垮（连续 3 轮 Transient 的 error 判定在循环侧）。
const RETRY_BACKOFF_MS: [u64; 2] = [1_000, 2_000];

/// POST + 429/5xx/传输故障的退避重试。非重试类状态码原样上抛给
/// [`classify_status`] 分类；重试耗尽才回 [`LlmError::Transient`]。
pub(crate) async fn post_with_retry(
    http: &dyn HttpChannel,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<(u16, String), LlmError> {
    for attempt in 0..=RETRY_BACKOFF_MS.len() {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(RETRY_BACKOFF_MS[attempt - 1])).await;
        }
        match http.post_json(url, headers, body.to_string()).await {
            // 重试类：没耗尽就退避再来，耗尽回 Transient。
            Ok((status, text)) if status == 429 || status >= 500 => {
                if attempt < RETRY_BACKOFF_MS.len() {
                    continue;
                }
                return Err(LlmError::Transient(format!("HTTP {status}: {}", excerpt(&text))));
            }
            Ok(result) => return Ok(result),
            Err(transport) => {
                if attempt < RETRY_BACKOFF_MS.len() {
                    continue;
                }
                return Err(LlmError::Transient(transport));
            }
        }
    }
    unreachable!("循环每个分支要么 continue 要么 return，走不到循环外")
}

/// 非重试类错误的分类（429/5xx 已在退避层消化）：
/// 400 且错误体带超窗特征 → [`LlmError::ContextWindowExceeded`]（compact 路径入口）；
/// 其余（含 401/403/404——key 无效、模型或路径不存在）→ [`LlmError::Fatal`]：
/// 配置或请求形态损坏，静默重试只会烧钱不解决问题。
pub(crate) fn classify_status(status: u16, body: &str) -> LlmError {
    if status == 400 && is_context_overflow(body) {
        return LlmError::ContextWindowExceeded;
    }
    LlmError::Fatal(format!("HTTP {status}: {}", excerpt(body)))
}

/// 超窗错误体特征（三协议混一清单，各家文案见 pi overflow.ts 的盘点）：
/// OpenAI 两协议的 error code `context_length_exceeded` / "exceeds the context
/// window" / "maximum context length"；Anthropic 的 "prompt is too long: X > Y"。
pub(crate) fn is_context_overflow(body: &str) -> bool {
    const MARKERS: [&str; 4] = [
        "context_length_exceeded",
        "prompt is too long",
        "exceeds the context window",
        "maximum context length",
    ];
    MARKERS.iter().any(|m| body.contains(m))
}

/// 错误体截断（进 agent_status.error 的文本；错误体可能是整页 HTML）。
fn excerpt(body: &str) -> String {
    body.chars().take(400).collect()
}

/* ---------------- 测试件：本地 std::TcpListener 裸 HTTP 桩 ---------------- */

#[cfg(test)]
pub(crate) mod testing {
    //! 三协议适配器测试的裸 HTTP 桩：真 TCP 回环收请求、按预置清单逐连接回响应、
    //! 捕获请求原文供断言。**不用任何 mock HTTP 库**——请求关键字段的断言对象就是
    //! 适配器真正发上线的字节。

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// 一只已启动的桩：`responses` 按**连接**次序回放（重试会开新连接），
    /// 每次请求的原文追加进 `requests`。
    pub struct Stub {
        pub base_url: String,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    impl Stub {
        /// 启动桩并回吐基地址（适配器拿它当 `LlmConfig.base_url`）。
        ///
        /// # Panics
        /// 端口绑定失败（回环端口耗尽，属环境故障）。
        pub fn start(responses: Vec<(u16, String)>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 127.0.0.1:0");
            let port = listener.local_addr().expect("local_addr").port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            std::thread::spawn(move || {
                for (status, body) in responses {
                    let (mut stream, _) = match listener.accept() {
                        Ok(x) => x,
                        // 桩的生命周期归测试：测试侧不再来请求时静默收摊。
                        Err(_) => return,
                    };
                    let raw = read_request(&mut stream);
                    captured.lock().expect("锁中毒即 bug").push(raw);
                    let resp = format!(
                        "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
            });
            Self { base_url: format!("http://127.0.0.1:{port}"), requests }
        }

        /// 最后一次请求原文（断言路径/头/体用）。
        ///
        /// # Panics
        /// 还没有请求进来（测试序错误）。
        pub fn last_request(&self) -> String {
            self.requests
                .lock()
                .expect("锁中毒即 bug")
                .last()
                .expect("桩至少应收到一次请求")
                .clone()
        }
    }

    /// 读完整 HTTP 请求：读到头结束标记，再按 Content-Length 补齐请求体。
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let n = stream.read(&mut chunk).expect("read 请求头");
            if n == 0 {
                return String::from_utf8_lossy(&buf).into_owned();
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_head_end(&buf) {
                break pos;
            }
        };
        let content_length = String::from_utf8_lossy(&buf[..head_end])
            .split("\r\n")
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().to_string())
            })
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let want = head_end + 4 + content_length;
        while buf.len() < want {
            let n = stream.read(&mut chunk).expect("read 请求体");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn find_head_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }
}
