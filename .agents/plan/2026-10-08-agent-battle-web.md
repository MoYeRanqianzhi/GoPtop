# 「Agent 对战」阶段⑤——Web 端内置模式契约（探路产出）

> 权威规格：`.agents/plan/2026-10-08-agent-battle.md`（阶段⑤/R9 与「LLM 客户端/记忆库」节）。
> 本文是阶段⑤实施前的**逐字契约**：实现与审查逐条对齐本文；与主计划冲突处，本文为阶段⑤的细
> 化与修正（偏差清单见 §6）。
> 探路证据全部为 2026-10-08 一手读取/实测，file:line 以当日 master（96f149b）为准。

## 0. 一页结论

| 问题 | 结论 | 关键证据 |
|---|---|---|
| Q1 tokio::sync::watch 在 wasm32 | **可用**（`default-features=false, features=["sync"]`） | tokio-1.53.1 `src/lib.rs:462-480` 白名单含 sync；watch 依赖链纯 std（§1.1） |
| Q1 tokio::time 在 wasm32 | **不可用（panic）**，`full` 在 wasm 直接编译失败 | `src/lib.rs:440-443,479`、`tests/time_wasm.rs:13-16,57-64`；本机实测 §1.2 |
| Q2 transport 会话注册表 | **单全局槽**（`SESSION: RefCell<Option<Rc<RefCell<Core>>>>`），IO 回调全部经 `queue_event` 进全局槽 | `goptop-transport/src/lib.rs:24-26,433-439`；rtc/ws/bc 各回调 §2.1 |
| Q3 agent 的 wasm 切割 | goptop-agent 加 `cfg(target_arch="wasm32")` 目标表 + 自带 js-sys/web-sys（不复用 transport 内部件）；trait 泛化 `PlatformHost`；六处 sleep 换 `delay()`；Instant 三处换 `now_ms()` | 逐点 §3 |
| Q4 前端接缝 | 新增 `createDetachedSession`/`agentId()`；启用判定 `!isTauri() && !harmonyHost() && wasm 导出在 && goptopVfsCall 在`；AgentPage 九处翻转点 | §5 |
| Q5 e2e | mock-llm CORS 已齐零改动；agent-entry.js 逐条翻转清单 §5.4；agent-builtin.js 加浏览器模式；A'/B **同页**配对（BC 同源同页可达） | §5 |

**环拓扑（先钉死，防依赖环）**：

```
goptop-net ──▶ goptop-core            （不动）
goptop-transport ──feature "agent"──▶ goptop-agent ──▶ goptop-net/core
      │                                    │
      └── wasm32: 依赖 goptop-agent        └── native: 依赖 goptop-transport-native（现状不变）
          （agent 导出面/Hub/B 席在此）         （wasm 目标下该依赖被 cfg 摘除）
```

goptop-transport **依赖** goptop-agent（feature "agent"，wasm 目标专用）；goptop-agent **绝不**反向依赖
goptop-transport——`cargo` 不允许环，且 agent 的 js 依赖面（fetch/delay/钩子宿主）本就该内聚在
goptop-agent。transport 的 `storage_hook/rand_u32/window` 都是 `pub(crate)`
（`goptop-transport/src/lib.rs:41-65`），为复用而改 pub 是为单一消费者扩公共面，**不做**。

---

## 1. Q1：tokio::sync::watch 与 tokio 在 wasm32 的可用性

### 1.1 watch 可用（逐依赖证据，tokio-1.53.1 本地 registry 源码）

- **feature 白名单**：`src/lib.rs:462-480`——非 unstable 且 `target_family="wasm"` 时，启用
  `fs|io-std|net|process|rt-multi-thread|signal` 任一即 `compile_error!("Only features
  sync,macros,io-util,rt,time are supported on wasm.")`。`sync` 在白名单内。
- **watch.rs 的依赖闭包**（`src/sync/watch.rs:165-174`）：`crate::sync::notify::Notify` +
  `crate::task::coop::cooperative` + `crate::loom::sync`（非 loom 构建即 std 的
  Arc/RwLock/AtomicUsize）+ 纯 std（fmt/mem/ops/panic）。无 time、无 net、无 rt。
- **Notify 纯 std**（`src/sync/notify.rs:8-18`）：loom Mutex/AtomicUsize + `util::linked_list`
  + `std::future::Future`。`Notified` 的 waker 就是执行器给的 std Waker——
  `wasm-bindgen-futures::spawn_local` 的微任务执行器可直接驱动。
- **`thread_rng_n` 不构成 rt 依赖**：watch 的 `BigNotify::notified` 随机化分支
  （`watch.rs:418-423`）仅在 `any(feature="macros", all(feature="sync", feature="rt"))` 下编译
  （`src/runtime/context.rs:130-138`）；wasm 目标只开 `sync` 时走**循环取模回退**分支
  （`watch.rs:400-408`），完全不碰 `crate::runtime`。
- **`async_trace_leaf` 是空操作**：非 taskdump 构建下 `trace_leaf()` 恒回 `Poll::Ready`
  （`src/lib.rs:585-600`）。

