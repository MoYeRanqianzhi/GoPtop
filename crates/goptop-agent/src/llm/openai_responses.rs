//! OpenAI Responses 协议适配器（非流式 `POST {base}/responses`）。
//!
//! 映射要点（计划「LLM 客户端」节 + 官方 wire 基线）：
//! - 系统提示词放**顶层 `instructions`**（不进 input）；
//! - 鉴权头 `Authorization: Bearer`；
//! - 工具定义**不嵌 function 壳**（扁平 `{type:"function", name, description,
//!   parameters}`——与 Chat Completions 相反，两协议在这里最容易混）；
//! - input 是**条目流**：assistant 文本=`message` 条目（output_text）、工具调用=
//!   `function_call` 条目（`call_id` 配对、arguments 是 JSON 字符串）、工具结果=
//!   `function_call_output` 条目（`output` 是字符串）；
//! - v1 约定 **`store:false`**：服务端不留状态，每轮**全量重传** input——与循环
//!   「对话历史自持」的设计互为前提（无 response id 续链的复杂度）。
//!
//! 响应侧：`output` 数组里 message 条目的 output_text 拼接为文本、function_call
//! 条目归一成 [`ToolCall`]（id 取 `call_id`——回传 function_call_output 时配对的
//! 就是它）；停止判定：`status:"incomplete"` 且 `incomplete_details.reason ==
//! "max_output_tokens"`→MaxTokens（优先，半截调用循环会作废）、有 function_call
//! →ToolUse、`status:"completed"`→EndTurn；`status:"failed"`→Transient。
//! usage 取 `input_tokens` / `output_tokens` / `input_tokens_details.cached_tokens`。

use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    Block, ChatRequest, ChatResponse, HttpChannel, LlmConfig, LlmError, Role, StopReason, ToolCall,
    Usage, classify_status, post_with_retry,
};

/// 端点基址拼尾路径（base_url 约定含版本段，如 `https://api.openai.com/v1`）。
const PATH: &str = "/responses";

/// 一次非流式对话（[`super::LlmClient::OpenAiResponses`] 的实现体）。
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
        // 流式：等 response.completed 帧——它携带**完整响应对象**，直接复用
        // 非流式解析（增量帧不需要逐个拼）。
        for ev in super::sse_data_events(&text) {
            if ev.get("type").and_then(Value::as_str) == Some("response.completed") {
                let full = ev.get("response").cloned().unwrap_or(Value::Null);
                return parse_response(&full.to_string());
            }
        }
        return Err(LlmError::Transient(
            "responses stream ended without response.completed".into(),
        ));
    }
    parse_response(&text)
}

/// 统一请求 → Responses 请求体。
fn build_body(cfg: &LlmConfig, req: &ChatRequest) -> Value {
    let mut input = Vec::with_capacity(req.messages.len());
    for msg in &req.messages {
        // 每条统一消息先吐 function_call_output（与前面的 function_call 配对），
        // 再吐文本条目——与循环的块顺序一致，协议侧的条目流不要求严格交替。
        let mut text = String::new();
        for b in &msg.content {
            match b {
                Block::ToolResult { call_id, content, .. } => input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": content,
                })),
                Block::Text { text: t } => text.push_str(t),
                // 视觉矩阵（计划）：本协议工具结果不含图像——占位文本兜底
                //（registry 层已按 supports_image_result 替换，这里防御）。
                Block::Image { .. } => text.push_str(crate::vfs::syn_image_placeholder()),
                // reasoning 项逐字回传（store=false 全量重传形态下 OpenAI 要求
                // 带思维链的 assistant 轮次把 reasoning 项一并回给）。
                Block::ThinkingRaw { data } => input.push(data.clone()),
                Block::ToolCall { .. } => {}
            }
        }
        match msg.role {
            Role::User => {
                if !text.is_empty() {
                    input.push(json!({
                        "role": "user",
                        "content": [{ "type": "input_text", "text": text }],
                    }));
                }
            }
            Role::Assistant => {
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text }],
                    }));
                }
                for b in &msg.content {
                    if let Block::ToolCall { call } = b {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            // arguments 线上是 JSON 字符串（统一形态的对象回序列化）。
                            "arguments": serde_json::to_string(&call.arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        }));
                    }
                }
            }
        }
    }
    let mut body = json!({
        "model": cfg.model,
        "input": input,
        // v1 无服务端状态：store=false + 全量重传（与循环自持历史配套）。
        "store": false,
        "max_output_tokens": req.max_output_tokens,
    });
    // effort → reasoning.effort（推理系模型；不支持模型 4xx 走 Fatal 回配置面）。
    if let Some(level) = super::sanitize_effort(&cfg.effort) {
        body["reasoning"] = json!({ "effort": level });
    }
    // 流式（兼容性开关）：等 response.completed 帧取完整响应对象聚合。
    if cfg.stream {
        body["stream"] = json!(true);
    }
    if !req.system.is_empty() {
        body["instructions"] = json!(req.system);
    }
    if !req.tools.is_empty() {
        // 扁平工具定义：不嵌 function 壳。
        body["tools"] = json!(req.tools.iter()
            .map(|t| json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.input_schema,
            }))
            .collect::<Vec<_>>());
    }
    body
}

