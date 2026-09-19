//! P2P 会话的原生宿主 —— 桌面与 Android 直接跑 `goptop-transport-native`。
//!
//! 与 `rules.rs` / `ai.rs` 同一个思路：契约与状态机全在 crate 里
//! （`goptop-net` 的 Event/Effect、`goptop-transport-native` 的执行器），这里只做
//! 「Tauri command ↔ 会话实例 ↔ 宿主能力」的适配。
//!
//! **为什么命令面只有一个 `session_cmd`**：wasm 端每个 UiCommand 一个
//! `#[wasm_bindgen]` 方法（JS 侧静态类型调用），而 IPC 边界只能收字符串。若在宿主里
//! 再抄一份「方法名 → 命令」的分派表，两处契约迟早分叉，而分叉的表现是「某个命令在
//! 某个端静默无效」。所以给 `UiCommand` 加了 serde，序列化就是那份契约本身。
//!
//! **状态回推走拉模式**（`session_poll`），与 wasm 侧的 50ms 泵同构：前端
//! `start_pump()` 的语义两端一致，不必为原生端另立一套事件推送。宿主要做的
//! 平台动作（提示条 / 剪贴板 / 导航）在 Rust 侧没有 UI 可动，因此**入队**，
//! 由 `session_poll` 一并带回，由前端在同一个 JS 上下文里执行——这样
//! `window.goptopNotice` 这类既有钩子在两端是同一条路径。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession, SessionConfig};
use tauri::State;

/// 一条会话：状态机句柄 + 它的宿主。
///
/// 宿主句柄要**另外存一份**（`Arc` 共享，同一个对象也传给了 `NativeSession`）：
/// 宿主动作队列是宿主自己的事，`NativeSession` 不暴露它的 host——那属于内部结构。
pub struct SessionEntry {
    session: NativeSession,
    host: Arc<TauriHost>,
}

/// 多会话表（形状与 `rules.rs` 的 `Games` 同构）。
///
/// **必须有 id 而不是单例**：前端在「P2P 页」与「本地页」可能各有一个会话实例，
/// 且切换页面时会重建。
#[derive(Default)]
pub struct Sessions(pub Mutex<HashMap<u32, SessionEntry>>);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// 宿主在 Rust 侧做不到、要交给前端执行的动作。
///
/// 序列化成 `{"t":"notice","text":…,"ms":…}` 这类形态（tag 走 camelCase，
/// 与项目其余线上契约一致）。
#[derive(Clone, serde::Serialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum HostAction {
    Notice { text: Option<String>, ms: Option<u32> },
    Copy { text: String },
    Nav { path: String },
}

/// Tauri 宿主：存储落到 `store.rs` 的那份文件，其余动作入队。
///
/// `app` 是必要的——存储的落盘位置由 Tauri 的应用目录决定（桌面 `~/.goptop`、
/// Android 应用私有根），不能在这里自己拼路径，否则与前端门面写的不是同一份文件。
pub struct TauriHost {
    app: tauri::AppHandle,
    actions: Mutex<Vec<HostAction>>,
}

impl TauriHost {
    fn new(app: tauri::AppHandle) -> Self {
        Self { app, actions: Mutex::new(Vec::new()) }
    }

    fn push(&self, a: HostAction) {
        if let Ok(mut v) = self.actions.lock() {
            v.push(a);
        }
    }

    /// 取走累积的动作（取走即清空；`session_poll` 每次调用都会带上）。
    fn take_actions(&self) -> Vec<HostAction> {
        self.actions.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

impl Host for TauriHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        // 复用 `store_load`（与前端 `net/store` 门面同一份文件）——**不要另开一份**，
        // 否则「设置」在 UI 与传输层会看到两个不同的值。
        crate::store::store_load(self.app.clone()).ok()?.get(key).cloned()
    }

    fn storage_set(&self, key: &str, value: Option<&str>) {
        let r = match value {
            Some(v) => crate::store::store_set(self.app.clone(), key.to_string(), v.to_string()),
            None => crate::store::store_remove(self.app.clone(), key.to_string()),
        };
        // 失败不致命（读多写少，且下一次写入会重试），但必须留痕——
        // 静默丢弃的表现是「改了设置，重启后又变回去」，极难归因
        if let Err(e) = r {
            eprintln!("[session] 平台存储写入失败 key={key}: {e}");
        }
    }

    fn copy(&self, text: &str) {
        self.push(HostAction::Copy { text: text.to_string() });
    }

    fn notice(&self, text: Option<&str>, ms: Option<u32>) {
        self.push(HostAction::Notice { text: text.map(str::to_string), ms });
    }

    fn nav(&self, path: &str) {
        self.push(HostAction::Nav { path: path.to_string() });
    }

    fn emit(&self, _snapshot_json: &str) {
        // 拉模式：前端每次 `session_poll` 都取最新快照，此处无需记录。
        // 留着实现是为了与 `Host` 契约一致（wasm 侧对应 `window.goptopOnChange`）。
    }
}

