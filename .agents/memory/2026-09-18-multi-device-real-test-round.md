# 2026-09-18 全维度实机测试轮 —— 五端两两对战 + 设计功能，7 个产品缺陷

## 测试环境与方法（下次复跑直接照做；另有专门手册 `.agents/docs/real-device-testing.md`）

- **五端**：web 桌面浏览器（Playwright chromium 1400×950）、web 手机浏览器（Pixel 5 仿真）、
  Windows 桌面壳（`target/release/goptop.exe`，两实例：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222/9223`
  + 第二实例加 `WEBVIEW2_USER_DATA_FOLDER` 隔离）、安卓（AVD `goptop_test` x86_64）、
  鸿蒙（DevEco 模拟器 Mate 80）。
- **驱动**：`scripts/e2e/device.js`（端点抽象 + 真实鼠标/触摸输入）+ `match.js`（两两对战：
  game/chat/watch 三场景）+ `matrix.js`（矩阵批跑）+ `features.js`（设计功能：local/go/challenge/
  resign/specchat/kick）+ `diag-pair.js`/`diag-watch.js`（配对/镜像诊断）。
- **接管方式**（关键差异，别再踩）：
  - 桌面壳 / 鸿蒙 ArkWeb：`connectOverCDP` 可用。
  - **安卓 WebView 不可用 connectOverCDP**（只有 page 域，Playwright 建连时调
    `Browser.setDownloadBehavior` 直接失败）——改用 Playwright 的 `_android` 驱动
    （adb 直连，拿到真实 Page，与浏览器端点共用同一套定位器）。
  - 鸿蒙 CDP：`hdc fport tcp:9444 localabstract:webview_devtools_remote_<pid>`；
    换进程后端口规则要先 `hdc fport rm` 旧的（socket 名带 pid，会残留）。
  - 桌面壳必须 `cargo build --release -p goptop --features tauri/custom-protocol`：
    不加 `custom-protocol` 会编成 dev 版（加载 localhost:1420，白屏）。
- **数据面**：全部走官方服务器（free28, wss://goptopserver.meowoo.org/ws），服务器 relay 兜底
  保证仿真器 NAT 后也能对战。分享链接指向 `https://goptop.pages.dev`（Cloudflare Pages
  从 GitHub 自动构建，push 后自动更新）。

## 本轮修掉的 7 个产品缺陷（全部有单测/实机复验）

1. **鸿蒙壳分享链接不可达**：ArkWeb 不是 Tauri（无 `__TAURI_INTERNALS__`），
   `shareOrigin()` 退化成虚拟域名 `https://appassets.goptop`，生成的邀请链接对方打不开。
   → `links.ts` 新增 `isShellRuntime()`（Tauri 或宿主为 appassets.goptop）。
2. **服务器模式粘贴邀请链接完全无效**：UI 直接调 `accept_invite`，而该函数「无 rtc offer」
   分支只发**同源 presence 挑战**（跨设备到不了），Boot 路径因 `process_intent` 提前分流到
   `pending_link` 才没暴露。→ server_mode 时同样走 `pending_link + flush_pending_link`。
3. **服务器模式无观战链接**：`spec_url` 只在无服务器分支生成，「邀请观战」复制空串。
   → server_mode 分支补 `spec_link_url`。
4. **粘贴观战链接被当对局 join**：UI 解析出 `intent.spec` 却没往下传，房主按 join 互斥拒绝，
   观众被弹回主页。→ `UiCommand::AcceptInvite` 加 `spec` 字段，走与「打开链接」相同的通道。
5. **乱序落子永久丢失（观战者最明显）**：数据面三路径到达次序不保证，后一手先到时被行棋方
   守卫拒收——而消息已记入去重表，重传副本被吃掉，这一手永久丢失。
   → `Session.pending_moves` 乱序暂存（上限 16）+ 轮次对上补应用；重开/悔棋清空。
   注意去重表是**按 sender 单调**的，测试要构造不同 sender 才对应真实场景。
6. **围棋终局整条链路在 UI 不可达**：`handlePass`/`toggleDead`/`confirmScore`/`handleResign`
   从未被任何组件使用——双 Pass 后既停不了手也标不了死子、确认不了计分，终局卡死。
   → P2pPage 补停一手/认输/计分控件；`BoardSvg` 补 `allowOccupied`（标死子必须能点已有棋子）
   与 `dead`（红叉标记）。
7. **认输判错胜负 + 手数虚增 + 对手收不到**：引擎 Resign 语义是「当前行棋方认输」而认输者
   未必在行棋；镜像把认输映射成 Pass 使手数 +1；乱序修复又把认输也轮次门控了。
   → 按认输者执色重定胜负、镜像不收录认输、认输不做轮次门控。
   另有：挑战者从「在线用户」页发起、受理到达时不跳转（补 `Effect::Nav("/p2p")`）。

## 两个布局/交互陷阱（改 UI 前务必知道）

