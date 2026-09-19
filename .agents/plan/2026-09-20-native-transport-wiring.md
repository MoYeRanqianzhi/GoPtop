# 原生端传输层接线（把 `WasmSession` 从三端换掉）

> 状态：**未开工**，等用户拍板。背景见 `.agents/TODO.md` 的遗留条目与
> `memory/2026-09-19-wasm-is-web-only-native-platforms-link-rust.md`。

## 为什么

规则与 AI 已经在桌面 / Android / 鸿蒙直连 Rust，但 **P2P 会话三端仍跑 wasm**。
证据（安卓模拟器实测，`performance.getEntriesByType("resource")`）：
`goptop_transport_bg-*.wasm` 在时间线里，而 `goptop_core` / `goptop_ai` 都不在。

`crates/goptop-transport-native` 已存在（WebSocket=tokio-tungstenite、WebRTC=webrtc-rs、
同源=进程内 hub、定时器=tokio），无头 5/5，但**没有接进任何宿主**——
`src-tauri/Cargo.toml` 只依赖 `goptop-core` 与 `goptop-ai`。

**注意口径**：逻辑本身已经是纯 Rust，差别只在编译目标与 IO 后端。所以这不是
「功能没用 Rust 做」，而是「原生端白扛了 wasm 的边界与 WebView 限制」。

## 现状的形状（before）

```
frontend/src/state/useGameSession.tsx
  import initTransport, { WasmSession } from "../wasm/transport/goptop_transport.js";
  const s = new WasmSession(cfgJson, href);   // 三端都是这一份
  s.start_pump();                              // 50ms 事件泵
  s.snapshot() / s.state_json() / s.cmd(...)
  全局钩子：goptopOnChange / goptopNotice / goptopCopy
```

`WasmSession` 的方法面（前端只依赖这些）：`snapshot()` `state_json()` `cmd()` `pump()`
`start_pump()` `parse_link()` `parse_answer()` `state_debug()` `ice_debug()`
`accept_spec_receipt()` 等，另有宿主钩子（存储 / 剪贴板 / 提示 / 导航 / emit）。

## 目标形状（after）

**一个门面、三个后端**，与 `game/rules.ts` 的 `pickBackend()` 同款：

| 端 | 后端 |
|---|---|
| Web | `WasmSession`（现状不变） |
| 桌面 / Android | `invoke` 到 `goptop-transport-native`（Tauri 命令面） |
| 鸿蒙 | `goptopHost.call(...)` 到 NAPI（同一条 NativeSession） |

## 步骤（按依赖顺序，每步都要能独立验）

### 1. Tauri 宿主（`src-tauri/src/session.rs`）

- `SessionHost` 实现 `goptop_transport_native::Host`：
  - `storage_get/set` → 复用 `store.rs`（**不要新开一份落盘**，否则门面分叉）
  - `copy/notice` → 复用现有前端钩子（`app.emit("goptop:notice", ...)`）
  - `nav` → 同上，由前端路由决定
- 多会话表 `Mutex<HashMap<u32, NativeSession>>`（形状抄 `rules.rs` 的 `Games`）
- 命令：`session_new(cfgJson, href)` / `session_drop(id)` / `session_cmd(id, cmdJson)` /
  `session_snapshot(id)` / `session_state_json(id)` / `session_parse_link(text)` …
- **状态回推**：`useGameSession` 现在是 50ms 拉一次 `snapshot()`，原生侧照搬即可——
  不要改成 Tauri event 推送，拉模式两端同构，前端 `start_pump()` 的语义不用动。

### 2. 前端第三个 transport 后端（`frontend/src/net/session.ts` 之类）

- 抽一个 `SessionLike` 接口（= 上面「方法面」那一列），`WasmSession` 与
  `NativeSession` 各实现一份；`useGameSession` 只认接口。
- 分派判据与 `game/rules.ts` 的 `pickBackend()` 一致（Tauri → 原生；鸿蒙 → 原生；
  其余 → wasm）。
- **先例**：`ai/client.ts` 的三后端就是这么分的，照抄结构。

### 3. 鸿蒙侧

难点不在 Rust（同一个 `NativeSession`），而在**异步 IO 的状态怎么回到 JS**：
javaScriptProxy **不支持返回 Promise、也不能传函数**（`ai/client.ts` 的
`analyzeHarmony` 已经踩过）。传输层比 AI 复杂——它有持续事件（对端状态、消息、
超时），不是「一问一答」。

可选路子（需先在设备上 POC 再选）：
- **A**：照搬 post/poll——`sessionPump()` 每次返回自上次以来累积的事件数组，
  JS 侧 50ms 轮询。与 wasm 侧的泵同构，**推荐先试这条**。
- **B**：ArkTS 定时器 + `webviewController.runJavaScript("window.__goptopPush(...)")`
  主动推。少一次轮询，但多一条注入路径要维护。

### 4. 宿主能力对齐

`goptop-transport-native/src/host.rs` 的 `Host` trait 目前只有 `HeadlessHost` 一个实现。
Tauri 与鸿蒙各补一个；**两个实现的行为必须一致**，否则同一份状态机会在两个宿主上
分叉（这是本项目反复踩过的坑：契约分叉不报错，只是行为不同）。

## 验证（缺一不可）

1. `cargo test --workspace`（无头 5/5 必须仍然全绿）。
2. **桌面壳 P2P 真实对局**：两个壳实例 + 本地信令服务器
   （`WEBVIEW2_USER_DATA_FOLDER` 隔离第二实例，端口 9222/9223），
   跑通「挑战 → 对局 → 落子同步」。**这一步是本次改动的核心风险点**——
   `run.js` 只跑浏览器，覆盖不到新的原生 transport。
3. `frontend` 的 vitest + `tsc`。
4. 资源时间线复核：三端都**不该**再出现 `goptop_transport_bg-*.wasm`。
5. 鸿蒙侧先在模拟器上跑 `ohos-native-probe.js` 式的探针，再跑 `ai.js` / `match.js`。

## 已知风险

- 动的是刚修好并验过的 P2P 路径（本轮修了三处根因：钥匙判定 / 回执带钥 / 槽位 tag）。
  改完必须重跑那 4 条状态机回归 + 跨端 `noserver-pair.js`。
- 传输层是**有状态且异步**的，比规则/AI 难得多；鸿蒙那条尤其。
- `goptop-transport-native` 的 `io/bc.rs` 里有一个**已知未收敛**的差异：
  wasm 的对局通道按局名隔离（`goptop-game-{gameId}`），native 只有一个全局 hub，
  且 `GameMsg` 里没有局号可过滤——同进程两个窗口若在不同局，消息会串。
  接进宿主**之前**要先把它收敛掉，否则桌面壳双开会串消息。
