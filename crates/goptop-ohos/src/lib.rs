//! goptop-ohos — 鸿蒙原生宿主的 **Rust 侧**（纯 C ABI，不含任何 NAPI 声明）。
//!
//! # 为什么 Rust 侧只到 C ABI
//!
//! 鸿蒙的原生模块必须是 NAPI 模块，而 NAPI 的头文件（`napi/native_api.h`）由 DevEco
//! SDK 提供。在 Rust 里手写这些结构体与函数签名等于凭记忆复刻一份 ABI——`napi_module`
//! 的字段顺序、`napi_status` 的取值、`napi_callback` 的调用约定错一个字节，表现都是
//! 设备上加载即崩或静默不注册，且没有本地复现手段。
//!
//! 因此分工是：**C++ 写 NAPI 薄层**（`harmony/entry/src/main/cpp/napi_init.cpp`，
//! 头文件来自 SDK，ABI 由构造保证正确），**Rust 只出 C ABI**。这样还有一个额外好处：
//! 本 crate 能在宿主机上以 rlib 直接单测（见 `tests/`），不必动鸿蒙工具链。
//!
//! # 契约
//!
//! 与桌面/Android 端（`src-tauri/src/rules.rs` 的 Tauri command）**同源**：两边都只是
//! `goptop_core::json_api` 的宿主适配。
//!
//! **回执统一是一条 JSON 文本**（不是结构体）：C ABI 只有 `(cmd, argsJson) → char*`
//! 这一个口子，没法像 Tauri 那样按命令给不同的返回类型。前端因此与 Tauri 侧
//! （`game_place`/`game_state_json` 也回字符串）保持同一套解析习惯。
//!
//! | cmd | args | 回执（JSON 文本） |
//! |---|---|---|
//! | `game_new` | `{kindJson}` | 局号；失败 `null` |
//! | `game_drop` | `{id}` | `null` |
//! | `game_state_json` | `{id}` | GameState；无此局 `null` |
//! | `game_place` | `{id,x,y}` | PlaceResult；无此局 `{"ok":false,"error":"no_game"}` |
//! | `game_pass` / `game_undo` | `{id}` | 同上 |
//! | `game_score` | `{id,deadJson}` | 计分结果 |
//! | `game_reset` | `{id}` | `null` |
//! | `game_adopt` | `{id,boardJson,toMove,winner,historyJson}` | `true`/`false` |
//! | `game_board_size` | `{id}` | 尺寸（无此局 `0`） |
//! | `ai_analyze` | AnalyzeRequest | AnalyzeResult |
//! | `ai_warmup` | — | `null` |
//!
//! # 线程
//!
//! [`goptop_call`] 是**同步阻塞**的纯计算，**调用方负责把它放到非 UI 线程**：
//! `ai_analyze` 按预算要跑 0.3~3 秒，在 ArkWeb 的 JS 线程上直接调会把界面冻住
//! （与桌面端的 `spawn_blocking`、Web 端的 Web Worker 是同一件事）。
//! 规则类命令是微秒级，同步调无妨。

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use goptop_core::game::GameState;
use goptop_core::json_api;
use serde_json::{Value, json};

/// 多局实例表（与 `src-tauri/src/rules.rs` 的 `Games` 同构）。
///
/// 用局号索引而非单例：本地页与 P2P 页可能同时各有一局，前端每个页面各持一个引擎
/// 实例。局号由 `game_new` 分配，用完调 `game_drop` 释放；不 drop 就随每次进出页面
/// /切尺寸永久累积。
static GAMES: Mutex<Option<HashMap<u32, GameState>>> = Mutex::new(None);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// 取实例表的锁。**用 `Option` 包一层而不是 `HashMap::new()` 常量**：
/// `Mutex::new(HashMap::new())` 不是 const fn，静态初始化过不了编译。
fn with_games<T>(f: impl FnOnce(&mut HashMap<u32, GameState>) -> T) -> Option<T> {
    let mut guard = GAMES.lock().ok()?;
    let map = guard.get_or_insert_with(HashMap::new);
    Some(f(map))
}

/// 命令分发（同步）。`cmd` / `args_json` 都是 UTF-8 的 NUL 结尾 C 字符串。
///
/// 返回值是**堆上的 C 字符串**，调用方用完必须交给 [`goptop_free`] 释放——
/// 这是唯一的内存所有权约定，C++ 侧漏释放就是稳定泄漏（每次落子泄漏几十字节，
/// 长对局下会累积）。
///
/// 任何解析失败都退化成一条合法的 JSON（`{"error":...}` / `null`），**不 panic**：
/// panic 跨 FFI 边界是未定义行为，且 release profile 是 `panic="abort"`，
/// 会在鸿蒙侧表现为整个应用闪退。
///
/// # Safety
///
/// 两个指针都必须指向有效的 NUL 结尾 C 字符串（或为 null，此时按空串处理）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn goptop_call(cmd: *const c_char, args_json: *const c_char) -> *mut c_char {
    let cmd = unsafe { cstr(cmd) };
    let args_raw = unsafe { cstr(args_json) };
    let args: Value = serde_json::from_str(&args_raw).unwrap_or(Value::Null);
    let reply = dispatch(&cmd, &args);
    // CString::new 失败只可能是串里含 NUL——我们的 JSON 不会有，兜底成 "null"
    CString::new(reply).unwrap_or_else(|_| CString::new("null").unwrap()).into_raw()
}

