# frontend — 架构与数据流（agent 开发文档）

> 读者：后续贡献者的 agent。人类读者请看 `docs/使用指南.md`。
> 本文记录**当前真实实现**，随代码演进同步更新。规则：文档不许超前于代码；改代码必须同步改这里。

更新：2026-09-13（整理轮：net/state/UI 三层拆分 + 规则下沉 wasm + B1-B4 修复后）

## 架构总原则（用户拍板，2026-09-13 重申）

**Rust 承接一切核心功能；TS 只负责 UI。** 落子/悔棋/重开等一切规则判定由
`crates/goptop-core` 编译为 wasm（`frontend/src/wasm/`，经 `game/rules.ts`）在
Web 与 Tauri WebView 中执行——TS 侧禁止再建规则副本。传输层（WebRTC/信令/编解码）
当前仍在 TS（历史路径），Rust 化是路线图后续阶段（见 docs/已知限制与路线图.md）。

## 模块地图

```
frontend/src/
├── main.tsx                       入口，StrictMode 挂载 App
├── App.tsx                        纯壳：header + 页面 switch + chatDock + 弹窗组装 + footer
├── state/
│   ├── useGameSession.tsx         对局状态机编排层：states/refs、net 消息分发、
│   │                              presence/服务器通道 effect、邀请/回执/挑战流程、落子、URL 意图
│   ├── sessionContext.ts          SessionCtx 接口：三个域工厂共享的成员清单（ref 所有权在 hook）
│   ├── serverSignaling.ts         服务器模式信令域：join/offer/answer/challenge/spec-* 全套 + 观战房间管理
│   ├── negotiation.ts             协商域：悔棋/重开/换棋请求与本地执行（走 Rust 规则引擎）
│   └── chat.ts                    聊天域：pushChat/sendChat/resolvePeerName + peersRefCache
├── net/
│   ├── protocol.ts                线格式类型唯一真源（StoneColor/Coord/GameMsg/MsgKind…）
│   ├── identity.ts                myUserId/myName/genPwd/genGameId
│   ├── links.ts                   UrlIntent/parseUrl/五种链接构造/粘贴解析（域名无关）/nav
│   ├── stun.ts                    STUN 线路配置（内置 9 条，默认开 3 条）
│   ├── servers.ts                 信令服务器配置（内置官服 + 自建，单选切换）
│   ├── presence.ts                同源发现/挑战（BroadcastChannel）
│   ├── gameChannel.ts             对局数据通道（同源 BC + RTC + 服务器 relay 三链路，(sender,seq) 去重）
│   ├── rtc.ts                     DirectRtcPeer（WebRTC STUN-only 直连）+ G1 编解码（deflate+XOR+base64url）
│   ├── serverChannel.ts           信令服务器客户端（WSS，断线重连，纯转发协议）
│   ├── gameChannel.test.ts        vitest：去重/重放/跨 sender
│   ├── rtcCodec.test.ts           vitest：G1 编码往返/坏前缀/截断/错钥
│   └── links.test.ts              vitest：genPwd/parsePastedLink/parsePastedAnswer
├── game/
│   ├── rules.ts                   Rust 规则引擎门面（wasm 绑定 + TLA init + 组合守卫）
│   └── board.ts                   只剩 emptyBoard（规则禁止 TS 副本）
├── wasm/                          goptop-core 的 wasm-bindgen 产物（入库；scripts/build-wasm.sh 生成）
├── pages/
│   ├── MenuPage/P2pPage/UsersPage/SettingsPage/UserPage/WatchPage/LocalPage.tsx
│   └── components.tsx             PeerList/StunSettings/ServerSettings/BoardPanel/loadDefaults/Role/Phase
├── components/
│   ├── BoardSvg.tsx               棋盘 SVG（唯一棋盘组件；类型 re-export 自 net/protocol）
│   ├── PosterStrip.tsx            顶部黑条 + Tauri 窗口控制三键（最小化/最大化/关闭，配合 src-tauri titlebar.rs）
│   ├── BrutalCard.tsx             卡片容器（暂无调用者，见其头注释）
│   ├── StatusLamp/UrlRow/NoticeLine/KindSizePicker.tsx   App 抽出的共用 JSX
│   └── InviteModal/PasteModal/ConfirmBanner.tsx          三类弹窗
└── styles/brutal.css              新野兽派样式 + 自适应布局约束 + header 降级/容器查询（自 App 收编）
```

## 规则执行（wasm，2026-09-13 起）

- 落子唯一入口 `RulesEngine.place(x,y)` → wasm `WasmGame.try_place`：返回**权威棋盘**
  （围棋含提子）、toMove、winner；`ok:false` 附稳定错误码（occupied/suicide/out_of_bounds/game_over）。
- 悔棋 `undo_last()`：Rust 弹出一手并全量重放（提子一并还原）；TS 只镜像裁剪 history/lastMove。
- SyncState 采纳走 `adopt(board,toMove,winner,history)`：棋盘照收、历史重建供后续 undo；
  kind/size 有变时先 `newGame` 再 adopt。
