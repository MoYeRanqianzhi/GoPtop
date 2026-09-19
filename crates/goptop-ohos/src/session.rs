//! P2P 会话的鸿蒙宿主 —— 与 Tauri 那份（`src-tauri/src/session.rs`）同契约、同形状。
//!
//! 命令面、拉模式（`session_poll`）、宿主动作队列都与桌面一致，前端因此能共用
//! `frontend/src/net/session.ts` 里的那一份适配器，只在「怎么把命令送过去」上分叉
//! （Tauri 走 `invoke`，这边走 javaScriptProxy 的同步调用）。
//!
//! # 与 Tauri 版唯一的实质差异：存储怎么读写
//!
//! **鸿蒙没有反向通道**：javaScriptProxy 只支持 JS→ArkTS 的同步调用，ArkTS 不能
//! 反过来同步调进 JS。而 `Host::storage_get` 必须是同步返回值的（会话构造时要读
//! `goptop:userId` / `goptop:stun` / `goptop:avatar` 来决定身份与 STUN 线路）。
//!
//! 所以存储拆成两半：
//! - **读**：建会话时由 ArkTS 把设置整表快照传进来（`session_new` 的 `settings`），
//!   本层按需查这张表；
//! - **写**：走宿主动作队列回传给 JS，由 JS 经它自己的 `storeSet` 落盘。
//!
//! 好处是**只有一个写者**（JS 的存储门面），不会出现「Rust 与 ArkTS 各写一遍
//! 同一个 store.json、互相覆盖」的竞态——桌面端是 Rust 单写者（前端经 `invoke`
//! 也走 `store.rs`），两边各自只有一条写路径。
//!
//! # 线程
//!
//! NAPI 的 `aiPost` 走的是后台线程池，但会话类命令（`session_*`）是**同步调用**、
//! 跑在 ArkWeb 的 JS 线程上。传输层到处 `tokio::spawn`，所以每次碰会话之前都要
//! [`goptop_transport_native::enter_runtime`]——与桌面壳踩过的是同一个坑
//! （`there is no reactor running` 直接 panic 带走进程）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession, SessionConfig, enter_runtime};
use serde_json::{Value, json};

/// 会话表（与 `lib.rs` 的 `GAMES`、桌面的 `Sessions` 同构）。
static SESSIONS: Mutex<Option<HashMap<u32, Entry>>> = Mutex::new(None);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// 一条会话：状态机句柄 + 它的宿主。
struct Entry {
    session: NativeSession,
    host: Arc<HarmonyHost>,
}

/// 回传给 JS 执行的平台动作。**字段形态与桌面端逐字一致**
/// （`src-tauri/src/session.rs` 的 `HostAction`），前端才能共用一份处理逻辑。
#[derive(Clone, serde::Serialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum HostAction {
    Notice { text: Option<String>, ms: Option<u32> },
    Copy { text: String },
    Nav { path: String },
    /// 鸿蒙专有：存储写入回传给 JS 落盘（见模块头的说明）。
    Store { key: String, value: Option<String> },
}

/// 鸿蒙宿主：读走建会话时传入的快照，写回传给 JS。
struct HarmonyHost {
    /// 建会话时由 ArkTS 传入的设置整表（只读；写入不回填，回传给 JS 由它落盘）。
    settings: Mutex<HashMap<String, String>>,
    actions: Mutex<Vec<HostAction>>,
}

impl HarmonyHost {
    fn new(settings: HashMap<String, String>) -> Self {
        Self { settings: Mutex::new(settings), actions: Mutex::new(Vec::new()) }
    }

    fn push(&self, a: HostAction) {
        if let Ok(mut v) = self.actions.lock() {
            v.push(a);
        }
    }

    fn take_actions(&self) -> Vec<HostAction> {
        self.actions.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

impl Host for HarmonyHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        self.settings.lock().ok()?.get(key).cloned()
    }

    fn storage_set(&self, key: &str, value: Option<&str>) {
        // 不在这里落盘：鸿蒙侧的唯一写者是 JS 的存储门面（见模块头）。
        self.push(HostAction::Store { key: key.to_string(), value: value.map(str::to_string) });
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
        // 拉模式：前端每次 `session_poll` 都取最新快照。
    }
}