/// 释放 [`goptop_call`] 返回的字符串。
///
/// # Safety
///
/// `p` 必须是 `goptop_call` 返回且**尚未释放**的指针（或 null）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn goptop_free(p: *mut c_char) {
    if p.is_null() {
        return;
    }
    // 交回 CString 令其按 Rust 的分配器析构——不能用 libc::free，
    // 两者分配器不一定同源
    drop(unsafe { CString::from_raw(p) });
}

/// 读一个 C 字符串（null → 空串）。
///
/// # Safety
///
/// `p` 为 null 或指向有效的 NUL 结尾 C 字符串。
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// 取字符串参数（缺失/类型不符 → 空串）。
fn sarg(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// 取 u32 参数（缺失/类型不符 → None）。
fn narg(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|v| v as u32)
}

/// 分发主体：返回待序列化的 JSON 字符串。
fn dispatch(cmd: &str, args: &Value) -> String {
    let v: Value = match cmd {
        "game_new" => match json_api::new_game(&sarg(args, "kindJson")) {
            Some(state) => match with_games(|m| {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                m.insert(id, state);
                id
            }) {
                Some(id) => json!(id),
                None => Value::Null,
            },
            // 非法尺寸组合：与 wasm 侧一致返回 null，让前端走「建局失败」分支，
            // 而不是让核心层的断言掀翻调用方
            None => Value::Null,
        },
        "game_drop" => {
            if let Some(id) = narg(args, "id") {
                with_games(|m| {
                    m.remove(&id);
                });
            }
            Value::Null
        }
        "game_reset" => {
            if let Some(id) = narg(args, "id") {
                with_games(|m| {
                    if let Some(s) = m.get_mut(&id) {
                        s.reset();
                    }
                });
            }
            Value::Null
        }
        "game_board_size" => match with_game(narg(args, "id"), |s| s.kind.size() as u64) {
            Some(n) => json!(n),
            None => json!(0),
        },
        "game_state_json" => match with_game(narg(args, "id"), |s| json_api::state_json(s)) {
            Some(j) => serde_json::from_str(&j).unwrap_or(Value::Null),
            None => Value::Null,
        },
        // 下面四条走 json_api 的字符串契约（PlaceResult / 计分结果），
        // 解成对象返回——契约本身仍只有一份，只是宿主层把字符串解开了
        "game_place" => {
            let (x, y) = (narg(args, "x").unwrap_or(0) as u8, narg(args, "y").unwrap_or(0) as u8);
            parse_or_err(with_game_mut(narg(args, "id"), |s| json_api::try_place(s, x, y)))
        }
        "game_pass" => parse_or_err(with_game_mut(narg(args, "id"), json_api::pass)),
        "game_undo" => parse_or_err(with_game_mut(narg(args, "id"), json_api::undo_last)),
        "game_score" => {
            let dead = sarg(args, "deadJson");
            parse_or_err(with_game(narg(args, "id"), |s| json_api::score(s, &dead)))
        }
        "game_adopt" => {
            let dead = (sarg(args, "boardJson"), sarg(args, "toMove"), sarg(args, "winner"), sarg(args, "historyJson"));
            match with_game_mut(narg(args, "id"), |s| json_api::adopt(s, &dead.0, &dead.1, &dead.2, &dead.3)) {
                Some(ok) => json!(ok),
                None => json!(false),
            }
        }
        "ai_analyze" => {
            let req: Result<goptop_ai::AnalyzeRequest, _> = serde_json::from_value(args.clone());
            match req {
                Ok(r) => serde_json::to_value(goptop_ai::analyze(&r)).unwrap_or(Value::Null),
                Err(e) => json!({ "error": format!("bad request: {e}") }),
            }
        }
        "ai_warmup" => {
            goptop_ai::gomoku::warmup();
            Value::Null
        }
        // 未知命令不静默：鸿蒙侧命令名打错时，界面上的表现是「功能没反应」，
        // 留一条可读的回执比查三天强
        other => json!({ "error": format!("unknown command: {other}") }),
    };
    serde_json::to_string(&v).unwrap_or_else(|_| "null".into())
}

fn with_game<T>(id: Option<u32>, f: impl FnOnce(&GameState) -> T) -> Option<T> {
    let id = id?;
    with_games(|m| m.get(&id).map(f)).flatten()
}

fn with_game_mut<T>(id: Option<u32>, f: impl FnOnce(&mut GameState) -> T) -> Option<T> {
    let id = id?;
    with_games(|m| m.get_mut(&id).map(f)).flatten()
}

/// json_api 的字符串契约 → JSON 对象；无此局时给 `no_game`（与 Tauri 侧同一错误码）。
fn parse_or_err(raw: Option<String>) -> Value {
    let raw = raw.unwrap_or_else(|| json_api::err_reply("no_game"));
    serde_json::from_str(&raw).unwrap_or_else(|e| json!({ "ok": false, "error": format!("bad json: {e}") }))
}