### 1.2 time 不可用（编译失败 + 运行时 panic 双重证据）

- `src/lib.rs:440-443`：「The timing functions will panic if used on a `WASM` platform that does
  not support timers.」
- 自家测试 `tests/time_wasm.rs:13-16` `instant_now_panics`（`#[should_panic]`）、`:57-64`
  `sleep_panics_on_unknown_unknown`（`#[should_panic]`），注释「should remove this once time is
  supported」。
- **本机实测**（rustc 1.98.1，`--target wasm32-unknown-unknown` cdylib 探针 + node 执行）：
  `Instant::now()` 即 trap（panic=abort → unreachable）。`std::time::Instant` 在
  wasm32-unknown-unknown 上**不可用**；`std::thread::sleep` 同理不可依赖。
- `features=["full"]` 在 wasm 目标**编译失败**（full 展开 net/process/fs/rt-multi-thread → 命中
  lib.rs:479 compile_error）。本仓现状 `cargo check -p goptop-agent --target
  wasm32-unknown-unknown` 的第一报错是 `getrandom` 的 compile_error——其唯一来源是
  goptop-transport-native 的 webrtc 链（`cargo tree -i getrandom@0.2.17` 全部经 rtc-* 指向
  goptop-transport-native）；cfg 摘掉该依赖后 getrandom 整链消失，**无需**加 `getrandom/js`。

### 1.3 决定

- **watch 保留**：wasm 目标 `tokio = { version = "1", default-features = false, features = ["sync"] }`。
  `EmitWatch::changed()`（`goptop-agent/src/player.rs:144-147`）的 await 由
  `spawn_local` 驱动，语义零改动。
- **time/net/spawn 全禁**：下述 `delay()`/`now_ms()` 抽象（§3.2）；`tokio::spawn` 在 wasm 一律换
  `wasm-bindgen-futures::spawn_local`；`tokio::select!`（registry.rs:698，仅 MCP 用）整体
  cfg(feature="mcp") 门外置（§3.4），wasm 不引 `macros`。
- **首日验证门**：W1 完成切割后跑 `cargo check -p goptop-agent --target wasm32-unknown-unknown`
  （零 feature）必须过——这是 §3 全部切割项的总闸。

---

## 2. Q2：goptop-transport（wasm cdylib）现状与多会话改造

### 2.1 现状（证据）

- **单全局槽**：`SESSION: RefCell<Option<Rc<RefCell<Core>>>>`（`src/lib.rs:24-26`）；
  `WasmSession::new` 构造即顶掉旧值（`lib.rs:163`）；`queue_event` 只投全局槽
  （`lib.rs:433-439`）；`start_pump` 的 50ms 闭包也只泵全局槽（`lib.rs:418-429`）。
  **生命周期**：全局槽永不清空（被替换即旧 Core 失联），JS 侧 `free()` 只 drop 绑定对象。
- **IO 回调全部全局路由**：`queue_event` 的全部调用点——`bridge.rs:123`（offer 回喂）、
  `bridge.rs:150`（answer 回喂）、`bridge.rs:209`（Timer）、`io/mod.rs:29-49`（presence 三事件）、
  `io/rtc.rs:54,203,213,225,235`（ICE/DC open/close/message/竞态补发）、`io/ws.rs:47-118`
  （服务器 WS 五事件）。多数闭包已捕获 `core` 但只 `let _ = &core2;`（rtc.rs:204 等）——**按核
  路由的管道早已预埋，只是没接**。
- **导出面**：`WasmSession`（constructor `new(cfg_json)`）四十余个命令方法 + `drain`（50ms 泵 +
  命令后同步调）+ `snapshot/state_json/ice_debug/state_debug/parse_link/parse_answer`
  （`src/lib.rs:126-430`）。
- **deps**（`goptop-transport/Cargo.toml`）：wasm-bindgen/js-sys/wasm-bindgen-futures 0.4、
  console_error_panic_hook、web-sys 0.3（features 清单见该文件 27-39 行，无 fetch/Headers/
  RequestInit/Response）。
- **谁在消费 transport**：只有前端 wasm 构建（`scripts/build-transport.sh`）——native 无消费者，
  加 feature/改内部不影响任何 native 产物。

### 2.2 改造契约（feature "agent" 内外的分界）

**（a）per-core 路由修复（无条件改动，不属 agent feature）**

- `Core` 增两字段：`on_change: Option<js_sys::Function>`、`suppress_nav: bool`
  （默认 None/false，主会话路径零行为变化）。
- `queue_event(ev)` → `queue_event_to(core: &SharedCore, ev)`：以上全部调用点改为把事件推入
  **所属 core** 的队列（编译器牵出全部 call sites；闭包里的 `core` 已在手）。`bridge.rs` 的
  `Effect::Emit` → 有 `on_change` 调它，否则回落全局 `notify_change()`；`Effect::Nav` →
  `suppress_nav` 为真则跳过。
