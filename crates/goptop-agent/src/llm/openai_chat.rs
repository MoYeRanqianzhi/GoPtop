//! OpenAI Chat Completions 协议适配器（非流式 `POST {base}/chat/completions`）。
//!
//! 映射要点（计划「LLM 客户端」节 + 官方 wire 基线）：
//! - 系统提示词 = **messages[0] 的 `{role:"system"}`**；
//! - 鉴权头 `Authorization: Bearer`；
//! - 工具定义**嵌 function 壳**：`{type:"function", function:{name, description,
//!   parameters}}`（与 Responses 的扁平形态不同——两协议在这里最容易混）；
//! - assistant 侧工具调用 = 消息上的 `tool_calls` 数组，`function.arguments` 是
//!   **JSON 字符串**（统一形态里已 parse 成对象，发出时再序列化回字符串）；
//! - 工具结果 = **`{role:"tool"}` 独立消息**（`tool_call_id` 配对；OpenAI 没有
//!   is_error 表达，错误语义由文本本身传达）；
//! - 输出预算字段用 `max_tokens`：它是本协议生态（DeepSeek/Kimi/vLLM 等兼容
//!   端点）的公共分母；OpenAI 新推理模型的 `max_completion_tokens` 是单家特化，
//!   用户真需要时在装配层换字段即可，不适配器分叉。
//!
//! 响应侧：`choices[0].message` 的 content 为文本、tool_calls 归一成 [`ToolCall`]；
//! `finish_reason`：`tool_calls`→ToolUse、`length`→MaxTokens、`stop`→EndTurn、
//! 其余→Other；usage 取 `prompt_tokens` / `completion_tokens` /
//! `prompt_tokens_details.cached_tokens`（缺省 0）。

use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    Block, ChatRequest, ChatResponse, HttpChannel, LlmConfig, LlmError, Role, StopReason, ToolCall,
    Usage, classify_status, post_with_retry,
};

/// 端点基址拼尾路径（base_url 约定含版本段，如 `https://api.openai.com/v1`）。
const PATH: &str = "/chat/completions";

/// 一次非流式对话（[`super::LlmClient::OpenAiChat`] 的实现体）。
pub(crate) async fn chat(
    cfg: &LlmConfig,
    api_key: &str,
    http: &Arc<dyn HttpChannel>,
    req: ChatRequest,
) -> Result<ChatResponse, LlmError> {
    let url = format!("{}{PATH}", cfg.base_url.trim_end_matches('/'));
    let headers = vec![("Authorization".to_string(), format!("Bearer {api_key}"))];
    let body = build_body(cfg, &req).to_string();
    let (status, text) = post_with_retry(http.as_ref(), &url, &headers, &body).await?;
    if !(200..300).contains(&status) {
        return Err(classify_status(status, &text));
    }
    if cfg.stream {
        parse_stream_response(&text)
    } else {
        parse_response(&text)
    }
}

/// 流式聚合：Chat Completions 的 delta 帧重组出与非流式等价的响应。
/// 帧型：choices[0].delta（content / reasoning_content / tool_calls[index] 的
/// id+function 增量）、finish_reason、usage（末帧，include_usage 已开）。
fn parse_stream_response(text: &str) -> Result<ChatResponse, LlmError> {
    let mut content = String::new();
    let mut thinking_text = String::new();
    let mut stop = StopReason::Other;
    let mut usage = Usage::default();
    // tool_calls 按 delta 的 index 聚合（id/name 首帧给齐，arguments 逐段拼）。
    let mut calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments 串)
    for ev in super::sse_data_events(text) {
        if let Some(u) = ev.get("usage").cloned().filter(|u| !u.is_null()) {
            usage = Usage {
                input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                output_tokens: u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            };
        }
        let Some(choice) = ev.pointer("/choices/0") else { continue };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            stop = match reason {
                "tool_calls" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "stop" => StopReason::EndTurn,
                _ => StopReason::Other,
            };
        }
        let Some(delta) = choice.get("delta") else { continue };
        if let Some(t) = delta.get("content").and_then(Value::as_str) {
            content.push_str(t);
        }
        if let Some(t) = delta.get("reasoning_content").and_then(Value::as_str) {
            thinking_text.push_str(t);
        }
        if let Some(tc) = delta.get("tool_calls").and_then(Value::as_array) {
            for c in tc {
                let idx = c.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while calls.len() <= idx {
                    calls.push((String::new(), String::new(), String::new()));
                }
                if let Some(id) = c.get("id").and_then(Value::as_str) {
                    calls[idx].0 = id.to_string();
                }
                if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                    calls[idx].1 = name.to_string();
                }
                if let Some(a) = c.pointer("/function/arguments").and_then(Value::as_str) {
                    calls[idx].2.push_str(a);
                }
            }
        }
    }
    let tool_calls = calls
        .into_iter()
        .map(|(id, name, arguments)| ToolCall {
            id,
            name,
            arguments: serde_json::from_str(&arguments).unwrap_or_else(|_| json!({})),
        })
        .collect();
    Ok(ChatResponse { content, thinking_text, thinking_blocks: Vec::new(), tool_calls, stop, usage })
}

