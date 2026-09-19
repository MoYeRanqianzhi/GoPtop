# 全维度实机测试手册（给 agent 看）

> 人类文档见 `README.md` / `docs/使用指南.md`。本文只写「怎么把五端真机跑起来、
> 怎么用脚本驱动真实点击、踩过哪些坑」，供后续回归直接照做。
> 首次落地：2026-09-18 全维度实机测试轮（见 `../memory/2026-09-18-multi-device-real-test-round.md`）。

## 0. 五端与它们的驱动方式

| 端 | 规格串 | 驱动路径 | 备注 |
|---|---|---|---|
| web 桌面浏览器 | `web` / `web:<url>` | Playwright chromium，1400×950 | 静态服务 `node scripts/e2e/serve.js 5173 localhost frontend/dist` |
| web 手机浏览器 | `mob` / `mob:<url>` | Playwright `devices["Pixel 5"]`（触摸） | 窄屏布局（聊天走弹窗形态、无停靠栏） |
| Windows 桌面壳 | `cdp:http://127.0.0.1:9222` | `connectOverCDP` | 见 §1 启动要点 |
| 安卓 | `android` / `android:<serial>` | Playwright `_android` 驱动（adb） | **禁止 connectOverCDP**，见 §2 |
| 鸿蒙 | `cdp:http://127.0.0.1:9444` | `connectOverCDP`（ArkWeb inspector） | 见 §3 |

服务器统一走官服 `wss://goptopserver.meowoo.org/ws`（free28）。仿真器都在 NAT 后，
直连未必成功，但服务器模式的 relay 兜底能保证数据面通。分享链接指向
`https://goptop.pages.dev`（Cloudflare Pages 从 GitHub 自动构建，push 后自动更新；
改完前端要确认它构建完成，否则跨端「打开链接」拿到的是旧版本）。

## 1. 桌面壳（Windows Tauri）

```bash
# 构建：必须带 custom-protocol，否则编成 dev 版（加载 localhost:1420 → 白屏）
cargo build --release -p goptop --features tauri/custom-protocol

# 实例 1
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./target/release/goptop.exe
# 实例 2（自弈/同端对战用）：必须换 user data folder，否则共用一个 WebView2 进程、
# 两个页面挤在同一个 CDP 端点里分不清
WEBVIEW2_USER_DATA_FOLDER="<dir>" WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9223" ./target/release/goptop.exe
```

- 重建前必须 `taskkill //F //IM goptop.exe`（exe 被占用会「拒绝访问 os error 5」）。
- 默认窗口 1100×760 —— **改布局后务必在这个尺寸看一眼棋盘是否可点**（见 §6）。

## 2. 安卓（AVD）

```bash
/d/Android/Sdk/emulator/emulator.exe -avd goptop_test -no-boot-anim -gpu swiftshader_indirect -no-snapshot-save
adb install -r -t src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
adb shell am start -n com.goptop.app/.MainActivity
```

构建 APK（AVD 是 x86_64，只编该 ABI 省时间）：

```bash
ANDROID_HOME="D:\\Android\\Sdk" NDK_HOME="D:\\Android\\Sdk\\ndk\\27.1.12297006" CI=true \
  frontend/node_modules/.bin/tauri android build --debug --apk --target x86_64
```

- **不要用 connectOverCDP**：安卓 WebView 的 CDP 只有 page 域，Playwright 建连时会调
  `Browser.setDownloadBehavior` 直接失败。用 `_android.devices()` → `webViews()` →
  `wv.page()` 拿到真实 Page（与浏览器端点共用同一套定位器与输入事件）。
- 仿真器重启后 serial 会变（`emulator-5554` ↔ `emulator-5556`），且会留下 `offline` 幽灵条目：
  用 `adb devices | awk '/device$/{print $1; exit}'` 取在线的那台；
  `adb kill-server && adb start-server` 可清幽灵。

## 3. 鸿蒙（DevEco 模拟器）

```bash
"/d/Huawei/DevEco Studio/tools/emulator/Emulator.exe" -start "Mate 80"     # 只传名字；多传 instancePath/imageRoot 反而起不来
"/d/Huawei/DevEco Studio/tools/emulator/Emulator.exe" --help              # 忘了可以查
hdc shell aa start -a EntryAbility -b com.goptop.shell
hdc fport tcp:9444 localabstract:webview_devtools_remote_<pid>            # socket 名带 pid，每次重启都变
```