- `WasmSession::start_pump` 改为捕获**自身** core（`lib.rs:418-429` 的 `SESSION.with` 改为
  `self.core.clone()` 入闭包）。主会话语义不变（它泵的就是全局那一份 Core）。
- 回归门：现有 e2e（shell-pair/receipt-retry/agent-builtin 壳模式等）全绿即证无回归。

**（b）feature "agent"：detached 会话 + WebPlayer + Hub + 导出面**

- **A'（人的专用会话，JS 所有）**：新增静态导出
  `WasmSession::new_agent(cfg_json: &str, on_change: Option<js_sys::Function>, suppress_nav: bool) -> WasmSession`
  ——与 `new` 逐字同流程（identity/sessionStorage、presence、Boot、首 drain），**三处不同**：
  ①不写全局 SESSION 槽；②emit 走 `on_change`（Some 时）；③Nav 受 `suppress_nav` 拦。
  另加导出 `agent_id(&self) -> String`：返回它在 FRONT 注册表的 id（u32 的十进制串）。
- **FRONT 注册表**：`FRONT: RefCell<HashMap<String, Weak<RefCell<Core>>>>`，`new_agent` 自动登记
  （单局互斥由 Hub 保证，同时至多一个 Agent 局 → 至多一个 A'；旧条目弱引用自动失效）。
- **B（Agent 席，Rust 所有）**：`agent_session_new(cfg_json, hook: Arc<dyn PlatformHost>, href)
  -> WebPlayer`（crate 内 fn）——镜像 `goptop-transport-native/src/session.rs:47-86`：user_id/
  avatar/stun **全部经 hook.storage_get**（HookHost 的临时 id 与 `stun="[]"` 才能生效），server_mode
  恒 false。`WebPlayer` impl `goptop_agent::player::PlayerHandle`：`cmd`=入队+drain、
  `snapshot`=解析 `Session::snapshot()`、`pump`=drain。B 的常驻泵 = `spawn_local` 循环
  `delay(50ms)+drain`，随 Run 拆除置停机标志退出。
- **拦截面**：`CMD_GUARD` 槽（`RefCell<Option<GuardToken>>`；GuardToken 持豁免 A' 的
  `Weak<RefCell<Core>>`）。`WasmSession::cmd`（`lib.rs:237-240`）入队前判：guard 在位 &&
  `Rc::ptr_eq` 非豁免会话 && 命中开局五命令——判定表照抄 src-tauri
  （`src-tauri/src/session.rs:318-327`：`CreateInvite|AcceptInvite|AcceptReceipt|ServerChallenge|
  AcceptChallenge`）→ 调 `window.goptopNotice({"text":"Agent 对局进行中","ms":2400})` 且**不入队**。
  语义与桌面逐字一致（`src-tauri/src/session.rs:242-256`）。
- **Hub（`src/agent.rs` 新模块）**：镜像 src-tauri/src/agent.rs 的 Run 记账与状态机——
  `Run{ id, state, detail, staging, llm_calls(Atomic), tokens/tokens_out/compactions(stats_final),
  events 环≤200, abort: Arc<AtomicBool>, player, agent_color }`，表
  `RefCell<HashMap<u32, Arc<Run>>>`。配对原语镜像 `pair_seats/claim_seats`
  （src-tauri/src/agent.rs:1055-1140 的「A' 归前端、B 由壳建」形态，**不调 pair.rs::pair()**——
  pair.rs 在 wasm 被 cfg 摘除，见 §3.3）。
- **取消是协作式**（与桌面 `select!` 硬取消的偏差，见 §6）：wasm 单线程无并行取消点，
  `agent_stop` = 局中先 `cmd(Resign)` + `delay(600)`（RESIGN_SETTLE_MS，agent.rs:95）→ 置
  `abort` → 配对轮询与循环的既有检查点（每轮 LLM 调用前、每拍 `run.is_live()`）收口。
  不引 tokio `macros`/`Notify`。

---

## 3. Q3：goptop-agent 的 wasm 切割清单

### 3.1 Cargo.toml 目标表（逐依赖）

```toml
# 现有 [dependencies] 里的以下各项移入 native 目标表（内容逐字不变）：
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { workspace = true, features = ["full"] }
goptop-transport-native = { version = "...", path = "../goptop-transport-native" }
reqwest = { workspace = true }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
rusqlite = { workspace = true }

# wasm 目标表（新增）：
[target.'cfg(target_arch = "wasm32")'.dependencies]
tokio = { version = "1", default-features = false, features = ["sync"] }  # 不能 workspace=true（会连带 full）
wasm-bindgen = "0.2"
js-sys = { workspace = true }
wasm-bindgen-futures = "0.4"
[dependencies.web-sys]（wasm 门内）version = "0.3" features = ["Window","Headers","RequestInit","Response","console"]
```

保留在公共表（两端都要、纯 Rust）：`goptop-net`、`goptop-core`、`serde`、`serde_json`、
`async-trait`、`png`、`base64`。dev-dependencies 不动（`cargo test` 永远跑 native）。
wasm 依赖树里 getrandom 随 transport-native 摘除（§1.2），无需任何 js feature。

