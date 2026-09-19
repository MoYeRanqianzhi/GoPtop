//! WASM 绑定 — 仅在 `feature = "wasm"` 时编译。
//!
//! 与 goptop-core 的 wasm.rs 同款约定：入参出参都是 JSON 字符串，前端已有 JSON
//! 解析习惯，且 wasm-bindgen 跨界传结构体成本高。
//!
//! 本模块只做「JSON ⇄ AnalyzeRequest/AnalyzeResult」的翻译，不含任何搜索逻辑。

use wasm_bindgen::prelude::*;

use crate::{analyze, AnalyzeRequest};

/// 分析一个局面，返回 `AnalyzeResult` 的 JSON。
///
/// 请求格式见 [`AnalyzeRequest`]；解析失败返回 `{"error":"..."}`（而不是抛异常，
/// 让 Worker 侧统一按结果对象处理）。
#[wasm_bindgen]
pub fn analyze_json(req_json: &str) -> String {
    let req: AnalyzeRequest = match serde_json::from_str(req_json) {
        Ok(r) => r,
        Err(e) => return format!("{{\"error\":\"bad request: {e}\"}}"),
    };
    match serde_json::to_string(&analyze(&req)) {
        Ok(s) => s,
        Err(e) => format!("{{\"error\":\"serialize: {e}\"}}"),
    }
}

/// 预热：加载并把权重解压进内存。Worker 启动时调用一次，
/// 免得第一步棋才付 14MB 解压 + 反序列化的代价（实测约 56ms）。
#[wasm_bindgen]
pub fn warmup() {
    crate::gomoku::warmup();
}
