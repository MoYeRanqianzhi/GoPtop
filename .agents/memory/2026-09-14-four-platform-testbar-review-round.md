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

## 补修 2026-09-19：最大化键悬停底色从不生效（非客户区消息）

用户报「放大缩小按钮鼠标放上去背景不变色」。**根因**：覆盖层 `WM_NCHITTEST` 恒返回
`HTMAXBUTTON`（非客户区命中码），系统把该窗口的鼠标输入**整体走非客户区通道**——
到达的是 `WM_NCMOUSEMOVE`(160) / `WM_NCMOUSELEAVE`(674) / `WM_NCLBUTTONDOWN`，
`WM_MOUSEMOVE`(512) / `WM_MOUSELEAVE`(675) 一条都不来。原实现只收客户区消息，
悬停分支是死代码 → `titlebar://max-hover` 从不触发 → `.wc-btn--hover` 从不挂上。
（点击本来就正常：它收的正是非客户区的 `WM_NCLBUTTONDOWN`，所以只有悬停这一路哑。）

- 修法：改收 `WM_NCMOUSEMOVE` / `WM_NCMOUSELEAVE`；`TrackMouseEvent` 的 `dwFlags`
  必须加 `TME_NONCLIENT`（只给 `TME_LEAVE` 只跟踪客户区，永远收不到离开消息，
  底色会挂着不撤）。
- 顺带修：Alt+Tab 切走时光标不离开按钮，不产生离开消息 → 失焦窗口一直亮着按钮
  （真 Windows 失焦即清）。补 `WM_ACTIVATE`+`WA_INACTIVE` 清悬停态（默认处理照走）。
- **实测证据（本机 Win11 26200 / 150% DPI）**：临时探针落盘覆盖层收到的消息，
  悬停时只有 `msg=160`（周期性的 NCHITTEST+WM_NCMOUSEMOVE）；修后 DOM 侧
  `document.querySelectorAll('.wc-btn:hover')` 为空而 `.wc-btn--hover` 命中「最大化」、
  计算背景 `rgb(43,43,43)`；屏幕取色在按钮物理区间（1840..1902）取到 #2B2B2B。
- **坐标换算坑（诊断时踩到）**：截图/MCP 屏幕坐标是物理 px，页面 CSS 坐标要乘
  `devicePixelRatio` 再加 `window.screenX/screenY`（实测 screenX*1.5 与 Rust 侧
  `GetWindowRect(overlay)` 一致）；靠缩放截图目测字形认按钮会认错（曾把最小化当最大化，
  CSS `:hover` 因此假阳性）。判定「是哪条路径生效」用 DOM 探针（`:hover` vs `--hover` 类），
  不要只看颜色变了没有。GDI `GetPixel` 返回 COLORREF 是 **BGR** 序（`303BFF` = #FF3B30）。

**Why:** 这是「单测/编译全绿但用户一眼可见」的缺陷，且第三版才定位到消息类型；
非客户区命中会改变整条鼠标消息通道这件事，是这条覆盖层路线最容易漏的一条规则。

**How to apply:** 改 titlebar.rs 后按上面 DOM 探针法复验（悬停进入/离开/失焦/最大化态
四种转换），并确认 Snap Layouts 弹层仍在（那是 OS 基于 HTMAXBUTTON 给的，别动命中返回）。

## 会话收尾状态（2026-09-14，全部已推送 origin/main 至 2554fd5）

全部完成：四端实机验证、真 Windows 标题栏、六维审查 #1-#6、修复批
（f64419b/cd538ba/d414e1a/3e98cc8/7b33d43/30d2823/181632b/2554fd5）、
E2E 入库（run 42/42 + go-capture 7/7 + official-smoke 8/8）、官服部署重启、
测试补齐（wasm 36 + vitest 49 + server 3）、Android/HarmonyOS 包已同步最终产物并装机验证。

下次继续的入口（全部在台账 review/2026-09-13-review-round.md 状态列）：
- 低危残留修复：#2 join 无互斥、双挑战 tiebreak、muted 未强制；#3 A3 直连超时
  用户可见失败、A6 自定义 STUN 校验、B5 links 解析三份拷贝收敛；#4 P2-1 非服务器
  重开绕过同意制、P2-2/2-3 聊天跨局残留/双 ChatPanel；#5 CSP null。
- 架构第二阶段：传输层 Rust 化（TODO.md，方向待拍板）。
- 鸿蒙小遗留：ArkWeb 里合成 PointerEvent 偶发丢手（tap 重试可绕过；真机触控无此问题，
  桌面 E2E 42 断言已覆盖逻辑面）——只影响 CDP 驱动测试，不影响产品。