/// 构造会话：`cfg_json` 与 wasm 侧 `WasmSession::new` 同字段（见 `SessionConfig`）。
///
/// **必须 async**：`NativeSession::new` 内部要 `tokio::spawn` 起 presence 订阅与
/// WS 连接，同步命令不在 tokio 运行时上下文里，会直接 panic。
#[tauri::command]
pub async fn session_new(
    app: tauri::AppHandle,
    sessions: State<'_, Sessions>,
    cfg_json: String,
    href: String,
) -> Result<Option<u32>, String> {
    let cfg: SessionConfig = serde_json::from_str(&cfg_json).map_err(|e| format!("cfg 解析失败: {e}"))?;
    let host = Arc::new(TauriHost::new(app));
    let s = NativeSession::new(cfg, host.clone() as Arc<dyn Host>, &href);
    s.start_pump();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    sessions
        .0
        .lock()
        .map_err(|_| "会话表已中毒".to_string())?
        .insert(id, SessionEntry { session: s, host });
    Ok(Some(id))
}

/// 释放会话（切页面/重开都要显式调，否则会话连同它的 tokio 任务一起常驻）。
#[tauri::command]
pub fn session_drop(sessions: State<'_, Sessions>, id: u32) {
    if let Ok(mut m) = sessions.0.lock() {
        m.remove(&id);
    }
}

/// 轮询：返回最新快照 + 本次累积的宿主动作。
///
/// 返回 `{"snapshot": <快照 JSON 文本>, "actions": [...]}`。快照**保持文本形态**
/// 而不是解成对象：前端 `snapshot()` 的契约就是「给我一段 JSON」，wasm 侧也是文本，
/// 两端一致才能共用同一份 `parseSnap`。
#[tauri::command]
pub fn session_poll(sessions: State<'_, Sessions>, id: u32) -> String {
    let Ok(m) = sessions.0.lock() else { return null_poll() };
    let Some(e) = m.get(&id) else { return null_poll() };
    // 泵一次再取快照：IO 回调塞进队列的事件要在这里被消化，
    // 否则前端要等下一个 50ms 周期才看到变化
    e.session.pump();
    let snap = e.session.snapshot();
    let actions = e.host.take_actions();
    serde_json::json!({ "snapshot": snap, "actions": actions }).to_string()
}

fn null_poll() -> String {
    serde_json::json!({ "snapshot": "null", "actions": [] }).to_string()
}

/// 一条 UI 命令。`cmd_json` 是 `UiCommand` 的 serde 形态（见 goptop-net 的说明）。
#[tauri::command]
pub fn session_cmd(sessions: State<'_, Sessions>, id: u32, cmd_json: String) -> String {
    let cmd: UiCommand = match serde_json::from_str(&cmd_json) {
        Ok(c) => c,
        // 不回 Result：调用方是「点一下就发」的 UI，抛错只会变成没人接的 rejection。
        // 给一条可读回执，前端把它打到控制台——**未知命令不能静默**，
        // 静默的表现是「这个按钮没反应」，在设备上极难归因。
        Err(e) => return serde_json::json!({ "ok": false, "error": format!("bad cmd: {e}") }).to_string(),
    };
    let Ok(m) = sessions.0.lock() else {
        return serde_json::json!({ "ok": false, "error": "lock poisoned" }).to_string();
    };
    let Some(e) = m.get(&id) else {
        return serde_json::json!({ "ok": false, "error": "no_session" }).to_string();
    };
    e.session.cmd(cmd);
    serde_json::json!({ "ok": true }).to_string()
}

/// 对局局面（AI 分析输入；与规则引擎的 `state_json` 同源同形）。
#[tauri::command]
pub fn session_state_json(sessions: State<'_, Sessions>, id: u32) -> String {
    sessions
        .0
        .lock()
        .ok()
        .and_then(|m| m.get(&id).map(|e| e.session.state_json()))
        .unwrap_or_else(|| "null".into())
}

/// 会话内部状态（E2E 诊断，与 wasm 的 `state_debug` 同契约）。
#[tauri::command]
pub fn session_state_debug(sessions: State<'_, Sessions>, id: u32) -> String {
    sessions
        .0
        .lock()
        .ok()
        .and_then(|m| m.get(&id).map(|e| e.session.state_debug()))
        .unwrap_or_else(|| "{}".into())
}

/// 连接诊断（E2E 用）。**与 wasm 侧深度不同**：wasm 能同步读 ICE 状态与候选列表，
/// webrtc-rs 的对应 API 是 async，而本方法的调用方（诊断钩子）是同步读，
/// 所以只报账本里直接有的（连接 tag、是否已关闭）——见 `NativeSession::ice_debug`。
#[tauri::command]
pub fn session_ice_debug(sessions: State<'_, Sessions>, id: u32) -> String {
    sessions
        .0
        .lock()
        .ok()
        .and_then(|m| m.get(&id).map(|e| e.session.ice_debug()))
        .unwrap_or_else(|| "[]".into())
}

/// 解析粘贴的链接 / 回执（纯函数，不需要会话实例）。
#[tauri::command]
pub fn session_parse_link(text: String) -> String {
    goptop_transport_native::session::parse_link_json(&text)
}

#[tauri::command]
pub fn session_parse_answer(text: String) -> String {
    goptop_transport_native::session::parse_answer_json(&text)
}