/// 会话类命令的分发（返回 JSON 文本；`None` 表示不是本模块的命令）。
///
/// 命令名与桌面端**逐字一致**（`session_new` / `session_poll` / `session_cmd` / …），
/// 前端那份适配器不用为鸿蒙另写分支。
pub fn dispatch(cmd: &str, args: &Value) -> Option<Value> {
    let v: Value = match cmd {
        "session_new" => session_new(args),
        "session_drop" => {
            if let Some(id) = narg(args, "id") {
                with_sessions(|m| {
                    m.remove(&id);
                });
            }
            Value::Null
        }
        "session_poll" => {
            let Some(id) = narg(args, "id") else { return Some(Value::Null) };
            let polled = with_sessions(|m| {
                let e = m.get(&id)?;
                // 泵一次再取快照：IO 回调塞进队列的事件要在这里被消化。
                // `enter_runtime()` 不可省：pump 会执行 Effect，建连接/发消息/定时器
                // 都会 `tokio::spawn`，而本函数跑在 ArkWeb 的 JS 线程上。
                let _g = enter_runtime();
                e.session.pump();
                Some((e.session.snapshot(), e.host.take_actions()))
            })
            .flatten();
            match polled {
                Some((snap, actions)) => json!({ "snapshot": snap, "actions": actions }),
                None => json!({ "snapshot": "null", "actions": [] }),
            }
        }
        "session_cmd" => {
            let (Some(id), Some(raw)) = (narg(args, "id"), args.get("cmdJson").and_then(Value::as_str)) else {
                return Some(json!({ "ok": false, "error": "session_cmd 需要 (id, cmdJson)" }));
            };
            let Ok(c) = serde_json::from_str::<UiCommand>(raw) else {
                // 未知命令不能静默：静默的表现是「这个按钮没反应」，设备上极难归因
                return Some(json!({ "ok": false, "error": format!("bad cmd: {raw}") }));
            };
            let ok = with_sessions(|m| {
                let Some(e) = m.get(&id) else { return false };
                let _g = enter_runtime();
                e.session.cmd(c);
                true
            })
            .unwrap_or(false);
            if ok { json!({ "ok": true }) } else { json!({ "ok": false, "error": "no_session" }) }
        }
        "session_state_json" => read_session(args, |e| e.session.state_json()),
        "session_state_debug" => read_session(args, |e| e.session.state_debug()),
        "session_ice_debug" => read_session(args, |e| e.session.ice_debug()),
        // 纯函数解析：不需要会话实例
        "session_parse_link" => json!(goptop_transport_native::session::parse_link_json(sarg(args, "text").as_str())),
        "session_parse_answer" => json!(goptop_transport_native::session::parse_answer_json(sarg(args, "text").as_str())),
        _ => return None,
    };
    Some(v)
}

fn session_new(args: &Value) -> Value {
    let cfg_json = sarg(args, "cfgJson");
    let href = sarg(args, "href");
    let Ok(cfg) = serde_json::from_str::<SessionConfig>(&cfg_json) else {
        return json!({ "error": format!("cfg 解析失败: {cfg_json}") });
    };
    // 设置整表由 ArkTS 传入（鸿蒙没有反向通道，Rust 读不到 JS 的门面）
    let settings: HashMap<String, String> = args
        .get("settings")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let host = Arc::new(HarmonyHost::new(settings));
    let _g = enter_runtime();
    let s = NativeSession::new(cfg, host.clone() as Arc<dyn Host>, &href);
    s.start_pump();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    with_sessions(|m| {
        m.insert(id, Entry { session: s, host });
    });
    json!(id)
}

fn read_session(args: &Value, f: impl FnOnce(&Entry) -> String) -> Value {
    let Some(id) = narg(args, "id") else { return Value::Null };
    with_sessions(|m| m.get(&id).map(f)).flatten().map_or(Value::Null, |s| json!(s))
}

fn with_sessions<T>(f: impl FnOnce(&mut HashMap<u32, Entry>) -> T) -> Option<T> {
    let mut g = SESSIONS.lock().ok()?;
    Some(f(g.get_or_insert_with(HashMap::new)))
}

fn sarg(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn narg(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|v| v as u32)
}

/// 供 `lib.rs` 复用的 `Duration`（保持与 wasm 侧同周期的说明可读）。
#[allow(dead_code)]
const PUMP_PERIOD: Duration = Duration::from_millis(50);
