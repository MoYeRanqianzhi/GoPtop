# 2026-09-13/14 四端实机验证轮 + 真 Windows 标题栏 + 六维审查修复

## 四端实机测试的拍板与结论（全部实机点击+截图验证）

1. **Windows 真标题栏**（用户拍板「必须是真实 Windows 控制按钮功能，纯 HTML 假按钮是致命问题」）：
   wry 0.55 只有子窗口宿主（无 DWM 合成宿主），可达路径 = zbrooklyn/tauri-snap-layouts 技法：
   透明 WS_CHILD 覆盖窗悬在 HTML 最大化键正上方，`WM_NCHITTEST` 恒返回 `HTMAXBUTTON`
   → OS 给真 Snap Layouts 弹层；点击/悬停**必须回传页面**（`titlebar://max-click`、
   `titlebar://max-hover` 事件 → 前端 toggleMaximize / 手动挂悬停底色）——
   decorations:false 下 root 窗口 WM_NCLBUTTONDOWN 转发是 no-op（踩过坑），
   覆盖层吞 mousemove 导致 CSS :hover 失效（踩过坑）。实现：src-tauri/src/titlebar.rs。
2. **Linux (ssh remote, openbox+xvfb)**：装饰归 WM（SSD），应用组件进不了系统标题栏是平台
   边界；三键走 Tauri API 转发（GNOME 风格 SVG 字形）。glyph 曾漏传 restore——
   `glyph()` 里 Linux 分支要写 `maximized && kind === "max" ? "restore" : kind`。
3. **Android (Studio AVD Pixel 6 API 35)**：实机五连胜/顶带/短文案全过。两个坑：
   - `__TAURI_INTERNALS__` 在 Android **非 document-start 注入**（docstart 探针实测 no），
     首渲染 isTauri()=false 且不再重渲——PosterStrip 分支必须按 UA 平台判定而非 isTauri。
   - 截图与触控同坐标系（wm size），但从缩放截图估棋盘坐标会差半格——用 CDP 读
     `svg.getBoundingClientRect()` 换算才是权威。
4. **HarmonyOS (DevEco 模拟器 Mate 80, HarmonyOS 6.0)**：Tauri 无 OHOS target →
   ArkTS Web 组件壳（harmony/ 工程）：虚拟域名 https://appassets.goptop +
   onInterceptRequest 映射 rawfile（带扩展名→静态资源，否则 SPA 回退 index.html）。
   - **模拟器接受未签名 HAP**（无需华为账号/DevEco 自动签名）。
   - 调试用 `webview.WebviewController.setWebDebuggingAccess(true)`（静态方法，
     **不是** WebAttribute）；CDP 驱动 + 指针事件序列注入即可全流程测试。
   - ArkTS 严格模式：回调参数必须显式标 `OnInterceptRequestEvent` 类型，否则
     arkts-no-any-unknown 编译失败。
   - 触控：`uitest uiInput click` 用原生坐标（dumpLayout 的 bounds 空间）。
5. release 体积（opt-level=s + fat LTO + codegen-units=1 + strip + panic=abort）：
   Windows exe 3.35MB / NSIS 1.27MB / wasm 117KB / Linux ELF 4.3MB。

## 六维串行审查轮（#1-#6 全完成）

- 台账唯一真源：`review/2026-09-13-review-round.md`（每条 finding 状态在此推进）。
- 修复批：f64419b（P1×2+P2×3）、cd538ba（IME×2/头像预览/双解码）、d414e1a（#5 wasm 契约）。
- **#5 的核心发现**：SyncState.history 原来结构上不可表达 Pass——含 Pass 的对局
  adopt/undo 重放轮转必漂移；已升级 `(Coord|"pass")[]` + wasm pass() 绑定 +
  adopt 边界/颜色白名单校验 + new_game 预验证（panic=abort 下断言=wasm trap 白屏，
  边界必须自己挡，核心层 assert 只作内部保证）。
- E2E 基线入库 scripts/e2e/（#6 B1）：run.js 42 断言 + go-capture 7 + official-smoke 8。
  **8b 的教训**：「观战」字样在「正在连接对局观战…」提示里即刻出现，检查窗口起点过早
  会假阴——观战 RTC gathering 上限 8s，手数检查给 15s。

## 官服部署程序（free28）

`scp main.rs → free28:/opt/goptop-server/goptop-server/src/`（独立 cargo 工程）→
`~/.cargo/bin/cargo build --release` → `sudo systemctl restart goptop-server` →
`scripts/e2e/official-smoke.js`（8 断言，wss 名册/邀请/对局/同步）。本轮部署后 8/8 全绿。

**Why:** 四端验证与审查修复的结论和坑位都是反复实机调试得出的，下一端开发/回归直接复用；
官服部署程序不在任何文档里，只在记忆。

**How to apply:** 新端适配先读本文对应小节；改信令/协商/观战/服务器后跑
`scripts/e2e/run.js` + 部署官服后跑 `official-smoke.js`；改 crates/goptop-core 后
重跑 scripts/build-wasm.sh 并提交产物。
