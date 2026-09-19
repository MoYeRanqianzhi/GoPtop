//! AI 引擎的原生宿主 —— 桌面与 Android 直接跑 native Rust。
//!
//! AI 是纯计算（五子棋的 NNUE 推理 + 围棋的 MCTS），原生编译能吃上 NEON/AVX2，
//! 而 noru 在 wasm 上**只有标量路径**。此前它被绕道 WebView 跑 wasm，白白丢掉
//! SIMD——这是「把 Web 端的编译目标当成全平台实现方式」的直接代价。

use goptop_ai::{AnalyzeRequest, analyze};
use tauri::async_runtime;

/// 分析一个局面，返回 `AnalyzeResult` 的 JSON。
///
/// **必须异步**：一次搜索几百毫秒到几秒，同步 command 会占住 Tauri 的主线程，
/// 窗口直接卡死（Web 端靠 Worker 隔离，原生这边靠 blocking 线程池）。
#[tauri::command]
pub async fn ai_analyze(req_json: String) -> Result<String, String> {
    async_runtime::spawn_blocking(move || {
        let req: AnalyzeRequest =
            serde_json::from_str(&req_json).map_err(|e| format!("bad request: {e}"))?;
        serde_json::to_string(&analyze(&req)).map_err(|e| format!("serialize: {e}"))
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

/// 预热：解压并反序列化 NNUE 权重（约 56ms），免得第一步棋才付这个代价。
#[tauri::command]
pub async fn ai_warmup() {
    let _ = async_runtime::spawn_blocking(goptop_ai::gomoku::warmup).await;
}
