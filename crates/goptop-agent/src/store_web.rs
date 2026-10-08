//! /memory 的 web 后端 —— 经 `window.goptopVfsCall` 同步桥的 [`VfsStore`] 实现
//! （阶段⑤契约 §4：冻结的 op 表，本文件是 Rust 侧唯一消费方）。
//!
//! 桥的形状（冻结，改任何一头要同步另一头）：同步、JSON 进出、永不 throw——
//! `window.goptopVfsCall(op, payloadJson) -> jsonString`，回执恒为 JSON 文本，
//! JS 侧把任何异常折成 `{"ok":false,"error":"..."}`。trait 是同步 `fn`（见
//! [`crate::store::VfsStore`] 注），IndexedDB 的异步由 TS 镜像层吸收
//! （内存镜像 + write-behind），Rust 侧零 await、零 cfg 分叉。
//!
//! `path` 一律是**存表键形态**（规范化已在 [`crate::vfs`] / [`crate::store::normalize_path`]
//! 前置完成，本层透传）；配额（单文件/ns 总量）在 Rust 侧查——JS 侧只管存取。

use wasm_bindgen::prelude::*;

use crate::store::{check_quota, VfsStore};

/// web 后端：持 `window.goptopVfsCall` 的函数引用（模块加载即装、页面生命周期
/// 内不变，缓存引用省每次 Reflect）。
pub struct WebStore {
    call: js_sys::Function,
}

// Send/Sync 标记只为满足共享 trait 的界（VfsStore: Send + Sync，native 消费方
// 跨线程持它）；wasm 单线程，JsValue 实际绝无跨线程移交，标记在 wasm 目标下是
// 空口头的（编译期成立、运行期无风险）。
unsafe impl Send for WebStore {}
unsafe impl Sync for WebStore {}

impl WebStore {
    /// 从全局钩子构造（`window.goptopVfsCall` 由前端 agentVfs 门面在模块加载时
    /// 安装；AgentPage 保证 `agentVfsReady()` 后才 agent_start）。
    ///
    /// # Errors
    /// 钩子未安装/不是函数——装配层把文本原样落进 error 态（人话、可归因）。
    pub fn from_hook() -> Result<Self, String> {
        let f = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("goptopVfsCall"))
            .map_err(|e| format!("memory store: goptopVfsCall 读取失败: {e:?}"))?;
        if !f.is_function() {
            return Err("memory store: window.goptopVfsCall 未安装（agentVfs 门面未加载？）".into());
        }
        Ok(Self { call: js_sys::Function::from(f) })
    }

    /// 调一个 op 并解析回执。回执恒为 JSON 文本；非 JSON/抛错折成 Err(String)
    /// （存储层故障的人话形态，与 [`crate::store`] 的错误口径一致）。
    fn call_op(&self, op: &str, payload: serde_json::Value) -> Result<serde_json::Value, String> {
        let ret = self
            .call
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(op),
                &JsValue::from_str(&payload.to_string()),
            )
            .map_err(|e| format!("memory store error: goptopVfsCall({op}) failed: {e:?}"))?;
        let text = ret
            .as_string()
            .ok_or_else(|| format!("memory store error: goptopVfsCall({op}) did not return text"))?;
        serde_json::from_str(&text)
            .map_err(|e| format!("memory store error: goptopVfsCall({op}) bad json: {e}"))
    }

    /// 回执的统一解包：`ok:false` → Err(error)，否则回整个对象。
    fn unwrap_ok(&self, v: serde_json::Value) -> Result<serde_json::Value, String> {
        if v["ok"] == serde_json::Value::Bool(false) {
            return Err(v["error"].as_str().unwrap_or("unknown error").to_string());
        }
        Ok(v)
    }
}

impl VfsStore for WebStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        let v = self.unwrap_ok(self.call_op("read", serde_json::json!({ "ns": ns, "path": path }))?)?;
        // found=false 时 content 为 null（契约 §4 表）；不隐式区分「字段缺」与
        // 「不存在」——桥的回执形状由 TS 侧冻结，这里按 found 单一事实源判定。
        if v["found"] == serde_json::Value::Bool(true) {
            Ok(Some(v["content"].as_str().unwrap_or_default().to_string()))
        } else {
            Ok(None)
        }
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        let len = content.len();
        // 配额前置查（与 native 同一份 [`check_quota`]）：old 取旧内容字节数、
        // used 取 ns 占用——两次钩子调用之间没有事务，并发写窗在 web 单 Agent 局
        // 下不存在（同一循环串行调工具），读改写的窗口可忽略。
        let old = self
            .read(ns, path)?
            .map(|c| c.len() as u64)
            .unwrap_or(0);
        let used = self.usage(ns)?;
        check_quota(ns, len, old, used)?;
        self.unwrap_ok(self.call_op(
            "write",
            serde_json::json!({ "ns": ns, "path": path, "content": content }),
        )?)?;
        Ok(())
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        let v = self.unwrap_ok(self.call_op("delete", serde_json::json!({ "ns": ns, "path": path }))?)?;
        Ok(v["deleted"] == serde_json::Value::Bool(true))
    }

    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String> {
        let v = self.unwrap_ok(self.call_op("list", serde_json::json!({ "ns": ns, "prefix": prefix }))?)?;
        Ok(v["paths"]
            .as_array()
            .map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }

    fn usage(&self, ns: &str) -> Result<u64, String> {
        let v = self.unwrap_ok(self.call_op("usage", serde_json::json!({ "ns": ns }))?)?;
        Ok(v["bytes"].as_u64().unwrap_or(0))
    }
}
