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
//!
//! **碰会话之前先进运行时**：见 `goptop_transport_native::enter_runtime`——Tauri 的
//! 同步命令跑在主线程上，没有 tokio 上下文时传输层的 `tokio::spawn` 会直接 panic
//! 并把整个窗口进程带走（本项目实测踩过）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession, SessionConfig, enter_runtime};
use tauri::State;

/// 一条会话：状态机句柄 + 它的宿主。
///
/// 宿主句柄要**另外存一份**（`Arc` 共享，同一个对象也传给了 `NativeSession`）：
/// 宿主动作队列是宿主自己的事，`NativeSession` 不暴露它的 host——那属于内部结构。
pub struct SessionEntry {
    session: NativeSession,
    host: Arc<TauriHost>,
    /// 上次**实际回给前端**的快照文本（`session_poll` 的「无变化」判定基准）。
    ///
    /// 状态没变时每 50ms 重建并整段回传几 KB～几十 KB 的快照纯属浪费：序列化跑在
    /// Tauri 主线程上，与 UI 争同一线程。有这份缓存就能在宿主侧把「无变化」拦下来，
    /// 回 `{"snapshot":null}` 让前端沿用它的缓存——省掉整段 IPC 传输与 JS 侧解析。
    /// 只在会话表项里存一份（随 `session_drop` 一起释放），不设上限。
    last_served: Option<String>,
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
/// **同步命令 + 显式 `enter_runtime()`**：构造过程中要 `tokio::spawn` 起 presence
/// 订阅、WS 连接与 RTC 连接，必须站在运行时上下文里。做成 `async fn` 看似能借
/// Tauri 的运行时，但那个假设是错的（实测主线程 panic），显式进入才确定。
#[tauri::command]
pub fn session_new(
    app: tauri::AppHandle,
    sessions: State<'_, Sessions>,
    cfg_json: String,
    href: String,
) -> Result<Option<u32>, String> {
    let cfg: SessionConfig = serde_json::from_str(&cfg_json).map_err(|e| format!("cfg 解析失败: {e}"))?;
    let host = Arc::new(TauriHost::new(app));
    let _g = enter_runtime();
    let s = NativeSession::new(cfg, host.clone() as Arc<dyn Host>, &href);
    s.start_pump();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    sessions
        .0
        .lock()
        .map_err(|_| "会话表已中毒".to_string())?
        .insert(id, SessionEntry { session: s, host, last_served: None });
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
///
/// **「无变化」分支**：快照与上次回给前端的逐字相同 → `{"snapshot": null, "actions": [...]}`
/// （注意是 JSON `null`，不是字符串 `"null"`），前端据此沿用它的缓存、不触发重渲染。
/// 前端 50ms 轮询一次，对局页空闲时绝大多数轮询落在这条分支——省掉的是每拍的
/// 整段快照 IPC 传输 + JS 侧 `JSON.parse` + 整串比较（原生壳上这段工作压在
/// WebView 的 JS 线程，与动画/滚动争时间）。宿主动作**照常带回**：提示条/剪贴板/
/// 导航不能因为快照没变就被扣住。
#[tauri::command]
pub fn session_poll(sessions: State<'_, Sessions>, id: u32) -> String {
    let Ok(mut m) = sessions.0.lock() else { return null_poll() };
    // 要写 `last_served`，所以取 `get_mut` 而不是 `get`。
    let Some(e) = m.get_mut(&id) else { return null_poll() };
    // 泵一次再取快照：IO 回调塞进队列的事件要在这里被消化，
    // 否则前端要等下一个 50ms 周期才看到变化。
    // `enter()` 不可省：pump 会执行 Effect，其中 CreatePeer / 发消息 / 定时器都会
    // `tokio::spawn`——本命令是同步命令，跑在主线程上，没有上下文就 panic。
    let _g = enter_runtime();
    e.session.pump();
    let snap = e.session.snapshot();
    let actions = e.host.take_actions();
    match serve_snapshot(&mut e.last_served, snap) {
        None => serde_json::json!({ "snapshot": null, "actions": actions }).to_string(),
        Some(snap) => serde_json::json!({ "snapshot": snap, "actions": actions }).to_string(),
    }
}

/// 「无变化」判定 + 缓存更新：`snap` 与上次回给前端的相同 → `None`（前端沿用缓存），
/// 否则 `Some(snap)` 并把它记为「已回过」。
///
/// 首次 poll 的 `last_served` 是 `None`，必然走 `Some` 分支——契约上**首拍必须给全量**，
/// 否则前端拿着初始缓存 `"null"` 没有东西可沿用。比较是纯文本比对：快照字段固定、
/// 无时间戳/随机量（见 goptop-net `Session::snapshot`），同一状态两次序列化逐字节相同，
/// 所以「串相同」当且仅当「前端已见的状态 == 当前状态」。判定错了只会退化成
/// 「每拍都回全量」（与旧行为一致），不会丢变化。
fn serve_snapshot(last_served: &mut Option<String>, snap: String) -> Option<String> {
    if last_served.as_deref() == Some(snap.as_str()) {
        return None;
    }
    *last_served = Some(snap.clone());
    Some(snap)
}

/// 会话已不存在（切页面竞态 / 表已中毒）时的回包。
///
/// 这里刻意回**字符串** `"null"` 而非 JSON `null`：前端解析后得到空快照、界面复位，
/// 与「无变化、沿用缓存」是两种不同的语义，不能共用一个记号。
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
    // 同上：`cmd` 内部会同步泵一次，泵里会 spawn
    let _g = enter_runtime();
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

#[cfg(test)]
mod tests {
    use super::*;
    use goptop_transport_native::{HeadlessHost, Host};

    /// 判定函数的契约：首拍必给全量；相同串回 `None`；不同串回全量并换缓存。
    #[test]
    fn serve_snapshot_first_poll_serves_full() {
        let mut last = None;
        assert_eq!(serve_snapshot(&mut last, "A".into()), Some("A".into()), "首拍必须给全量，前端才有东西可缓存");
        assert_eq!(last.as_deref(), Some("A"), "已回过的快照要记下来，作为下一拍的比较基准");
    }

    #[test]
    fn serve_snapshot_unchanged_returns_none() {
        let mut last = Some("A".into());
        assert_eq!(serve_snapshot(&mut last, "A".into()), None, "快照与上次相同就是「无变化」，前端沿用缓存");
        assert_eq!(last.as_deref(), Some("A"), "无变化时缓存不动");
    }

    #[test]
    fn serve_snapshot_changed_serves_full_and_updates_cache() {
        let mut last = Some("A".into());
        assert_eq!(serve_snapshot(&mut last, "B".into()), Some("B".into()));
        assert_eq!(last.as_deref(), Some("B"), "缓存必须跟上实际回出去的内容");
    }

    /// 无头真会话驱动的回归（`HeadlessHost`，不碰 Tauri 的 `AppHandle`）：
    /// 连续两次 poll、状态未变 → 第二次走「无变化」分支（`snapshot` 为 null）。
    ///
    /// 走的是与线上同一条代码路径（`NativeSession::pump` + `Session::snapshot`），
    /// 同时守住整条优化的地基——**同一状态两次序列化必须逐字节相同**：快照里若混进
    /// 时间戳或随机顺序的 Map，这里就会红。
    fn headless_session() -> NativeSession {
        let cfg = SessionConfig {
            name: "测试甲".into(),
            server_mode: false,
            share_origin: String::new(),
            kind: "gomoku".into(),
            size: 15,
        };
        NativeSession::new(cfg, Arc::new(HeadlessHost::default()) as Arc<dyn Host>, "http://localhost/test")
    }

    /// 与 `session_poll` 同构的一次轮询：泵一次 → 快照 → 判定。
    fn poll_once(s: &NativeSession, last: &mut Option<String>) -> Option<String> {
        s.pump();
        serve_snapshot(last, s.snapshot())
    }

    #[test]
    fn two_polls_no_change_second_is_null() {
        let _g = enter_runtime();
        let s = headless_session();
        let mut last = None;
        let first = poll_once(&s, &mut last);
        assert!(first.is_some(), "首拍必须回全量快照");
        let second = poll_once(&s, &mut last);
        assert_eq!(second, None, "状态未变的第二拍必须回 snapshot:null，不该再整段重传");
    }

    #[test]
    fn state_change_serves_full_snapshot_again() {
        let _g = enter_runtime();
        let s = headless_session();
        let mut last = None;
        let first = poll_once(&s, &mut last).expect("首拍给全量");
        assert_eq!(poll_once(&s, &mut last), None, "空闲拍走无变化分支");

        // 改名：状态真的变了 → 下一拍恢复全量，且内容是新状态（不是旧缓存）。
        s.cmd(UiCommand::SetName("测试乙".into()));
        let third = poll_once(&s, &mut last).expect("状态变了必须回全量");
        assert_ne!(third, first, "回出去的必须是新快照");
        assert!(third.contains("\"name\":\"测试乙\""), "全量快照要带上新名字（serde_json 不转义非 ASCII）");
        // 之后继续空闲 → 回到无变化分支。
        assert_eq!(poll_once(&s, &mut last), None);
    }
}
