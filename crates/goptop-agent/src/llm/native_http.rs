//! [`HttpChannel`] 的 native 实现 —— reqwest + rustls(ring)。
//!
//! 通道不做任何协议解释：POST JSON、原样回 `(状态码, 体)`，分类判断归适配器
//!（见 [`super`] 模块注）——这样错误分类能对着真实错误体写测试，传输层与协议层
//! 各改各的互不牵连。

use std::time::Duration;

use async_trait::async_trait;

use super::HttpChannel;

/// 单次请求超时（计划拍板 60s：非流式 LLM 回复的正常上界；更长的生成属于
/// 配置失误——max_output_tokens 该调小——而不是把等待无限拉长）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// reqwest 客户端壳。连接池生命周期归它（[`super::LlmClient`] 变体只存
/// `Arc<dyn HttpChannel>`，无全局状态）。
pub struct NativeHttp {
    client: reqwest::Client,
}

impl NativeHttp {
    /// 建客户端（进程级一次；多个实例共存也只是多一个连接池，无碍）。
    ///
    /// # Errors
    /// 客户端构建失败（TLS 后端初始化异常等环境级故障）。
    pub fn new() -> Result<Self, String> {
        // rustls 0.23 要求显式 crypto provider（进程级一次性安装；重复安装回 Err，
        // 多客户端并存时以首个安装者为准——忽略即幂等）。ring 的取舍见
        // crates/goptop-agent/Cargo.toml 的说明（aws-lc-rs 在 Windows MSVC 上要
        // 现场编 C 源）。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| format!("http client build failed: {e}"))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl HttpChannel for NativeHttp {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String), String> {
        let mut req = self.client.post(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("http post failed: {e}"))?;
        // 非 2xx 不算 Err：状态码+体原样带回，分类是适配器的事。
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("http body read failed: {e}"))?;
        Ok((status, text))
    }
}
