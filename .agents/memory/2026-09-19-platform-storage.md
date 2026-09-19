# 2026-09-19 平台本地存储（用户拍板：数据按平台规定的位置落盘）

用户原话要点：数据「储存在浏览器中，只是作为 Web 端的回退，其他端都有各自规定的储存区域」；
desktop 存 `~/.goptop`，移动端存「各自平台要求应用储存数据的地方」；「之前一直没有明确这些
开发，导致在开发过程中存在谬误，现在全部修复」。

## 落地形态

唯一门面 `frontend/src/net/store.ts`（业务模块禁止再直接碰 localStorage）：
启动 `main.tsx` 先 `await storeInit()` 再渲染（内存表 + 异步落盘，保住同步读语义）。

| 后端 | 判定 | 落盘位置（实测） |
|---|---|---|
| `browser` | 兜底 / Web 端本体 | 浏览器 localStorage |
| `tauri` | `isTauri()` | 桌面 `~/.goptop/store.json`；**Android 实测 `/data/data/com.goptop.app/store.json`**（Tauri `app_data_dir()` 在安卓 = 应用私有数据根） |
| `harmony` | `window.goptopStore` 存在 | `/data/app/el2/100/base/com.goptop.shell/haps/entry/files/store.json`（ArkTS `javaScriptProxy` 注入，`filesDir`） |

- `goptop:tabUser` 例外：每标签页一个身份，固定 sessionStorage（四端一致）。
- 首次启动迁移：平台侧没有 `goptop:` 键而浏览器存储里有 → 搬过去（只做一次）。
  桌面壳实测：旧 WebView localStorage 里的昵称被搬进 `~/.goptop/store.json`。
- 落盘失败退回 localStorage + console.warn；Rust 侧校验键名/值长（头像上限 4MB），
  写入用临时文件 + rename 原子替换（5 个单测覆盖缺文件/往返/无残留/坏 JSON 报错/键名校验）。

## 关键坑：wasm 侧会绕过门面直接写 localStorage

`Effect::SetStorage`（昵称保存）由 **goptop-transport（wasm）** 执行，原来直接写
`window.localStorage`——不修的话 UI 改昵称仍落在 WebView 存储里，平台存储形同虚设。
修法：transport 的 `storage_get/set` 优先调用宿主钩子 `window.goptopStorageGet/Set`
（门面安装 → 走平台存储），钩子不存在时才回落 localStorage（纯 Web / 老宿主）。
**原则**：wasm 跑在 WebView 里，凡涉及平台 IO 的都应该由宿主提供通道，别自己碰浏览器 API。

## 实测（全部真实点击 + 文件核对 + 杀进程重启）

| 端 | 证据 |
|---|---|
| Web | run.js 58/58（后端 browser，设置在重载后仍在） |
| 桌面壳 | 7/7：后端 tauri、文件生成、旧数据迁移、UI 改昵称后文件同步、**清空 localStorage 后重载仍读到**、无 .tmp 残留 |
| 安卓 | 4/4：后端 tauri、`/data/data/com.goptop.app/store.json` 内容含新昵称、**force-stop 重启 + 抹 localStorage 后仍在** |
| 鸿蒙 | 5/5：后端 harmony、桥可见、`filesDir/store.json` 内容正确、**kill -9 重启 + 抹 localStorage 后仍在** |
| 跨端 | `features.js resign web cdp:<桌面壳>` 通过（壳端配置改走 `ep.setSetting` 后仍能配对） |

**Why:** 存储位置是用户明确提出的平台约束（桌面要 `~/.goptop`、移动端要平台私有区），
之前从没实现过——所有端都靠 WebView localStorage 兜着；而且 wasm 层还自己写 localStorage，
光改 TS 门面修不干净。三条（门面 / 宿主钩子 / 各端壳注入）缺一不可，位置与验证方法不记下来
下次还会漏。

**How to apply:** 新增设置项时：加 `goptop:` 键 → 业务侧用 `storeGet/Set`（或让 Rust 发
`Effect::SetStorage`）→ 四端按 §5.5 的表核对落盘位置；脚本给壳端预置配置一律用
`ep.setSetting`。文档：`../docs/architecture.md` 本地存储节 + `real-device-testing.md` §5.5。