### 3.2 时间抽象（六处 sleep + 三处 Instant）

新模块 `src/wasm.rs`（`#[cfg(target_arch="wasm32")]`）+ `src/time_compat.rs`（或同文件 cfg 双臂）：

```rust
pub(crate) async fn delay(ms: u64);   // native = tokio::time::sleep；wasm = JsFuture(setTimeout)
pub(crate) fn now_ms() -> u64;        // native = SystemTime 差值或既有口径；wasm = js_sys::Date::now()
```

替换点（全部编译器可查）：

| 位置 | 现状 | 改为 |
|---|---|---|
| `pair.rs:238`（pump_until 拍） | `tokio::time::sleep(50ms)` | `delay(50)` |
| `pair.rs:231-232`（40s deadline） | `std::time::Instant::now()` | `now_ms()` 差值 |
| `pair.rs:335-336` | Instant（**测试代码**） | 不改（tests 只跑 native） |
| `registry.rs:764`（settle 拍） | `tokio::time::sleep(50ms)` | `delay(50)` |
| `registry.rs:691-699`（wait_events） | Instant + `select!` + sleep | 整段 cfg(feature="mcp")（§3.4） |
| `vfs.rs:1179`（settle_and_reread 拍） | `tokio::time::sleep(50ms)` | `delay(50)` |
| `agent_loop.rs:392`（空转限速拍） | `tokio::time::sleep(50ms)` | `delay(50)` |
| `llm/mod.rs:330`（退避） | `tokio::time::sleep` | `delay(RETRY_BACKOFF_MS[i])` |
| `player.rs:100,107,110` | Instant + `thread::sleep`（wait_until） | cfg(not(wasm32)) 门内保留（§3.3/§8.9） |
| `store.rs:325`（NativeStore::now_ms） | `SystemTime::now()` | 不改（随 rusqlite 留在 native 表） |

### 3.3 trait 面：PlatformHost 泛化 + wait_until 拆分

- **`PlatformHost`（goptop-agent 新有，player.rs）**：方法面与 `goptop-transport-native/src/host.rs:11-24`
  逐字镜像（storage_get/storage_set/copy/notice/nav/emit），**无 Send+Sync 界**（wasm 的
  js_sys::Function 非 Send；native 实现自动满足无碍）。
  - native 桥：cfg(not(wasm)) 下 blanket `impl<H: Host + 'static> PlatformHost for H`
    （`HookHost::wrap` 调用点签名变为 `wrap(inner: Arc<dyn PlatformHost>)`——src-tauri 现传
    `Arc<TauriHost>`/`Arc<HeadlessHost>`，blanket 使其照旧编译，**src-tauri 零改动**）。
  - wasm 实现 `WebHost`（`src/wasm.rs`）：storage_get/set 走 `window.goptopStorageGet/Set`
    钩子（store.ts:207-217 安装；缺钩子回落 localStorage——与 transport lib.rs:62-94 同款五
    行）；notice/copy 走 `window.goptopNotice/Copy`；nav 对 B 席恒吞（Hub 场景 B 不该导航）；
    emit no-op（B 的 emit 由 HookHost 覆写推 watch，内层无人消费）。
- **`PlayerHandle::wait_until` 的 wasm 切割（原案拆 `TestPlayerExt`，落地修正见 §8.9）**：
  生产代码零调用（只有 pair.rs/player.rs 测试在用，grep 证据 §探路）。拆独立 trait 无法在
  「src-tauri 一行不改」红线下落地——src-tauri 的 LogPlayer 在 `impl PlayerHandle` 块内实现
  wait_until 委托（src-tauri/src/agent.rs:1502-1508），方法出 trait 即编译错误。落地为：
  wait_until 以 `#[cfg(not(target_arch = "wasm32"))]` 门保留在 trait 内（Instant+
  thread::sleep 现文不变，理由见 player.rs 方法注）——**wasm 面仍是三方法**
  （cmd/snapshot/pump），§6 冻结面的「拆后三方法」在 wasm 目标逐字成立；native 面四方法
  照旧（NativePlayer/src-tauri LogPlayer 零改动）。
- **pair.rs 整体 cfg(not(target_arch="wasm32"))**：web 侧配对原语在 transport 的 agent 模块
  镜像 src-tauri `pair_seats/claim_seats` 的形态（A' 归前端、B 由 Hub 建；`create_invite/
  wait_invite/wait_playing` ~80 行）。理由：pair.rs 以 `NativeSession` 为硬类型，而 web 的
  A' 由前端持有、不归 Hub 建——镜像桌面壳的既有做法，不强行抽象。
- **player.rs 其余全部共享**：EmitWatch/HookHost/EventQueue/GameEvent/diff_events/
  ack_event_from_notice/event_baseline/run_event_pump——事件物化语义是全仓最微妙的一段，
  泛化到 PlatformHost 后两端一份实现，禁止 cfg 复制。

### 3.4 其余模块的 wasm 门

- `mcp.rs`：已 feature 门（现状 `#[cfg(feature="mcp")]`），wasm 目标不开 mcp，零改动。
- `registry.rs`：`execute_wait_events`（670-716 行）+ `"wait_events" =>` 分发臂（186 行）+
  `WAIT_EVENTS_MAX_SECS/WAIT_POLL_MS` 常量 → `#[cfg(feature = "mcp")]`。行为不变：in_scope
  本就拒 builtin 调 wait_events（registry.rs:196-201），门只是让 wasm 少编译 tokio select。