/// 统一请求 → Chat Completions 请求体。
fn build_body(cfg: &LlmConfig, req: &ChatRequest) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty() {
        messages.push(json!({ "role": "system", "content": req.system }));
    }
    for msg in &req.messages {
        // ToolResult 先行：role:"tool" 独立消息，一块一条（OpenAI 的配对要求）。
        for b in &msg.content {
            if let Block::ToolResult { call_id, content, .. } = b {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content,
                }));
            }
        }
        let text: String = msg
            .content
            .iter()
            .filter_map(|b| match b {
                // 视觉矩阵（计划）：本协议工具结果/消息不含图像——出现即按占位文本
                // 走（registry 层已按 supports_image_result 替换，这里是防御兜底）。
                Block::Text { text } => Some(text.as_str()),
                Block::Image { .. } => Some(crate::vfs::syn_image_placeholder()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        match msg.role {
            Role::User => {
                if !text.is_empty() {
                    messages.push(json!({ "role": "user", "content": text }));
                }
            }
            Role::Assistant => {
                let tool_calls: Vec<Value> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolCall { call } => Some(json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                // arguments 线上是 JSON 字符串（统一形态的对象回序列化）。
                                "arguments": serde_json::to_string(&call.arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            },
                        })),
                        _ => None,
                    })
                    .collect();
                if text.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                let mut m = json!({ "role": "assistant" });
                // content 与 tool_calls 可并存；空文本按 null（OpenAI 规范形态）。
                m["content"] = if text.is_empty() { Value::Null } else { json!(text) };
                if !tool_calls.is_empty() {
                    m["tool_calls"] = json!(tool_calls);
                }
                messages.push(m);
            }
        }
    }
    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "max_tokens": req.max_output_tokens,
    });
    // effort → reasoning_effort（推理系模型认这个字段；打给不支持模型的 4xx 会
    // 走 Fatal 分类回给配置面——各家的能力差异在配置文档里交代，运行面不猜）。
    if let Some(level) = super::sanitize_effort(&cfg.effort) {
        body["reasoning_effort"] = json!(level);
    }
    // 流式（兼容性开关）：delta 帧在 parse_stream_response 聚合；usage 要单独
    // 开 stream_options 才随流下发。
    if cfg.stream {
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !req.tools.is_empty() {
        body["tools"] = json!(req.tools.iter()
            .map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                },
            }))
            .collect::<Vec<_>>());
    }
    body
}

/// Chat Completions 响应体 → 统一响应。
fn parse_response(text: &str) -> Result<ChatResponse, LlmError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| LlmError::Fatal(format!("OpenAI Chat 响应不是合法 JSON: {e}")))?;
    let choice = v
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| LlmError::Fatal(format!("OpenAI Chat 响应缺 choices: {}", trunc(text))))?;
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let content = message.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
    // 思维链展示（DeepSeek 系的 reasoning_content）：只进测试模式日志，
    // **不回放**——序列化端有意跳过 ThinkingRaw（该系不收回传的推理字段）。
    let thinking_text = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for c in calls {
            let function = c.get("function").cloned().unwrap_or(Value::Null);
            // arguments 线上是 JSON 字符串，这里 parse 成统一形态的对象；
            // 解析失败（截断/坏 JSON）按空对象兜——registry 的参数校验会回一条
            // 可自纠的错误，模型重发即可，循环不因此停。
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            tool_calls.push(ToolCall {
                id: c.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                name: function.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                arguments,
            });
        }
    }
    let stop = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("stop") => StopReason::EndTurn,
        _ => StopReason::Other,
    };
    let usage = v.get("usage").cloned().unwrap_or(Value::Null);
    Ok(ChatResponse {
        content,
        thinking_text,
        thinking_blocks: Vec::new(),
        tool_calls,
        stop,
        usage: Usage {
            input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_read_tokens: usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_write_tokens: 0,
        },
    })
}