- HAP 构建：先把 `frontend/dist` 同步到 `harmony/entry/src/main/resources/rawfile/app/`
  （删旧目录再整体拷，去掉 `_redirects`），再
  `cd harmony && DEVECO_SDK_HOME="D:\\Huawei\\DevEco Studio\\sdk" node "/d/Huawei/DevEco Studio/tools/hvigor/bin/hvigorw.js" assembleHap --mode module -p product=default -p buildMode=debug --no-daemon`。
  hvigor 用 DevEco 自带的 node（`tools/node`，v18）需要时加进 PATH。
- 换进程后旧的 fport 规则会残留（socket 名已失效但端口仍被占）：先
  `hdc fport ls` 找出旧的逐条 `hdc fport rm tcp:9444 localabstract:<旧socket>`。
- ArkWeb **支持** connectOverCDP（与安卓相反），所以鸿蒙走 `cdp:` 规格。
- 模拟器接受未签名 HAP，无需华为账号。

## 4. 测试脚本（`scripts/e2e/`）

| 脚本 | 用途 |
|---|---|
| `device.js` | 端点抽象（browser / mob / android / cdp 四种构造）+ 真实输入原语 + 五子棋对局策略 |
| `match.js` | 两两对战三场景：`game`（下到分出胜负）/ `chat`（聊天+悔棋+换棋+重开+收尾完局）/ `watch`（第三方观战） |
| `matrix.js` | 矩阵批跑：内置 11 对端组合，逐对 spawn `match.js` 并汇总 |
| `features.js` | 设计功能：`local` / `go` / `challenge` / `resign`（聊天区内两步确认）/ `specchat` / `kick` |
| `diag-pair.js` / `diag-watch.js` | 配对与观战镜像诊断（逐手打印各端手数与盘面差异） |
| `run.js` / `go-capture.js` / `ui-audit.js` / `challenge-server.js` | 既有浏览器基线（见同目录 README） |

用法示例：

```bash
node matrix.js game                                  # 五端两两 11 对
node match.js chat web android                       # 聊天全流程
node match.js watch web cdp:http://127.0.0.1:9222 mob
node features.js go cdp:http://127.0.0.1:9222 android
node diag-watch.js web mob web                       # 观战镜像逐手诊断
```

**唯一不取巧的写法**：所有交互走真实输入事件（`page.mouse.click` /
`touchscreen.tap` / 键盘键入），快照（`window.__session.snapshot()`）只用于**读取与断言**，
绝不调 `window.__session.place()` 这类内部 API 直接写状态。定位不到元素时用 JS 找到
元素句柄再 `elementHandle.click()`（仍是真实点击），也别改成内部 API 调用。

## 5. 驱动层踩过的坑（写脚本前先看）

1. **隐藏的同名元素**：聊天面板与其它卡片可能同时存在多份同名结构，
   `.locator(...).first()` 会选中 `display:none` 的那份且永远不可点。
   所有选择器统一加 `:visible`；聊天相关操作先取 `.chat-modal-bg` / `.chat-dock`
   为作用域（弹窗开着时侧栏按钮会被遮罩拦截，Playwright 报 `intercepts pointer events`）。
2. **openChat 必须幂等**：聊天入口按钮是**开关**（开着再点即收起），
   弹窗形态下它还会被遮罩盖住。点之前先判 `.chat-dock` / `.chat-modal-bg` 是否可见
   （`device.js` 的 `chatPanel()` 与 `run.js` 的 `openChat()` 都是这么做的）。
   2026-09-19 改成开关语义后，run.js 里三处「盲目点入口」的写法全炸过一遍。
3. **发起协商后要收起弹窗**：移动端发起方发完请求，弹窗仍开着会盖住棋盘，
   后续落子点击全被遮罩吃掉。`closeChat()` 点遮罩空白处（面板居中且有 padding，四角必是遮罩）。
4. **落子前先 `scrollIntoViewIfNeeded`**：`mouse.click` 用视口坐标，棋盘滑到折叠线下会点空。
5. **棋盘坐标**：`svg[role="grid"]` 是正方形（`min(100cqw,100cqh)`），
   按 `viewBox / rect.width` 换算格点即可；`allowOccupied` 只影响点击守卫，不影响换算。
6. **快照读取要容忍未挂载**：刚 `goto`（打开链接）时 `window.__session` 还不存在，
   `waitSnap` 必须吞掉 evaluate 异常继续轮询。
7. **名册里有陈旧条目**：异常断开的客户端在服务器空闲超时前仍在册，按昵称定位行可能命中
   旧条目（甚至处于「对局中」按钮禁用）。测试用**每轮唯一昵称后缀**规避。
8. **棋力按执色分配**：换棋会互换执色，按端点分配会让强弱随换棋漂移。
   两档棋力（sharp/casual）已离线模拟验证能稳定收束出胜负；两档相同会一路下到满盘和棋。