- **棋盘尺寸正反馈塌陷**：`--stack-max` 由 board-wrap 剩余高度反推，而卡片越窄越高——
  「棋盘变小→整组变窄→卡片变高→棋盘更小」一路塌到下限（桌面壳 1100×760 实测 50px）。
  → 固定卡片高度一律在**最宽**整组下测量再算棋盘可用高度。**别在棋盘上下再加卡片行**，
  每加一行都会进一步挤扁棋盘（围棋计分卡因此改为并入既有状态行）。
- **聊天弹窗（z-index 800）会整个盖住协商横幅**（ConfirmBanner 的静态 z-index 不生效）：
  请求到了用户看不见。→ 有待决 confirmReq 时自动收起聊天**弹窗**（2026-09-19 起判据
  改为实测形态 `chatDocked`，不再是「视口 <1080px」）：停靠栏在侧边不遮挡，收起反而
  打断用户（浏览器基线 E2E 曾因此回归）。

## 本轮实际跑的测试内容与结果

**对局矩阵（11 对，每对都是真实点击下到分出胜负——本轮定稿棋力组合稳定 37 手黑胜）**：

| # | 对局 | 结果 |
|---|---|---|
| 1 | web 桌面 ↔ web 手机 | 通过 |
| 2 | web 桌面 ↔ 桌面壳 | 通过 |
| 3 | web 桌面 ↔ 安卓 | 通过 |
| 4 | web 桌面 ↔ 鸿蒙 | 通过 |
| 5 | web 手机 ↔ 桌面壳 | 通过 |
| 6 | web 手机 ↔ 安卓 | 通过 |
| 7 | web 手机 ↔ 鸿蒙 | 通过 |
| 8 | 桌面壳 ↔ 桌面壳（双实例） | 通过 |
| 9 | 桌面壳 ↔ 安卓 | 通过 |
| 10 | 桌面壳 ↔ 鸿蒙 | 通过 |
| 11 | 安卓 ↔ 鸿蒙（两个移动系统互为主客） | 通过 |

**聊天矩阵（11 对）**：每对跑完整流程——双向聊天 → 悔棋（对方同意，手数回退）→
换棋（黑白互换）→ 重开（手数归零）→ 协商后继续下到分出胜负（21–37 手）。

**观战矩阵（8 组三端组合）**：房主邀请观战 → 观众接入（浏览器走「打开链接」、
壳端走「粘贴观战链接」）→ 逐手同步（高速落子下短暂落后 1–2 手，秒级追平）→
观战在场下完整局。

**设计功能逐项验证（端 × 场景）**：

| 场景 | 覆盖端组合 | 断言要点 |
|---|---|---|
| `local` 本地对战 | web / 手机 / 桌面壳 / 安卓 / 鸿蒙 | 落子 3 手、悔棋回退、重开清盘 |
| `go` 围棋终局 | web / 桌面↔安卓 / 鸿蒙↔桌面 / 安卓↔鸿蒙 / 桌面↔鸿蒙 / 手机 | 9 路建局、提子生效（白 (4,4) 被提）、双停一手进计分、死子标记同步、双确认出结果（黑 5 : 白 9.5 → 白胜）、宣布胜者 |
| `challenge` 大厅挑战 | web / 鸿蒙↔桌面 / 桌面↔安卓 | 挑战送达对方弹窗、双方入局、入局后可正常落子 |
| `resign` 认输 | web / 桌面↔web / 安卓↔鸿蒙 / 桌面↔鸿蒙 / 安卓↔桌面 | 两步确认、认输者判负、对手收到 |
| `specchat` 观战者申请发言 | web + 桌面壳 + 鸿蒙 | 申请送达房主横幅 → 房主同意 → 转达对手 → 双方同意后观众获发言权 → 发言到达对局者 |
| `kick` 踢出观战者 | web 三端 / web + 安卓 + 鸿蒙 | 房主名册出现观众、踢出后观众回主页 |

**测试期间观察到但判定为设计行为（未改）**：服务器名册在客户端异常断开后会残留条目
直到空闲超时（测试脚本改用每轮唯一昵称规避）；观战者接入瞬间有 1–2 手镜像滞后（秒级追平）。

## 回归口径（本轮定稿）

```bash
cargo test                      # net 42 / core 35 / server 4
cd frontend && npx tsc --noEmit && npx vitest run    # 37
node scripts/e2e/run.js         # 浏览器基线（2026-09-19 起 49/49，含聊天三档几何）
node scripts/e2e/matrix.js game # 五端两两 11 对（需先起两端壳与模拟器）
node scripts/e2e/features.js <场景> <端…>            # 设计功能
```

本轮最终成绩：对局矩阵 11/11、聊天矩阵（含悔棋/换棋/重开）11/11、观战矩阵 8/8、
设计功能全端全通（local 5 端、go 6 组合、challenge 3 组合、resign 5 组合、specchat、kick）。

**Why:** 这些缺陷全部是「单测绿、UI 点不到/点不对」的类型，只有多端真实点击 + 真实下棋
才能暴露；测试环境搭建方式（尤其安卓不能走 connectOverCDP、桌面壳必须带
custom-protocol）不记下来下次要从头试错。

**How to apply:** 改信令/观战/计分/认输后按上面口径复跑；改布局后必须在
1100×760（桌面壳默认窗口）与 Pixel 5 两个尺寸各看一次棋盘是否可点。
