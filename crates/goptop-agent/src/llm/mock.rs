//! Mock 剧本桩 —— 无头测试与无 key 演示的确定性「模型」。
//!
//! 语义极简：按序弹出预置 [`ChatResponse`]（或预置 [`LlmError`]——超窗兜底等
//! 错误路径的确定性驱动），请求内容一概不读（剧本在测试里写死，断言的是循环
//! 消费剧本的次序，不是模型对提示词的理解）。弹尽即 [`LlmError::Fatal`]：
//! 剧本少写一步是测试 bug，静默会让它伪装成「偶发卡死」。

use super::{ChatRequest, ChatResponse, LlmError, MockScript};

/// 弹出下一步预置响应。空剧本弹尽即 Fatal（文案即骨架注明的 `mock script
/// exhausted`——循环与测试都按它断言）。
///
/// `_req` 的「不读」指不解释（确定性桩不读懂提示词）；请求仍**留档**
/// （[`MockScript::recorded`]）——计划测试项「内置事件自动推送：下一轮 LLM 请求的
/// 注入消息里含 move 坐标」的断言对象就是请求原文，不记录只能黑盒猜。
pub(crate) async fn chat(script: &MockScript, req: ChatRequest) -> Result<ChatResponse, LlmError> {
    script.requests.lock().expect("锁中毒即 bug").push(req);
    let mut guard = script.script.lock().expect("锁中毒即 bug（与 EventQueue 同一语义）");
    if guard.is_empty() {
        return Err(LlmError::Fatal("mock script exhausted".to_string()));
    }
    match guard.remove(0) {
        super::MockStep::Reply(r) => Ok(r),
        super::MockStep::Error(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmClient, StopReason, Usage};
    use std::sync::Arc;

    fn resp(text: &str) -> ChatResponse {
        ChatResponse {
            content: text.to_string(),
            thinking_text: String::new(),
            thinking_blocks: vec![],
            tool_calls: vec![],
            stop: StopReason::EndTurn,
            usage: Usage { input_tokens: 1, output_tokens: 2, cache_read_tokens: 0, cache_write_tokens: 0 },
        }
    }

    /// 剧本按序弹出；弹尽 Fatal；remaining 随弹出递减到 0。
    #[tokio::test]
    async fn pops_in_order_then_fatals() {
        let script = Arc::new(MockScript::new(vec![resp("a"), resp("b")]));
        let client = LlmClient::Mock(Arc::clone(&script));
        assert_eq!(script.remaining(), 2);

        let r1 = client.chat(ChatRequest {
            system: "s".into(),
            messages: vec![],
            tools: vec![],
            max_output_tokens: 16,
        }).await.expect("第一步应有响应");
        assert_eq!(r1.content, "a");
        assert_eq!(script.remaining(), 1);

        client.chat(ChatRequest {
            system: "s".into(),
            messages: vec![],
            tools: vec![],
            max_output_tokens: 16,
        }).await.expect("第二步应有响应");
        assert_eq!(script.remaining(), 0);

        let err = client.chat(ChatRequest {
            system: "s".into(),
            messages: vec![],
            tools: vec![],
            max_output_tokens: 16,
        }).await.expect_err("弹尽应 Fatal");
        assert!(matches!(&err, LlmError::Fatal(m) if m == "mock script exhausted"));
        assert_eq!(err.message(), "mock script exhausted");
    }

    /// 空剧本直接 Fatal（测试剧本忘写是测试 bug，要炸得响）。
    #[tokio::test]
    async fn empty_script_fatals_immediately() {
        let client = LlmClient::Mock(Arc::new(MockScript::new(vec![])));
        let err = client.chat(ChatRequest {
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_output_tokens: 16,
        }).await.expect_err("空剧本应 Fatal");
        assert!(matches!(err, LlmError::Fatal(_)));
    }

    /// Mock 与 Anthropic 同形：工具结果带图判定为支持（最富路径）。
    #[test]
    fn mock_supports_image_result() {
        let client = LlmClient::Mock(Arc::new(MockScript::default()));
        assert!(client.supports_image_result());
    }
}