9. **观战镜像有短暂滞后**：高速落子时观战者落后 1–2 手是正常的，几秒内会追平；
   断言必须轮询等待，不能立即读。

## 5.5 平台存储的落盘位置（2026-09-19 起，验证存储改动先看这里）

设置类数据（昵称/头像/服务器/STUN/默认规则）不再放 WebView 的 localStorage：

| 端 | 后端 | 落盘位置 | 怎么看 |
|---|---|---|---|
| web | `browser` | 浏览器 localStorage | CDP `localStorage.getItem(...)` |
| 桌面壳 | `tauri` | `%USERPROFILE%\.goptop\store.json` | 直接读文件 |
| 安卓 | `tauri` | `/data/data/com.goptop.app/store.json` | `adb shell run-as com.goptop.app cat /data/data/com.goptop.app/store.json` |
| 鸿蒙 | `harmony` | `/data/app/el2/100/base/com.goptop.shell/haps/entry/files/store.json` | `hdc shell cat <路径>` |

- 端内自查：`window.__store.backend()`（`browser`/`tauri`/`harmony`）与 `window.__store.dump()`。
- **写设置给壳端**（脚本预置）用 `ep.setSetting(key, value)`，别再写 localStorage——
  `device.js`/`match.js`/`features.js` 已统一（`prepShell` 走它）。
- 验证套路（四端通用）：UI 改昵称 → 读文件确认 → `localStorage.clear()` → 杀进程重启
  → 昵称仍在（证明平台文件是权威源，不是靠浏览器存储兜着）。

## 6. 布局红线（改 UI 前必读）

- **棋盘尺寸是正反馈环**：`--stack-max`（整组宽度锚点）由 `board-wrap` 的剩余高度反推，
  而卡片越窄内容换行越多、越高 →「棋盘变小 → 整组变窄 → 卡片变高 → 棋盘更小」，
  一路塌到宽度下限（桌面壳 1100×760 实测棋盘只剩 50px）。
  修法是**固定卡片高度一律在最宽整组下测量**再算棋盘可用高度。
  **不要在对局页棋盘上下再加卡片行**——每加一行都会进一步挤扁棋盘
  （围棋终局计分控件因此并入既有状态行，没有单独成卡）。
- **聊天弹窗 z-index 800 会盖住协商横幅**（ConfirmBanner 的静态 z-index 不生效）：
  有待决 `confirmReq` 且聊天处于**弹窗形态**时自动收起聊天；停靠栏在侧边不遮挡，
  收起反而打断用户（浏览器基线 E2E 曾因此回归 42→失败）。判形态只看实测结果
  （`chatDocked`），不看视口宽度。
- **聊天停靠栏宽度/形态全由实测决定**（App.tsx `measureChat` → `--chat-w`）：
  空间充足与棋盘整组等宽 → 放不下先压缩聊天栏（下棋软件，棋盘完整优先）→
  压到 240px 以下改弹窗。**`.play-stack` 不许被压窄（`flex: 0 0 auto`）**，
  否则「停靠栏挤窄整组 → 实测又把挤窄值当基准」会锁死棋盘宽度（vw=650/600 中招）。
- **聊天面板必须有「收起」入口**：停靠栏形态没有遮罩可点，此前（2026-09-19 修）
  停靠栏**根本关不掉**——面板里最像关闭的「关闭观战」是关观战功能，点了反而把观战关掉。
  现在：面板标题行常驻「收起」，棋盘旁的聊天入口按钮是**开关**（开着再点即收起）。
- **认输在聊天区内**（对局操作集合区：悔棋/重开/换棋/认输同排），不在棋盘旁的操作行——
  聊天区存在的意义就是收纳这些按钮，棋盘旁只留棋盘/终局直接相关的东西。
- 改布局后至少在 **1100×760**（桌面壳默认窗口）与 **Pixel 5（393×851）** 两个尺寸
  各看一次棋盘是否可点、按钮是否可达；聊天改动另看三档形态（≈1400 等宽 /
  ≈800 压缩 / ≈650 弹窗）。

## 7. 回归口径

```bash
cargo test                                            # net / core / server 三包
cd frontend && npx tsc --noEmit && npx vitest run
node scripts/e2e/run.js                               # 浏览器基线 58 断言
node scripts/e2e/matrix.js game                       # 五端两两 11 对（需先起壳与模拟器）
node scripts/e2e/features.js <场景> <端…>              # 设计功能
```

改动信令/协商/观战/计分/认输 → 至少复跑 `run.js` + 相关 `features.js` 场景；
改动布局 → 复跑 `run.js` 并在两个尺寸目视棋盘。