- `llm/web_http.rs`（新增，cfg wasm）：`WebHttpChannel` impl `HttpChannel::post_json`——
  `window().fetch_with_request_and_init(url, RequestInit{method:"POST", headers, body})` →
  `JsFuture` await → `Response.status()/text()`。**60s 超时 = fetch 与 `delay(60_000)` 竞速**
  （放弃侧 future drop 不 abort 底层 fetch，连接自然终结——接受；不引 AbortController 免加
  web-sys feature）。非 2xx 原样回 `(状态码,体)`——`post_with_retry`/`classify_status`
  （llm/mod.rs:322+）两端共享零改动。
- `store_web.rs`（新增，cfg wasm）：`WebStore { call: js_sys::Function }` impl `VfsStore`
  五个同步方法（形状见 §4）；配额检查（MAX_FILE_BYTES/MAX_NS_BYTES，store.rs:22-26）从
  NativeStore 抽成 `pub(crate) fn check_quota(...)` 两后端共用。`error` JSON → `Err(String)`。
- `src-tauri/src/agent.rs` 的 `load_llm_cfg`/`parse_staged_move`/`LogStore/LogHttp/LogPlayer`
  是壳私有（红线不可引）——web Hub 在 transport 内按同键名同 clamp 复刻（§2.2b），JSON 契约
  是冻结点、代码是镜像（偏差注记 §6）。

---

## 4. 冻结契约：`window.goptopVfsCall`（VfsStore web 桥）

**签名（同步、JSON 进出、永不 throw）**：`window.goptopVfsCall(op: string, payloadJson: string) -> string`。
返回恒为 JSON 文本；JS 实现内部 try/catch，任何异常折成 `{"ok":false,"error":"..."}`。

**op 枚举与 payload/回执形状（对齐 `store.rs:92-127` 的 VfsStore 五方法，字段名 camelCase）**：

| op | payload | 回执 | 对齐 trait |
|---|---|---|---|
| `read` | `{"ns":"builtin","path":"notes/a.md"}` | `{"ok":true,"found":true,"content":"..."}` ／ `{"ok":true,"found":false,"content":null}` ／ `{"ok":false,"error":"..."}` | `read -> Result<Option<String>, String>` |
| `write` | `{"ns","path","content"}` | `{"ok":true}` ／ `{"ok":false,"error":...}` | `write -> Result<(), String>`（配额在 Rust 侧查，§3.4） |
| `delete` | `{"ns","path"}` | `{"ok":true,"deleted":bool}` | `delete -> Result<bool, String>` |
| `list` | `{"ns","prefix"}` | `{"ok":true,"paths":["a.md", ...]}`（字典序） | `list -> Result<Vec<String>, String>` |
| `usage` | `{"ns"}` | `{"ok":true,"bytes":12345}` | `usage -> Result<u64, String>` |

- `path` 一律**存表键形态**（已剥 `/memory/` 前缀、未规范化——规范化在 Rust
  `normalize_path`（store.rs:37-84）做，与 vfs.rs:202 的分工一致）。
- `ns`：web 只有 `builtin`（MCP 不上 web）；实现不硬编码该值，透传即可。

**JS 侧（`frontend/src/net/agentVfs.ts` 新建）**：

- 模块加载即装 `window.goptopVfsCall`（镜像 store.ts:207-226 的 installHostHooks 时机与手法）；
  另暴露 `agentVfsReady(): Promise<void>`。
- 数据面 = **内存镜像 + IndexedDB write-behind**（store.ts:11-14 的既有架构同款：调用点在同步
  路径，先改内存再异步落盘）：启动从 IndexedDB 全量装载进 Map → ready；write/delete 先改
  Map 再 fire-and-forget 落 IndexedDB（失败 console.warn 留副本不阻塞——Rust 侧语义不受影响，
  会话内一致性由镜像保证）。
- **为什么不是 Promise**：`VfsStore` trait 是同步 `fn`（store.rs:88-92 已预写「web 后端经
  wasm-bindgen 同步桥时同样成立」）；Promise 版要么把 trait 改 async（牵动 native 全部实现与
  registry/vfs 调用面），要么在 vfs 里 cfg 出第二套 async 路径——两头的代价都远大于在 TS 侧
  吸收 IndexedDB 的异步。此为对任务预写「->Promise<string>」的**显式偏差**（§6 第 1 条）。
- 消费方：AgentPage（或其 import 链）`await agentVfsReady()` 后才可 `agent_start`。