fn trunc(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmClient, LlmConfig, Msg, NativeHttp, Protocol, ToolSpec, testing::Stub};
    use std::sync::Arc;

    async fn client(base_url: String) -> LlmClient {
        LlmClient::OpenAiChat {
            cfg: LlmConfig {
                protocol: Protocol::OpenAiChat,
                base_url,
                model: "chat-test".to_string(),
                max_output_tokens: 512,
                reply_lang: None,
                effort: None,
                debug: false,
                stream: false,
            },
            api_key: "k-openai".to_string(),
            http: Arc::new(NativeHttp::new().expect("http client")),
        }
    }

    fn sample_request() -> ChatRequest {
        ChatRequest {
            system: "You are a player.".to_string(),
            messages: vec![
                Msg {
                    role: Role::User,
                    content: vec![Block::Text { text: "your turn".to_string() }],
                },
                Msg {
                    role: Role::Assistant,
                    content: vec![
                        Block::Text { text: "thinking".to_string() },
                        Block::ToolCall {
                            call: ToolCall {
                                id: "call_9".to_string(),
                                name: "submit".to_string(),
                                arguments: json!({ "path": "/game/in/move" }),
                            },
                        },
                    ],
                },
                Msg {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        call_id: "call_9".to_string(),
                        content: "{\"ok\":true}".to_string(),
                        is_error: true,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "submit".to_string(),
                description: "commit staged".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }],
            max_output_tokens: 512,
        }
    }

    fn body_of(raw: &str) -> Value {
        let (_, after) = raw.split_once("\r\n\r\n").expect("请求头与体以空行分隔");
        serde_json::from_str(after).expect("请求体必须是合法 JSON")
    }

    /// 请求关键字段：messages[0]=system / role:"tool" 独立消息 / arguments 是
    /// JSON 字符串 / 工具定义嵌 function 壳 / max_tokens 字段 / Bearer 头。
    #[tokio::test]
    async fn request_shape() {
        let stub = Stub::start(vec![(
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":null},"finish_reason":"stop"}],"usage":{}}"#
                .to_string(),
        )]);
        client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        let raw = stub.last_request();
        assert!(raw.starts_with("POST /chat/completions HTTP/1.1"), "应 POST 到 /chat/completions：{raw}");
        // hyper 上线的头名一律小写（x-api-key 那类本就小写的断言因此没暴露这点）。
        assert!(
            raw.to_ascii_lowercase().contains("authorization: bearer k-openai"),
            "缺 Bearer 鉴权头：{raw}"
        );

        let body = body_of(&raw);
        assert_eq!(body["model"], "chat-test");
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["messages"][0]["role"], "system", "系统提示词是 messages[0]");
        assert_eq!(body["messages"][0]["content"], "You are a player.");

        let msgs = body["messages"].as_array().expect("messages").clone();
        assert_eq!(msgs.len(), 4, "user/assistant(user+tool_calls)/tool 共四条");
        // assistant：文本与 tool_calls 并存；arguments 是字符串。
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["content"], "thinking");
        assert_eq!(msgs[2]["tool_calls"][0]["type"], "function");
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_9");
        assert_eq!(
            msgs[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"/game/in/move"}"#,
            "arguments 必须是 JSON 字符串"
        );
        // 工具结果：独立 role:"tool" 消息。
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_9");
        assert_eq!(msgs[3]["content"], "{\"ok\":true}");

        // 工具定义嵌 function 壳。
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "submit");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    }

    /// 响应解析：content/tool_calls（arguments 字符串 parse）/finish_reason/usage。
    #[tokio::test]
    async fn response_parse_and_usage() {
        let stub = Stub::start(vec![(200, r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "I will stage a move.",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "write", "arguments": "{\"path\":\"/game/in/move\",\"content\":\"7,7\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 200, "completion_tokens": 40,
                      "prompt_tokens_details": {"cached_tokens": 128}}
        }"#.to_string())]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        assert_eq!(resp.content, "I will stage a move.");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].name, "write");
        assert_eq!(resp.tool_calls[0].arguments["content"], "7,7", "arguments 字符串应 parse 成对象");
        assert_eq!(resp.usage.input_tokens, 200, "prompt_tokens 归一");
        assert_eq!(resp.usage.output_tokens, 40, "completion_tokens 归一");
        assert_eq!(resp.usage.cache_read_tokens, 128, "prompt_tokens_details.cached_tokens 归一");
        assert_eq!(resp.usage.cache_write_tokens, 0, "本协议无 cache write");
    }

    /// finish_reason=length → MaxTokens；arguments 坏 JSON → 空对象兜底。
    #[tokio::test]
    async fn length_stop_and_bad_arguments() {
        let stub = Stub::start(vec![(
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":null,
                "tool_calls":[{"id":"c","type":"function","function":{"name":"write","arguments":"{\"path\":"}}]},
                "finish_reason":"length"}],"usage":{}}"#.to_string(),
        )]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");
        assert_eq!(resp.stop, StopReason::MaxTokens);
        assert_eq!(resp.tool_calls[0].arguments, json!({}), "半截 arguments parse 失败按空对象兜");
    }

    /// 错误分类：403 → Fatal；context_length_exceeded → ContextWindowExceeded。
    #[tokio::test]
    async fn error_classification() {
        let forbidden = Stub::start(vec![(403, r#"{"error":{"message":"insufficient quota","code":"insufficient_quota"}}"#.to_string())]);
        let err = client(forbidden.base_url.clone()).await.chat(sample_request()).await.expect_err("403 应 Fatal");
        assert!(matches!(&err, LlmError::Fatal(m) if m.contains("403")), "实际：{err:?}");

        let overflow = Stub::start(vec![(400, r#"{"error":{"message":"This model's maximum context length is 8192 tokens","code":"context_length_exceeded"}}"#.to_string())]);
        let err = client(overflow.base_url.clone()).await.chat(sample_request()).await.expect_err("超窗应 ContextWindowExceeded");
        assert!(
            matches!(&err, LlmError::ContextWindowExceeded { model_limit: Some(8_192) }),
            "错误体里的模型实限要被解析出：{err:?}"
        );
    }
}
