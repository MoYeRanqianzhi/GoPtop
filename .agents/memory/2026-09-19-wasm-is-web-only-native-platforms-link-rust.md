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
| 鸿蒙 | **NAPI 原生模块**（2026-09-20 落地，见下） |

**判据**：如果一个平台能跑原生代码，它就不该跑 wasm。

## 鸿蒙落地（2026-09-20）

UI 仍是 ArkWeb 壳（与桌面用 WebView2 渲染 UI 同构），但**业务逻辑不再走 wasm**：

- `crates/goptop-ohos`：Rust 侧**只出 C ABI**（`goptop_call`/`goptop_free`），
  多局实例表与 src-tauri/rules.rs 同构。
- `harmony/entry/src/main/cpp/napi_init.cpp`：**NAPI 用 C++ 写**（头文件来自 DevEco
  SDK）。在 Rust 里手写 NAPI 的 FFI 声明等于凭记忆复刻一份 ABI——`napi_module` 字段
  顺序、`napi_status` 取值、调用约定错一个字节，表现都是设备上「加载即崩」或「静默
  不注册」，没有本地复现手段。这样分层后 Rust 侧还能在宿主机上单测（8/8）。
- 前端第三个后端（`game/rules.ts` 的 `HarmonyBackend`、`ai/client.ts` 的
  `analyzeHarmony`），探测 `window.goptopNative`，探测不到回落 wasm（老壳仍能跑）。
- AI **拆成 post/poll**：`ai_analyze` 同步阻塞 0.3~3 秒，而 javaScriptProxy 的方法在
  UI 线程执行且**不支持返回 Promise**（ArkTS 的 Promise 不会被 marshalling）。计算走
  NAPI 的 async work 线程池，JS 侧轮询票号——全是同步 NAPI 能力，不依赖任何
  「某版本才有的 marshalling 行为」。

### NAPI 接入的四个真因（2026-09-20 实测）

第一版 HAP 能装能跑，但桥的每次调用都返回 null，设备日志只有一句
「napi api call fail」。四个真因分属四层，**没有一条能从那句话看出来**：

1. **构造器被 GC 掉 → 模块从不注册**。`RegisterGoptopModule` 除 `.init_array` 外没有
   引用点，而鸿蒙的构建带 `-ffunction-sections --gc-sections`，整段被回收，
   `.init_array` 剩一个全零段（`llvm-readobj` 可见，连重定位都没有）。加
   `__attribute__((used))`。
2. **DT_NEEDED 写进了绝对路径**。Rust 产物没有 SONAME，链接方把
   `G:/…/libs/arm64-v8a/libgoptop_ohos.so` 整条写进动态段，设备上不存在 → dlopen 失败。
   `build-ohos.sh` 补 `-Wl,-soname,libgoptop_ohos.so`。
3. **`.javaScriptProxy()` 是单值属性**。注册两个对象只有最后一个生效——先注册的
   `goptopStore` 直接变 undefined，日志只留「native proxy object not found」。
   ArkWeb 没有多对象重载（`web.d.ts` 只有单个 `javaScriptProxy(value)`）。
   改成**一个对象一次注册**（`goptopHost` 同时带存储面与原生宿主面）。
4. **`napi_create_async_work` 的 `async_resource_name` 不能传 nullptr**（Node 文档写明
   必填）。传空在 OHOS 上返回 `napi_invalid_arg`，表现为自抛的「无法排队 AI 分析任务」。

**排错顺序建议**：先 `llvm-readobj -d/-x .init_array` 看链接产物（1、2 都在这里），
再看设备日志。C++ 侧的 `hilog` 与 ArkTS 侧的异常栈比 ArkWeb 的转述有用得多——
`ArkCompiler: Error: <你自己的中文报错>` 会把真正的抛出点连行号一起打出来。

**模拟器是 x86_64**，只带 arm64 的 HAP 装不上（`install parse native so failed:
Abi type … does not match`）。两个 ABI 都要编、都要拷进 `cpp/libs/`。

### 工具链的坑（`scripts/build-ohos.sh`）

OHOS NDK 装在 `D:\Huawei\DevEco Studio\…`，路径带空格，而 **RUSTFLAGS 是按空白切分**的：
`-C link-arg=--sysroot=…/DevEco Studio/…` 被切成两段，clang 收不到 sysroot，于是退回
环境里的 mingw `ld.exe`（GNU BFD，PE 目标），报

```
lld: error: unknown argument: -z
```

**错的是链接器被换掉了，不是它不认 `-z`**——顺着报错去查 lld 版本会一直查不出所以然。
脚本先在 `target/` 下建一个无空格的 junction，所有路径都从它走。

同类的第二个坑：传给 clang/rustc 的路径必须是 Windows 形态（`G:/…`）。传
`/g/ClaudeProjects/…`（Git Bash 的 `pwd`）会被当成「当前盘根下的 g\ClaudeProjects\…」，
报的却是「找不到 crti.o / -lc」——看着像 sysroot 缺文件，其实是路径根本没进去。

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