---

## 5. Q4/Q5：前端接缝与 e2e

### 5.1 transport 产物与构建（`scripts/build-transport.sh`）

- 现状：`cargo build -p goptop-transport --release --target wasm32-unknown-unknown` +
  `wasm-bindgen --out-dir frontend/src/wasm/transport --out-name goptop_transport --target web`
  （脚本 7-9 行）；产物（goptop_transport.js/.d.ts/_bg.wasm）**已提交**进 git，前端动态
  import（session.ts:369-378 的单例 Promise 缓存）。
- 改为默认带 agent：`cargo build -p goptop-transport --release --target wasm32-unknown-unknown --features agent`。
  单产物策略（web 只有一份 transport wasm；agent 导出多出的体积换零加载分叉）。
  特性面：transport `agent = ["dep:goptop-agent"]`；goptop-agent 以
  `[target.'cfg(target_arch="wasm32")'.dependencies] goptop-agent = { path, optional = true }`
  挂入。串行构建纪律不变（内存纪律）。
- 前端类型：`frontend/src/wasm/transport/goptop_transport.d.ts` 由 wasm-bindgen 重新生成，
  无手工维护面。

### 5.2 session.ts / 新门面

- `GameSession` 接口加**可选** `agentId?(): string`（与 `nativeId?` 同款容错模式，
  session.ts:42-48）；`WasmSessionAdapter` 实现之。
- 新导出 `createDetachedSession(cfgJson, href, onChange, { suppressNav }): Promise<GameSession>`
  ——仅 web 后端有意义：`new mod.WasmSession.new_agent(cfg, fn, suppressNav)` 包进
  WasmSessionAdapter。native 调用点不存在（AgentPage 的 A' 在 native 走既有
  `createSession(onChange, {suppressNav})` 路径）。
- `createSession` 本体零改动（session.ts:392-402）。

### 5.3 AgentPage 翻转点（行号为 2026-10-08 master 版 AgentPage.tsx）

| 行 | 现状 | 改为 |
|---|---|---|
| 277 | `const desktop = useMemo(() => isTauri(), [])` | `const native = useMemo(() => isTauri(), [])` + 新 state `webOk`（挂载时 `webAgentAvailable()` 异步置位） |
| 351 | MCP info effect `if (!desktop) return` | `if (!native) return`（MCP 面永不启 web） |
| 539 | `bootFront` 经 `createSession(cfg, href, onChange, {suppressNav:true})` | native 走原路；web 走 `createDetachedSession(...)`（同参） |
| 580 | `if (busy \|\| !desktop \|\| ...)` | `!native && !webOk` 拒绝 |
| 592/622 | `agentInvoke("agent_start"...)` | 双通道包装（§5.3.1） |
| 602/617 | `agent_bind { sessionId: created.nativeId?.() }` | native 原样；web `{ sessionId: created.agentId?.() }` |
| 565 | `agent_status` 轮询 | 同上双通道 |
| 659-672 | `!desktop` 整页降级横幅 | 横幅仅 `!native && !webOk`（鸿蒙壳/产物未带 agent）；webOk 时不渲染横幅 |
| 各处 `disabled={!desktop}` | 设置卡控件 | `disabled={!(native \|\| webOk)}`；MCP 连接卡/驱动按钮的门保持 `native`（仅桌面） |

**5.3.1 双通道包装（落点修正见 §8.8）**：落地为 `frontend/src/net/session.ts` 的
`webAgentCall(cmd, args): Promise<string>`（与 `createDetachedSession`/`createSession`
同文件，不另建 agentWeb.ts；web-only 语义以 `web` 前缀自名）——`native`（isTauri）→
tauri invoke；web → `loadTransport()` 后 `mod[cmd](...)`。命令映射冻结（与 §5.3 的
导出面一一对应）：

```
agent_start(cfgJson) -> String   // {"ok":true,"id":N} | {"ok":false,"error"}
agent_stop(id) -> Promise<String>
agent_status(id) -> String       // 与桌面 agent_status 逐字同形（state/detail/stagedMove/llmCalls/tokensIn/tokensOut/compactions）
agent_events(id, since) -> String  // {"next":N,"items":[...]}；since 用 u32（环≤200）
agent_bind(sessionId) -> String
agent_llm_test() -> Promise<String>
```

全部 JSON 进出、跨边界不 throw（异步导出回 rejected Promise 也折成
`{"ok":false,"error"}` 后 resolve）。

**5.3.2 「web 后端可用」的最终判定（`webAgentAvailable()`）**：

```
!isTauri()                                   // 非桌面壳
&& !harmonyHost()                            // 非鸿蒙壳（links.ts；ArkWeb 虚拟域 appassets.goptop 同判）
&& await loadTransport() 成功                 // wasm 产物在
&& typeof mod.agent_start === "function"     // 产物带 agent feature
&& typeof window.goptopVfsCall === "function" // agentVfs.ts 已安装
```

鸿蒙壳显式排除：壳内 wasm 产物虽在，但钩子通道/存储面未验收，维持整页降级（主计划 R7）。

