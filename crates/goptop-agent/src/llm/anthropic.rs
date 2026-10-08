//! Anthropic Messages 协议适配器（非流式 `POST {base}/messages`）。
//!
//! 映射要点（计划「LLM 客户端」节 + 官方 wire 基线）：
//! - 系统提示词放**顶层 `system`**（不进 messages）；`max_tokens` **必填**；
//! - 鉴权头 `x-api-key` + `anthropic-version: 2023-06-01`；
//! - 工具定义 `{name, description, input_schema}`；
//! - assistant 侧工具调用 = content 里的 `tool_use` 块，**`input` 是对象**（统一
//!   形态 [`crate::llm::Block::ToolCall`] 的 arguments 本就是对象，直传）；
//! - 工具结果 = **user 消息**里的 `tool_result` 块（`tool_use_id` 配对、`is_error`
//!   标错）；图像 = `image` 块（base64 source）；
//! - 相邻同角色消息合并：Anthropic 线上要求 user/assistant 严格交替，compact
//!   重排后的 `[user: 摘要] + [tail 首条 user]` 必须在这里并成一条。
//!
//! 响应侧：text 块拼接为 `content`，`tool_use` 块归一成 [`ToolCall`]；
//! `stop_reason`：`tool_use`→ToolUse、`max_tokens`→MaxTokens、`end_turn`→EndTurn、
//! 其余→Other；usage 取 `input_tokens` / `output_tokens` /
//! `cache_read_input_tokens` / `cache_creation_input_tokens`（缺省 0）。

use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    Block, ChatRequest, ChatResponse, HttpChannel, LlmConfig, LlmError, Role, StopReason, ToolCall,
    Usage, classify_status, post_with_retry,
};

/// 端点基址拼尾路径（base_url 约定含版本段，如 `https://api.anthropic.com/v1`）。
const PATH: &str = "/messages";

/// 一次非流式对话（[`super::LlmClient::Anthropic`] 的实现体）。
pub(crate) async fn chat(
    cfg: &LlmConfig,
    api_key: &str,
    http: &Arc<dyn HttpChannel>,
    req: ChatRequest,
) -> Result<ChatResponse, LlmError> {
    let url = format!("{}{PATH}", cfg.base_url.trim_end_matches('/'));
    let headers = vec![
        ("x-api-key".to_string(), api_key.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
    ];
    let body = build_body(cfg, &req).to_string();
    let (status, text) = post_with_retry(http.as_ref(), &url, &headers, &body).await?;
    if !(200..300).contains(&status) {
        return Err(classify_status(status, &text));
    }
    parse_response(&text)
}

/// 统一请求 → Anthropic 请求体。
fn build_body(cfg: &LlmConfig, req: &ChatRequest) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len());
    for msg in &req.messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let content: Vec<Value> = msg.content.iter().map(block_to_wire).collect();
        messages.push(json!({ "role": role, "content": content }));
    }
    let mut body = json!({
        "model": cfg.model,
        // max_tokens 是 Anthropic 的必填字段（骨架注：默认 1024 由装配层填）。
        "max_tokens": req.max_output_tokens,
        "messages": merge_same_role(messages),
    });
    // effort → extended thinking：预算表逐字对齐 pi DEFAULT_THINKING_BUDGETS
    //（low 2048 / medium 8192 / high 16384，另有 MIN_ANSWER_TOKENS=1024 恒留给正文）。
    // max_tokens 必须盖住「思考+正文」，否则思考吃光预算、正文零输出
    //（stop=max_tokens 空手而归——实测卡住形态之一）。
    if let Some(level) = super::sanitize_effort(&cfg.effort) {
        let budget = match level.as_str() {
            "low" => 2048,
            "medium" => 8192,
            _ => 16384,
        };
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
        body["max_tokens"] = json!(req.max_output_tokens.max(1024) + budget);
    }
    if !req.system.is_empty() {
        body["system"] = json!(req.system);
    }
    if !req.tools.is_empty() {
        body["tools"] = json!(req.tools.iter()
            .map(|t| json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            }))
            .collect::<Vec<_>>());
    }
    body
}

/// 单个统一块 → Anthropic content 块。
fn block_to_wire(b: &Block) -> Value {
    match b {
        Block::Text { text } => json!({ "type": "text", "text": text }),
        // input 必须是对象：Anthropic 拒收字符串形态的 tool_use.input。
        Block::ToolCall { call } => json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.arguments,
        }),
        Block::ToolResult { call_id, content, is_error } => json!({
            "type": "tool_result",
            "tool_use_id": call_id,
            "content": content,
            "is_error": is_error,
        }),
        Block::Image { mime, data_base64 } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": mime, "data": data_base64 },
        }),
        // thinking 块逐字回传：extended thinking + 工具循环时 API 强制要求
        // assistant 消息带原 thinking 块（签名完整性），丢了第二手直接 4xx。
        Block::ThinkingRaw { data } => data.clone(),
    }
}