/// Responses 响应体 → 统一响应。
fn parse_response(text: &str) -> Result<ChatResponse, LlmError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| LlmError::Fatal(format!("OpenAI Responses 响应不是合法 JSON: {e}")))?;
    // 200 但 status=failed（内容审核等）：可重试类失败，走 Transient 让循环的
    // 连续失败计数兜底，而不是把配置错误式的 Fatal 挂给用户。
    if v.get("status").and_then(Value::as_str) == Some("failed") {
        let err = v.get("error").cloned().unwrap_or(Value::Null);
        let detail = err.as_str().map_or_else(|| trunc(&err.to_string()), str::to_string);
        return Err(LlmError::Transient(format!("responses status=failed: {detail}")));
    }
    let mut content = String::new();
    let mut thinking_text = String::new();
    let mut thinking_blocks = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(items) = v.get("output").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                        for c in blocks {
                            if c.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(t) = c.get("text").and_then(Value::as_str) {
                                    content.push_str(t);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| json!({}));
                    tool_calls.push(ToolCall {
                        // id 取 call_id：回传 function_call_output 时配对的就是它。
                        id: item.get("call_id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        arguments,
                    });
                }
                // reasoning 项：原样收进回放块（store=false 重传要求），summary
                // 明文拼 thinking_text 供测试模式展示。
                Some("reasoning") => {
                    if let Some(summaries) = item.get("summary").and_then(Value::as_array) {
                        for s in summaries {
                            if let Some(t) = s.get("text").and_then(Value::as_str) {
                                thinking_text.push_str(t);
                            }
                        }
                    }
                    thinking_blocks.push(item.clone());
                }
                _ => {}
            }
        }
    }
    let incomplete_reason =
        v.get("incomplete_details").and_then(|d| d.get("reason")).and_then(Value::as_str);
    let stop = if incomplete_reason == Some("max_output_tokens") {
        // 优先级最高：半截的 function_call 参数可能被截断，循环会全部作废回错误。
        StopReason::MaxTokens
    } else if !tool_calls.is_empty() {
        StopReason::ToolUse
    } else if v.get("status").and_then(Value::as_str) == Some("completed") {
        StopReason::EndTurn
    } else {
        StopReason::Other
    };
    let usage = v.get("usage").cloned().unwrap_or(Value::Null);
    Ok(ChatResponse {
        content,
        thinking_text,
        thinking_blocks,
        tool_calls,
        stop,
        usage: Usage {
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_read_tokens: usage
                .get("input_tokens_details")
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
        LlmClient::OpenAiResponses {
            cfg: LlmConfig {
                protocol: Protocol::OpenAiResponses,
                base_url,
                model: "responses-test".to_string(),
                max_output_tokens: 2048,
                reply_lang: None,
                effort: None,
                debug: false,
                stream: false,
            },
            api_key: "k-rsp".to_string(),
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
                    content: vec![Block::ToolCall {
                        call: ToolCall {
                            id: "call_a".to_string(),
                            name: "read".to_string(),
                            arguments: json!({ "path": "/game/board" }),
                        },
                    }],
                },
                Msg {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        call_id: "call_a".to_string(),
                        content: "{\"kind\":\"gomoku\"}".to_string(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "read".to_string(),
                description: "read a file".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }],
            max_output_tokens: 2048,
        }
    }

    fn body_of(raw: &str) -> Value {
        let (_, after) = raw.split_once("\r\n\r\n").expect("请求头与体以空行分隔");
        serde_json::from_str(after).expect("请求体必须是合法 JSON")
    }

    /// 请求关键字段：顶层 instructions / 扁平工具定义（不嵌 function 壳）/
    /// store:false / function_call 与 function_call_output 条目 / Bearer 头。
    #[tokio::test]
    async fn request_shape() {
        let stub = Stub::start(vec![(
            200,
            r#"{"status":"completed","output":[],"usage":{}}"#.to_string(),
        )]);
        client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        let raw = stub.last_request();
        assert!(raw.starts_with("POST /responses HTTP/1.1"), "应 POST 到 /responses：{raw}");
        // hyper 上线的头名一律小写（同 openai_chat 的断言口径）。
        assert!(
            raw.to_ascii_lowercase().contains("authorization: bearer k-rsp"),
            "缺 Bearer 鉴权头：{raw}"
        );

        let body = body_of(&raw);
        assert_eq!(body["model"], "responses-test");
        assert_eq!(body["instructions"], "You are a player.", "系统提示词在顶层 instructions");
        assert_eq!(body["store"], false, "v1 无服务端状态");
        assert_eq!(body["max_output_tokens"], 2048);

        // 扁平工具定义。
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
        assert!(body["tools"][0].get("function").is_none(), "Responses 不嵌 function 壳");

        let input = body["input"].as_array().expect("input 条目流").clone();
        assert_eq!(input.len(), 3, "user 文本 / function_call / function_call_output");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        // function_call 条目：arguments 是 JSON 字符串。
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_a");
        assert_eq!(input[1]["arguments"], r#"{"path":"/game/board"}"#);
        // 工具结果条目。
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_a");
        assert_eq!(input[2]["output"], "{\"kind\":\"gomoku\"}");
    }

    /// 响应解析：output 条目归一 + function_call 的 call_id 作统一 id + usage。
    #[tokio::test]
    async fn response_parse_and_usage() {
        let stub = Stub::start(vec![(200, r#"{
            "status": "completed",
            "output": [
                {"type":"message","role":"assistant",
                 "content":[{"type":"output_text","text":"Board read. "},{"type":"output_text","text":"Now I move."}]},
                {"type":"function_call","id":"fc_1","call_id":"call_a","name":"write",
                 "arguments":"{\"path\":\"/game/in/move\",\"content\":\"7,7\"}"}
            ],
            "usage": {"input_tokens": 300, "output_tokens": 50,
                      "input_tokens_details": {"cached_tokens": 256}}
        }"#.to_string())]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        assert_eq!(resp.content, "Board read. Now I move.");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_a", "统一 id 取 call_id");
        assert_eq!(resp.tool_calls[0].name, "write");
        assert_eq!(resp.tool_calls[0].arguments["content"], "7,7");
        assert_eq!(resp.usage.input_tokens, 300);
        assert_eq!(resp.usage.output_tokens, 50);
        assert_eq!(resp.usage.cache_read_tokens, 256, "input_tokens_details.cached_tokens 归一");
        assert_eq!(resp.usage.cache_write_tokens, 0);
    }

    /// incomplete(max_output_tokens) → MaxTokens（优先于 function_call 存在）。
    #[tokio::test]
    async fn incomplete_max_output_tokens() {
        let stub = Stub::start(vec![(200, r#"{
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": [{"type":"function_call","call_id":"call_a","name":"write","arguments":"{\"path\":"}],
            "usage": {}
        }"#.to_string())]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");
        assert_eq!(resp.stop, StopReason::MaxTokens, "截断判定优先，半截调用循环会作废");
    }

    /// 错误分类：401 → Fatal；context_length_exceeded → ContextWindowExceeded；
    /// 200+status=failed → Transient。
    #[tokio::test]
    async fn error_classification() {
        let unauth = Stub::start(vec![(401, r#"{"error":{"message":"invalid api key"}}"#.to_string())]);
        let err = client(unauth.base_url.clone()).await.chat(sample_request()).await.expect_err("401 应 Fatal");
        assert!(matches!(&err, LlmError::Fatal(m) if m.contains("401")), "实际：{err:?}");

        let overflow = Stub::start(vec![(400, r#"{"error":{"code":"context_length_exceeded","message":"Requested token count exceeds the context window"}}"#.to_string())]);
        let err = client(overflow.base_url.clone()).await.chat(sample_request()).await.expect_err("超窗应 ContextWindowExceeded");
        assert!(
            matches!(&err, LlmError::ContextWindowExceeded { model_limit: None }),
            "该文案解析不出实限数字 → None（特征命中已足够触发紧急压缩）：{err:?}"
        );

        let failed = Stub::start(vec![(200, r#"{"status":"failed","error":{"code":"server_error","message":"generation failed"}}"#.to_string())]);
        let err = client(failed.base_url.clone()).await.chat(sample_request()).await.expect_err("status=failed 应 Transient");
        assert!(matches!(err, LlmError::Transient(_)), "实际：{err:?}");
    }
}