### 5.4 e2e 面

- **agent-mock-llm.js：零改动**。CORS 已齐（264-269 行：`ACAO:*`/`ACAM:POST,GET,OPTIONS`/
  `ACAH:*`/OPTIONS 204 预检），浏览器跨源 fetch 桩成立。真实 LLM 端点/key 照旧不入库，
  web e2e 一律走本桩。
- **agent-entry.js 翻转清单**（Web 启用后此脚本改测「web 内置可用」形态）：
  - 92-93 `Web 降级横幅在场` → 反转为 `webOk` 下横幅**不在场**；
  - 99-122 全部 `disabled` 断言（开始对局/棋种/执子/驱动按钮/协议/测试连接/Base URL）→ 反转为
    **enabled**（`allEnabled`）；
  - 94-97 MCP 驱动按钮/MCP 连接卡不渲染 → **保持**（仅桌面）；
  - 124-133 设置卡字段与选中态断言 → 保持；
  - 136-139 直达 /agent 降级断言 → 反转为可用形态；
  - 新增一条降级回归路径：构造 `window.goptopHost`（或挡掉 agent 导出）后横幅重新在场——
    鸿蒙降级面仍有断言可依。
- **agent-builtin.js 加浏览器模式**（`--web` 或参数翻转）：serve dist（复用 agent-entry.js 的
  serve 手法，5173 占用换 5174）+ `Endpoint.browser` + `page.addInitScript` 预置
  `localStorage["goptop:llm-config"/"goptop:llm-key"/"goptop:server-sel"]`（浏览器后端 =
  localStorage，store.ts:8-9）→ 走与壳版同一组断言：Agent 落子上盘（幽灵子先现后实子）、
  工具日志增长、聊天往返、拦截面（经 `window.__session`——useGameSession.tsx:137 的 E2E 钩子
  在 web 照常安装——发 createInvite 应被拒且出提示条）、Agent 认输终局。
- **vitest 新增**：`agentVfs.test.ts`（五 op 形状/镜像读写/write-behind/坏 payload 折 error）；
  `session.test.ts` 补 `createDetachedSession` 与 `agentId()` 用例。
- **双源注记**：A'/B 配对是**同页**两个 Core（各自 BroadcastChannel 实例、同名
  `goptop-game-{gid}`）——BC 规范下同源同页异实例互相可达，配对不跨源；5173/5174 只是
  serve 端口兜底（agent-entry.js:69-74），不参与配对。同源其它标签页会收到同 gid 频道广播，
  与既有 P2P 同源行为一致，非新增风险。

---

## 6. W1/W2 并行切分（互斥文件范围，零通信实现）

**冻结面（两 agent 共同契约，改动须回写本文）**：
`PlatformHost` 方法面（=host.rs 六方法，无 Send/Sync）、`PlayerHandle`（拆后三方法）、
`HookHost::wrap(Arc<dyn PlatformHost>)`、`window.goptopVfsCall` op 表（§4）、
`agent_*` wasm 导出 JSON（§5.3.1）、`new_agent(cfg, on_change, suppress_nav)` +
`agent_id()`、`createDetachedSession` 签名、`webAgentAvailable()` 判定、
`build-transport.sh --features agent`。