- **陷阱**：Rust `GameState::new` 对非法 kind/size 组合是断言 → wasm trap 会掀翻 React 树。
  `rules.ts` 边界守卫 + `pickKind` 原子 setKind+setSize 双保险，勿拆。
- 改 core 规则后：`bash scripts/build-wasm.sh`（需 wasm32 target 与匹配版本的 wasm-bindgen-cli）
  并提交 frontend/src/wasm/ 产物——前端构建不依赖 Rust 工具链。

## 状态与 ref 双轨

所有跨回调读取的可变状态都有镜像 ref（`boardRef/toMoveRef/...`），原因：presence/RTC/服务器
回调是闭包捕获，不能信任 React state 时效；StrictMode 双挂载会拿到旧闭包。**新增跨回调状态时
必须同时建 ref 并加同步 effect，否则就重现过历史 bug（受邀者永远收不到 accept）。**

- notice 的唯一写入口是 `showNotice(text, ms?)`（审查 D8）。
- 信件队列模式：presence 回调只入队 + `setSignalTick(t+1)`，drain effect 里用 ref 消费。
- 域工厂（serverSignaling/negotiation/chat）经 `Pick<SessionCtx,…>` 解构，函数体自 useGameSession
  原样搬出；「capture once + 内部只读 ref」的语义不许改成 useCallback/useMemo。

## 路由（无 react-router）

`links.parseUrl()` → UrlIntent；`nav()` = pushState + 手动 popstate。`processIntent` 是 URL → 行动唯一
入口，三层去重：`processedIntentRef`（同 href 一次）、`inviteDoneRef`（同邀请一次）、含 `rtcAns` 的
链接被当页面打开时绝不据此挑战（防同机两窗互弈）。服务器模式 userId 链接经 `pendingLinkRef` 等
连接 ready 后发 join/spec-join（不在本地查名册判断在线）。

## 对局流程状态机

```
phase: home → waiting → playing → home
role:  idle / inviter / invitee / spectator
```

- **createInvite()**：gameId+pwd+specPwd → waiting；服务器模式直接给 `/<userId>?pwd=` 链接并建
  inviter RTC 等 join；无服务器模式后台预生成 offer 编进链接（A5：offer 就绪前不给复制）。
- **acceptInvite(inviterId,pwd,kind,size,inviteOffer?)**：受邀者生成自己的 gameId（对局 channel
  以受邀者为准）→ waiting；带 offer 则自动 answer：同源经 presence 自动回传，跨设备备回执链接
  （1.2s 延迟弹窗避免打扰已直连的场景）。
- **服务器模式受邀**：join signal → 房主本地校验 pwd（错转 wrong-pwd 弹窗）→ accept+offer →
  受邀者 `serverAcceptOffer` → answer → 房主切 playing。大厅挑战：`challenge` → 邀请弹窗 →
  `challenge-accepted` → 发起方 `serverAdmitChallenger` 建局送 offer，双方自动进对局。
- **acceptChallenge/acceptReceipt**：同源/跨设备两条平行路径，都使 pwd 失效。
- **enterPlayingAsInvitee()**：presence accept 信或 RTC open 双触发，幂等。
- **观战**：spec 链接 `/<userId>?pwd=<specPwd>&spec=1`（specPwd 每局生成、整局有效、可关闭）；
  pwd 对自动受理（spectator 直连 + spec-offer），错/无转聊天区私有申请（批准→admit）。房间名单
  双方对局者共同维护（spec-sync 广播）；踢人/禁言/关闭观战见 serverSignaling.ts。
- **backHome()**：清一切（含 modal、rtcPeers、specPwd、connLost、观战聊天权限 B4）。

pwd 生命周期：createInvite 生成 → 两人进局即 null。**对手判定用 `opponentRef`**，不许取
relayTargets 首元素（观战者可能在等待期先加入，B2）。

## 消息双通道与去重

`transport.send()` 唯一发送口：同一 GameMsg（同 seq）同时发 BroadcastChannel、所有 open 的
WebRTC DataChannel、（服务器模式）relay 兜底。接收端对有副作用类型按 (sender, seq) 单调去重；
SyncState/SyncRequest 不去重（重连 seq 归零）。**SyncState 带 `sv` 回退纪元**：悔棋/重开时本地 +1
随快照广播，接收端 (sv, history.length) 双键比较——旧守卫只比 length 会把合法回退当旧快照丢弃，
观战者从此发散（B1，E2E 9h-9j 回归覆盖）。

## 服务器连接

`serverChannel`（WSS）：hello(user_id=tabUser)→welcome(s-短ID；带 user_id 时名册用持久 ID)；
25s 心跳/60s 空闲超时；指数退避重连，重连后重放 announce。服务器纯转发（signal/relay），不解析
不存储。**服务器模式下忽略 presence 名册事件**（双写会互相覆盖）；切换服务器 = 设置页单选 + reload。

