# 2026-09-19 · 架构口径订正：wasm 只是 Web 端的编译目标

## 用户指出的失误

> 「wasm 是 web 端的兼容，实际上本软件是实打实的 rust，不能把 web 端的特例当成
> 全平台的准则，这是开发上的严重失误」

**确实如此**。此前（2026-09-15 那条「传输层 Rust 化方向」）的推理是：

> 四端全部运行在 WebView，web-sys 路线一份 wasm 产物覆盖四端；原生桥接只覆盖
> Tauri 系两端，Web 与鸿蒙端会被砍掉。

这个推理**把结论用错了地方**：它正确地说明了「**Web 端**必须编 wasm」，却据此让
**所有端**都跑 wasm。结果是桌面与 Android 明明有完整的 Rust native 宿主
（`goptop.exe` / `libgoptop_lib.so`），业务逻辑却绕道 WebView 去执行。

## 正确口径（现行，覆盖 2026-09-15 那条）

**核心逻辑是 Rust crate；wasm 只是 Web 端的编译目标**（浏览器跑不了原生代码），
不是全平台的实现方式。各端用各自最自然的方式接入：

| 端 | 接入方式 |
|---|---|
| Web | 编 wasm（**唯一**需要 wasm 的端） |
| 桌面 / Android | **直接 `use`**（Tauri 的 Rust 宿主就是 native，零桥接） |
| 鸿蒙 | cargo-ohos + NAPI（待做；目前仍走 ArkWeb + wasm） |

**判据**：如果一个平台能跑原生代码，它就不该跑 wasm。

## 代价（为什么这不是"洁癖"）

1. **丢 SIMD**：noru 在 wasm 上**只有标量路径**（它有 AVX2/NEON/标量三档，没有
   wasm simd128）。原生端能吃 NEON/AVX2，wasm 吃不到。
2. **多一层边界**：WebView ↔ wasm 的 JSON 序列化与跨边界调用，在原生端是纯浪费。
3. **继承 WebView 的限制**：wasm 单线程、无文件系统、无时钟——为此专门写了
   Worker 隔离、`web-time` 补丁、vendored figrid（见
   [[2026-09-19-ai-offline-engines-and-winrate]]）。这些在原生端本都不需要。

## 本轮落地

- `crates/goptop-core/src/json_api.rs`：把 JSON 契约层从 `wasm.rs` **提出来**做成
  平台无关模块，wasm 与 Tauri command 共用同一份——**契约若分叉成两份，行为会
  静默不同**（不报错，只是行为不一致）。
- `src-tauri/src/rules.rs`：规则引擎的原生宿主（`Mutex<HashMap<id, GameState>>`
  管多局，因为本地页与 P2P 页可能同时有对局）。
- `src-tauri/src/ai.rs`：AI 引擎的原生宿主。**必须 async + `spawn_blocking`**——
  同步 command 会占住 Tauri 主线程把窗口卡死。
- 前端 `game/rules.ts` 与 `ai/client.ts` 改成**双后端**（`isTauri()` 分流），
  接口统一 async（IPC 本质异步）。

## 踩到的坑

**懒加载 wasm 时必须缓存 Promise 而不是模块对象**。`init()` 是异步的，只缓存模块
的话并发调用会双双看到缓存为空、各跑一次 `init()`，wasm 被实例化两遍后内存视图
互相失效——实测报 `RuntimeError: memory access out of bounds`，**且第一个落子仍
成功、之后才炸**，极具迷惑性。原先的顶层 `await init()` 没这问题（模块只执行一次）。

## 关联

- [[2026-09-19-ai-offline-engines-and-winrate]]
- [[2026-09-15-rustification-direction-wasm-websys]]（**推理仍有效，结论已订正**）