- **W1（crates/goptop-agent/** 独占）**：Cargo 目标表切割；`time_compat`（delay/now_ms）六+三
  替换点；`PlatformHost` 泛化 + native blanket；`wait_until` cfg(not(wasm32)) 门内保留
  （§8.9）；pair.rs
  cfg(not(wasm))；wait_events cfg(mcp) 门；`llm/web_http.rs`；`store_web.rs` + store.rs 配额
  helper 抽取；`src/wasm.rs`（WebHost/now_ms/rand）。验证：`cargo check -p goptop-agent
  --target wasm32-unknown-unknown` + `cargo test -p goptop-agent`（native 全绿）→ commit。
- **W2（crates/goptop-transport/**、frontend/**、scripts/e2e/**、scripts/build-transport.sh
  独占）**：per-core 路由修复 + Core.on_change/suppress_nav；`new_agent/agent_id`；
  FRONT 注册表；`src/agent.rs`（WebPlayer/Hub/配对原语/拦截面/导出面）；
  build-transport.sh；`agentVfs.ts`+`session.ts` 的 webAgent 通道（§8.8）+
  `createDetachedSession`+AgentPage 翻转；
  e2e 三脚本与 vitest。验证：`cargo build -p goptop-transport --features agent --target
  wasm32-unknown-unknown` + vitest/tsc + agent-entry/agent-builtin(web) 绿 → commit。
- 时序：W2 的 transport Rust 部分只依赖 §冻结面里 W1 已存在的 API（HookHost/EventQueue/
  LoopDeps/ToolCtx/LlmClient 等现状签名），可全并行；唯一接驳点是 `PlatformHost` 泛化后的
  `wrap()` 签名——本文已冻结其形状（`Arc<dyn PlatformHost>`），W2 直接按它写。
- 红线复核：goptop-net / goptop-transport-native / src-tauri / harmony 一行不改；wasm/ohos
  产物不受影响（build-transport.sh 产物按 §5.1 刷新属本阶段明确动作；build-wasm/ohos 冒烟照旧）。
- 文档同步：goptop-agent/src/lib.rs:25-26 的「范围红线 frontend 一行不改」措辞在阶段⑤失效，
  W1 完成切割时同步改该注释（注明阶段⑤放行 frontend 与 transport）。

## 7. 风险与不做清单

- **R-w1 wasm panic = 全页 trap**：循环/工具路径的所有失败都走 `LoopOutcome`/`RespondToModel`
  （既有纪律）；`console_error_panic_hook` 在 `WasmSession::new` 与 `new_agent` 都 set_once。
  残余风险：真 panic 仍会打死整页（与主会话现状同级，不新增防线）。
- **R-w2 `Date.now()` 非单调**（系统回拨影响 deadline）：配对 40s / settle 600ms 量级容忍秒级
  回拨；接受，不引 performance.now（省一个 web-sys feature 面）。
- **R-w3 fetch 超时竞速的连接残留**：drop 掉的 fetch future 不 abort 底层请求；60s 一次、
  重试 2 次封顶，量级无害。后续可换 AbortController（加 web-sys feature）。
- **R-w4 产物体积**：agent feature 把 loop/registry/vfs/compact/png 编进 web wasm（估
  +200~400KB gzip 前）；v1 接受（单产物换零加载分叉），超预期再拆第二产物。
- **R-w5 /memory 持久性**：IndexedDB write-behind 失败仅告警，页面存活期内一致、刷新后可能丢
  最近写入（隐私模式全丢）。记忆是最佳努力持久层，与 store.ts 的兜底哲学一致；文档如实注明。
- **不做**：MCP 上 web（永不，主计划条件编译拍板）；鸿蒙内置模式（R7 列后续）；A'/B 分标签页
  配对；双产物加载分叉；`getrandom/js`（依赖树已不需要）；wait_events/`tokio::select!` 的
  wasm 移植（MCP 桌面专属）；真 LLM 端点/key 入库（web e2e 只走 mock 桩）。

## 8. 与计划预写的偏差清单（阶段⑤口径，均已在上文钉死）

1. **`window.goptopVfsCall` 同步返回 `string`**（任务预写 `->Promise<string>`）：VfsStore 同步
   trait + store.rs:88-92 预写「同步桥」；IndexedDB 异步由 TS 镜像层吸收（§4）。
2. **取消改协作式**（桌面为 `select!` 硬取消）：wasm 单线程无 select/Notify 面，stop 走
   abort 标志 + 既有检查点 + resign 先行（§2.2b）。
3. **`agent_events` 的 `since` 用 u32**（桌面 u64）：wasm-bindgen u64 映射 BigInt，环≤200 无需。
4. **pair.rs 不上 wasm**（web 配对原语镜像 src-tauri 壳内做法）：A' 归前端的结构性差异使
   pair() 的「双向双建」形态在 web 无对应物（§3.3）。
5. **goptop-transport 增加 goptop-agent 依赖**（feature "agent"）：主计划只写「循环 wasm 驱动
   走 wasm-bindgen-futures」，未定 crate 拓扑；防环的唯一可行方向（§0）。
6. **`WasmSession::start_pump` 改 per-core 捕获 + IO 回调 per-core 路由**：主计划称「不改
   transport 一行」仅对阶段①-④成立；双会话是阶段⑤的硬前提（§2.2a）。
7. **Hub/Run/事件环在 transport 内复刻而非共享**：src-tauri 红线不可动，桌面与 web 的 Hub 是
   两份镜像实现，以 §5.3.1 的 JSON 契约为唯一冻结点（§2.2b/§3.4 末条）。
8. **webAgent 双通道包装并入 `session.ts`，不建 `agentWeb.ts`**（§5.3.1 原写新建文件、
   预写函数名 `agentCall`）：`webAgentAvailable`/`webAgentCall` 与 `createDetachedSession`/
   `createSession` 同文件落地，函数名加 `web` 前缀——纯落点/命名偏差，签名与行为照
   冻结面逐字（webAgentAvailable 四道判定 §5.3.2、webAgentCall 折叠不 throw §5.3.1），
   vitest 用例与 AgentPage 导入同源。
9. **`wait_until` 不拆 `TestPlayerExt`，以 `#[cfg(not(target_arch = "wasm32"))]` 门保留在
   `PlayerHandle` 内**（§3.3 原案）：拆独立 trait 会迫使 src-tauri 的 LogPlayer（
   `impl PlayerHandle` 块含 wait_until 委托，src-tauri/src/agent.rs:1502-1508）同步删方法，
   违反「src-tauri 一行不改」红线；cfg 门下 wasm 面仍为三方法（§6 冻结面形状不变），
   native 面多一个 native-only 必实现方法（NativePlayer 与 LogPlayer 均已实现，零改动）。