## 测试

- `cd frontend && npm test`（vitest 33 例）：去重、G1 编码、genPwd、链接解析。改协议面必须同步补。
- E2E：`%TEMP%/goptop-e2e/run.js`（Playwright，58 断言）——对局/聊天/协商/观战全链/大厅挑战/
  观战回退可见性（B1）/聊天停靠栏三档几何（13a-g）/聊天开与关两种形态（14a-i）。两个静态源（localhost:5173 跑 dist、127.0.0.1:5174）+ 本地信令服
  （`cargo run -p goptop-server -- --listen=127.0.0.1:9527`）。断言依赖 UI 文案，改卡片文案先看它。
- 浏览器实测脚本：go-capture.js（围棋提子/悔棋还原/五连）、ui-audit.js（515px 窄屏巡检）。

## 本地存储（用户拍板 2026-09-19：数据按平台规定位置落盘）

唯一门面 `frontend/src/net/store.ts`（**业务模块禁止再直接碰 localStorage**）。
启动时 `main.tsx` 先 `await storeInit()` 再渲染——门面把整表装进内存，之后
`storeGet` 读内存、`storeSet/Remove` 改内存并落盘（同步 API 是刚需：昵称/服务器/
STUN/规则/头像的读取散布在同步路径上）。

| 后端 | 判定 | 落盘位置 | 通道 |
|---|---|---|---|
| `browser` | 兜底（Web 端本体） | 浏览器 localStorage | 直接读写 |
| `tauri` | `isTauri()` | 桌面 `~/.goptop/store.json`；移动端 `app_data_dir()`（Android 实测 `/data/data/<pkg>/store.json`） | Rust 命令 `store_load/set/remove`（`src-tauri/src/store.rs`） |
| `harmony` | 存在 `window.goptopStore` | 应用 `filesDir/store.json` | ArkTS `javaScriptProxy` 注入（`harmony/.../pages/Index.ets`） |

- **wasm 侧也要走门面**：`goptop-transport` 的 `storage_get/set` 优先调用宿主钩子
  `window.goptopStorageGet/Set`（门面安装），没装才回落 localStorage。不接这一步的话
  昵称（`Effect::SetStorage`）会被 wasm 直接写进 WebView 的 localStorage，
  平台存储形同虚设——这是本次要修的「谬误」。
- `goptop:tabUser` 例外：**每标签页一个身份**，固定 sessionStorage（四端一致）。
- 首次迁移：平台侧还没有任何 `goptop:` 键、而浏览器存储里有（旧版数据）时自动搬过去，只做一次。
- 落盘失败（权限/桥异常）退回 localStorage 并 console.warn —— 留副本好过丢设置。
- 键名一律 `goptop:` 前缀；Rust 侧校验键名与值长（头像 data URL 上限 4MB），
  写入走「临时文件 + rename」原子替换。
- 读取面：`window.__store.backend()` / `window.__store.dump()`（只读，供 E2E 断言）。

## 自适应布局（不许破坏）

- `main` 必须 `overflow:hidden`（曾改成 auto 导致整组撑开，commit a4ae1d5 修回）。
- `.play-stack` 宽度 = `min(720px, 92vw, var(--stack-max))`；`--stack-max` 由 App 的 ResizeObserver
  实测 `.board-wrap` 剩余高度写入（旧 calc(100dvh-360px) 死数已废）。
- **`.play-stack` 必须 `flex: 0 0 auto`（不许被压窄）**：棋盘整组宽度是硬锚点。
  若允许 flex 压缩，聊天停靠栏会挤窄整组，而下一帧的实测又把挤窄后的宽度当基准
  → 停靠栏永久占住腾出的空间，棋盘再也回不来（实测 vw=650/600 中招）。
- `.board-wrap > .brutal-card` `aspect-ratio:1/1`；底部三卡容器查询切换（bp-wide/bp-swap）。
- header 三级降级（徽章→标题→「类型」弹出）阈值见 App 收编进 brutal.css 的注释。
- **聊天栏形态与宽度全部由实测决定**（App.tsx `measureChat`，写 `--chat-w`）：
  空间充足 = 与棋盘整组等宽 → 放不下则压缩聊天栏（棋盘完整优先）→ 压到
  `CHAT_MIN_W`(240) 以下改用弹窗。**不要再引入「视口宽度断点」**（旧 1080px
  媒体查询看不见「整组被高度压窄」这种显示不下）；停靠栏与弹窗互斥渲染。

## 历史教训索引（改相关代码前先读）

A1 回执幂等位/A3 快照守卫/A4 受理先于推进/A5 链接就绪/A7 换局清 peer/A8 拒绝信、
B1 sv 纪元/B2 opponentRef/B3 connLost 生命周期/B4 观战权限跨局残留、C4 Rust 错误枚举化、
D6 beforeunload 实例字段/D7 死 peer 出列/D8 showNotice 单入口——详见 review/archive/ 与 memory/。