/// 相邻同角色合并：Anthropic 要求 messages 严格交替，content 数组顺序拼接。
/// 拼接保序即保语义（tool_result 块仍先于后补的文本块——循环构造时就是这顺序）。
fn merge_same_role(mut messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for msg in messages.drain(..) {
        let same_role = merged
            .last()
            .and_then(|p| p.get("role"))
            .and_then(Value::as_str)
            == msg.get("role").and_then(Value::as_str);
        if same_role {
            let last = merged.last_mut().expect("same_role 蕴含 last 存在");
            if let (Some(dst), Some(src)) = (
                last.get_mut("content").and_then(Value::as_array_mut),
                msg.get("content").and_then(Value::as_array),
            ) {
                dst.extend(src.iter().cloned());
                continue;
            }
        }
        merged.push(msg);
    }
    merged
}

/// Anthropic 响应体 → 统一响应。
fn parse_response(text: &str) -> Result<ChatResponse, LlmError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| LlmError::Fatal(format!("Anthropic 响应不是合法 JSON: {e}")))?;
    let mut content = String::new();
    let mut thinking_text = String::new();
    let mut thinking_blocks = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(blocks) = v.get("content").and_then(Value::as_array) {
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        content.push_str(t);
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: b.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        name: b.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        // input 是对象；异常缺失按空对象兜——registry 的参数校验会
                        // 给模型一条可自纠的错误，比在这里 Fatal 更符合工具语义。
                        arguments: b.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
                // thinking / redacted_thinking：原样收进回放块（签名完整性），
                // 明文部分另拼 thinking_text 供测试模式展示。
                Some("thinking") | Some("redacted_thinking") => {
                    if let Some(t) = b.get("thinking").and_then(Value::as_str) {
                        thinking_text.push_str(t);
                    }
                    thinking_blocks.push(b.clone());
                }
                _ => {}
            }
        }
    }
    let stop = match v.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("end_turn") => StopReason::EndTurn,
        _ => StopReason::Other,
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
            cache_read_tokens: usage.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_write_tokens: usage.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmClient, LlmConfig, Msg, NativeHttp, Protocol, ToolSpec, testing::Stub};
    use std::sync::Arc;

    /// 经真 TCP 回环（NativeHttp → std::TcpListener 桩）跑一次对话。
    async fn client(base_url: String) -> LlmClient {
        LlmClient::Anthropic {
            cfg: LlmConfig {
                protocol: Protocol::Anthropic,
                base_url,
                model: "claude-test".to_string(),
                max_output_tokens: 1024,
                reply_lang: None,
                effort: None,
                debug: false,
            },
            api_key: "k-test".to_string(),
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
                            id: "call_1".to_string(),
                            name: "write".to_string(),
                            arguments: json!({ "path": "/game/in/move", "content": "7,7" }),
                        },
                    }],
                },
                Msg {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        call_id: "call_1".to_string(),
                        content: "{\"ok\":true}".to_string(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "write".to_string(),
                description: "stage or memorize".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }],
            max_output_tokens: 1024,
        }
    }

    fn body_of(raw: &str) -> Value {
        let (_, after) = raw.split_once("\r\n\r\n").expect("请求头与体以空行分隔");
        serde_json::from_str(after).expect("请求体必须是合法 JSON")
    }

    /// 请求关键字段：尾路径/鉴权头/顶层 system/必填 max_tokens/工具定义/工具对。
    #[tokio::test]
    async fn request_shape() {
        let stub = Stub::start(vec![(200, r#"{"content":[],"stop_reason":"end_turn","usage":{}}"#.to_string())]);
        client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        let raw = stub.last_request();
        assert!(raw.starts_with("POST /messages HTTP/1.1"), "应 POST 到 /messages：{raw}");
        assert!(raw.contains("x-api-key: k-test"));
        assert!(raw.contains("anthropic-version: 2023-06-01"));

        let body = body_of(&raw);
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["max_tokens"], 1024, "Anthropic 的 max_tokens 必填");
        assert_eq!(body["system"], "You are a player.", "系统提示词在顶层");
        assert_eq!(body["tools"][0]["name"], "write");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");

        let msgs = body["messages"].as_array().expect("messages 数组");
        assert_eq!(msgs.len(), 3, "user/assistant/user 已交替，不应合并");
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        // tool_use 的 input 是对象（不是字符串）。
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "call_1");
        assert_eq!(msgs[1]["content"][0]["input"]["path"], "/game/in/move");
        // 工具结果是 user 消息里的 tool_result 块。
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(msgs[2]["content"][0]["is_error"], false);
    }

    /// 相邻同角色合并成一条（Anthropic 要求严格交替）。
    #[tokio::test]
    async fn merges_consecutive_same_role() {
        let stub = Stub::start(vec![(200, r#"{"content":[],"stop_reason":"end_turn","usage":{}}"#.to_string())]);
        let mut req = sample_request();
        req.messages.insert(
            0,
            Msg { role: Role::User, content: vec![Block::Text { text: "earlier note".to_string() }] },
        );
        client(stub.base_url.clone()).await.chat(req).await.expect("200 应成功");

        let msgs = body_of(&stub.last_request())["messages"].as_array().expect("messages").clone();
        assert_eq!(msgs.len(), 3, "开头两条 user 应合并");
        assert_eq!(msgs[0]["content"].as_array().expect("content").len(), 2);
        assert_eq!(msgs[0]["content"][0]["text"], "earlier note");
    }

    /// 响应解析：text 拼接 + tool_use 归一 + stop_reason + usage 归一（cache 字段）。
    #[tokio::test]
    async fn response_parse_and_usage() {
        let stub = Stub::start(vec![(200, r#"{
            "content": [
                {"type":"text","text":"Let me "},
                {"type":"text","text":"think."},
                {"type":"tool_use","id":"toolu_1","name":"write","input":{"path":"/game/in/move"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 120, "output_tokens": 34,
                      "cache_read_input_tokens": 100, "cache_creation_input_tokens": 5}
        }"#.to_string())]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");

        assert_eq!(resp.content, "Let me think.", "多 text 块按序拼接");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "toolu_1");
        assert_eq!(resp.tool_calls[0].name, "write");
        assert_eq!(resp.tool_calls[0].arguments["path"], "/game/in/move");
        assert_eq!(resp.usage.input_tokens, 120);
        assert_eq!(resp.usage.output_tokens, 34);
        assert_eq!(resp.usage.cache_read_tokens, 100, "cache_read_input_tokens 归一");
        assert_eq!(resp.usage.cache_write_tokens, 5, "cache_creation_input_tokens 归一");
        assert_eq!(resp.usage.total(), 154);
    }

    /// stop_reason=max_tokens 归一为 MaxTokens（循环据此作废半截工具调用）。
    #[tokio::test]
    async fn max_tokens_stop_reason() {
        let stub = Stub::start(vec![(
            200,
            r#"{"content":[{"type":"text","text":"ha"}],"stop_reason":"max_tokens","usage":{"input_tokens":1,"output_tokens":1024}}"#.to_string(),
        )]);
        let resp = client(stub.base_url.clone()).await.chat(sample_request()).await.expect("200 应成功");
        assert_eq!(resp.stop, StopReason::MaxTokens);
        assert!(resp.tool_calls.is_empty());
    }

    /// 错误分类：401/404 → Fatal；超窗特征 → ContextWindowExceeded。
    #[tokio::test]
    async fn error_classification() {
        let unauth = Stub::start(vec![(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.to_string())]);
        let err = client(unauth.base_url.clone()).await.chat(sample_request()).await.expect_err("401 应 Fatal");
        assert!(matches!(&err, LlmError::Fatal(m) if m.contains("401")), "实际：{err:?}");

        let no_model = Stub::start(vec![(404, r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-nope"}}"#.to_string())]);
        let err = client(no_model.base_url.clone()).await.chat(sample_request()).await.expect_err("404（模型不存在）应 Fatal");
        assert!(matches!(err, LlmError::Fatal(m) if m.contains("404")));

        let overflow = Stub::start(vec![(400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 213462 tokens > 200000 maximum"}}"#.to_string())]);
        let err = client(overflow.base_url.clone()).await.chat(sample_request()).await.expect_err("超窗应 ContextWindowExceeded");
        assert!(
            matches!(&err, LlmError::ContextWindowExceeded { model_limit: Some(200_000) }),
            "超窗错误要带上错误体里的模型实限：{err:?}"
        );
    }

    /// 429/5xx 退避 2 次后仍失败 → Transient；期间恢复 → 成功。
    /// （共享退避件只在这里全链路验一遍，另两协议复用同一段代码。）
    #[tokio::test]
    async fn transient_retry_then_recover() {
        let recover = Stub::start(vec![
            (429, r#"{"type":"error","error":{"type":"rate_limit_error"}}"#.to_string()),
            (200, r#"{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#.to_string()),
        ]);
        let resp = client(recover.base_url.clone()).await.chat(sample_request()).await.expect("第二次应成功");
        assert_eq!(resp.content, "ok");
        assert_eq!(recover.requests.lock().expect("锁").len(), 2, "429 后应重试一次");

        let exhausted = Stub::start(vec![
            (429, "{}".to_string()),
            (429, "{}".to_string()),
            (500, "boom".to_string()),
        ]);
        let err = client(exhausted.base_url.clone()).await.chat(sample_request()).await.expect_err("重试耗尽应 Transient");
        assert!(matches!(&err, LlmError::Transient(m) if m.contains("500")), "实际：{err:?}");
        assert_eq!(exhausted.requests.lock().expect("锁").len(), 3, "共 3 次尝试");
    }
}
