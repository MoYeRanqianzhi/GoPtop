# frontend — 架构与数据流（agent 开发文档）

> 读者：后续贡献者的 agent。人类读者请看 `docs/使用指南.md`。
> 本文记录**当前真实实现**，随代码演进同步更新。规则：文档不许超前于代码；改代码必须同步改这里。

更新：2026-09-07（commit 99fbb4c，D1 拆分后）

## 模块地图

```
frontend/src/
├── main.tsx                     入口，StrictMode 挂载 App
├── App.tsx                      （~540 行）纯壳：header + 页面拼装 + footer + 弹窗 JSX
├── state/useGameSession.tsx     （~900 行）对局状态机 + 信令编排（App 全部逻辑所在）
├── net/transport.ts             （~880 行）P2P 全部底层：身份/URL/Presence/GameChannel/WebRTC/编码
├── net/transport.test.ts        vitest 30 例（去重/编码往返/解析/genPwd），npm test
├── pages/components.tsx         PeerList/StunSettings/RtcStatusLine/BoardPanel/loadDefaults/Role/Phase
├── pages/LocalPage.tsx          本地对战页（自含状态机）
├── game/board.ts                checkFive/emptyBoard 前端唯一实现（与 Rust core 语义对齐点）
├── components/BoardSvg.tsx      棋盘 SVG 渲染与落子命中（唯一棋盘组件）
├── components/BrutalCard.tsx    卡片容器（暂无调用者，App 用内联 div）
└── styles/brutal.css            新野兽派样式 + 自适应布局关键约束（见下）
```

## 路由（无 react-router）

- `transport.parseUrl()` 把 `location` 解析成 `UrlIntent`（menu/local/p2p/users/settings/user?pwd&kind&size&rtc/watch）。
- `nav(path)` = `pushState` + 手动派发 `popstate`；App 监听 popstate → `setIntent(parseUrl())`。
- `processIntent(intent)` 是 URL → 行动的唯一入口，含三层去重：`processedIntentRef`（同一 href 只处理一次，StrictMode 防重）、`inviteDoneRef`（同一邀请只挑战一次）、回执 URL 短路（含 `rtcAns` 的链接被当页面打开时**绝不**据此发起挑战——历史上导致同机两窗互弈，见 memory/2026-09-07）。

## 状态与 ref 双轨

所有跨回调读取的可变状态都有镜像 ref（`boardRef/toMoveRef/...`），原因：presence/RTC 回调是闭包捕获，不能信任 React state 时效；StrictMode 双挂载会拿到旧闭包。**新增跨回调状态时必须同时建 ref 并加同步 effect，否则就重现过历史 bug（受邀者永远收不到 accept）。**

notice 的唯一写入口是 `showNotice(text, ms?)`（自带旧 timer 清理，审查 D8）；直接调 setNotice 会绕过计时管理。

信件队列模式：presence 回调只把事件 push 进 `challengeQueueRef/acceptQueueRef/rejectQueueRef` 并 `setSignalTick(t+1)`；真正消费在 drain effect 里用最新 ref 判断。回调内不做业务决策。

## 对局流程状态机

```
phase: home → waiting → playing → home
role:  idle / inviter / invitee / spectator
```

- **createInvite()**：生成 gameId+pwd → waiting → 后台 `DirectRtcPeer.createOffer(pwd)` 生成邀请链接 `rtc=` 参数。offer 生成失败不阻塞（同源仍可用）。
- **acceptInvite(inviterId,pwd,kind,size,inviteOffer?)**：生成**受邀者自己的** gameId（对局 channel 以受邀者的为准）→ waiting；若带 inviteOffer 则 `acceptOffer` 生成 answer：先 `presence.challenge(...ans)`（同源自动送达），并备好回执链接；跨设备时回执弹窗延迟 1.2s 弹出（若同源通道已送达、直连已 open 就不打扰）。
- **acceptChallenge()**（同源路径，邀请者收到 pwd 正确的 challenge 自动调用）：join 受邀者 gameId → playing → Hello+SyncRequest。
- **acceptReceipt()**（跨设备路径，邀请者弹窗粘贴回执）：校验 inviterId==自己、pwd==本局 → `applyAnswer(ans,pwd)` → join 受邀者 gameId → playing。与 acceptChallenge 是平行路径，**都**使 pwd 失效。
- **enterPlayingAsInvitee()**：两个触发源——同源 presence accept 信件，或 **RTC open 事件**（跨设备无 presence，直连一通直接进）。双触发幂等（phaseRef 判断）。
- **backHome()**：清一切（含 modal、rtcPeers、inviterRtc）。

pwd 生命周期：createInvite 生成 → 两人进局即 null（第三人不可入）。回执校验、URL 自动挑战都依赖它。

## 消息双通道与去重（关键！）

`transport.send()` 是**唯一发送口**：一条 `GameMsg` 同时经 BroadcastChannel（同源）和所有 open 的 WebRTC DataChannel（`wireRtcBroadcast` 注入的广播函数 → `rtcPeersRef` 全员）发出。**同一条消息、同一个 seq。**

接收端 `GameChannel.dispatch()` 对 `Move` 类型按 `(sender, seq)` 单调去重（`lastSeq` Map）：先到应用、后到丢弃。**只对 Move 去重**——SyncState/SyncRequest 是幂等全量同步且重连后发送方 seq 归零，去重会丢重连同步。

历史教训：早期 App 手动双发（BC 一次 + RTC 一次用 `Date.now()` 当 seq）导致同源双窗口同一手棋应用两次、手数错乱。已废除手动双发。

## offer/answer URL 编码（encodeRtcPayload）

```
JSON({s:sdp,t:type,r:role}) → UTF-8 → deflate-raw 压缩
  → pwd 派生密钥流 XOR（fnv1a 双散列 keyStream）
  → base64url 无填充 → 前缀 "G1"（版本头）
```

- 目的：SDP 压缩变短 + 链接里不出现可读 SDP（含本机 IP）。**混淆级，非密码学级**（pwd 就在同一链接里）；真正安全由 WebRTC DTLS 保证。
- `DirectRtcPeer.createOffer(pwd)/acceptOffer(token,pwd)/acceptAnswer(token,pwd)` 全 async；三处签名都带 pwd，两端必须同钥。
- `CompressionStream/DecompressionStream("deflate-raw")`：Chromium 103+/FF 113+/Safari 16.4+/Tauri WebView2，无降级。deflate/inflate 的 writer.write/close promise 已落 catch（截断流曾产生 unhandled rejection）。
- `acceptAnswer` 有 `answered` 幂等位：回执可能经 presence 与弹窗双路径同时到达；幂等位在**应用成功后**才置位（审查 A1），坏回执不锁死重试。
- **A3 覆盖守卫（commit be7fac7）**：SyncState 接收端以 history.length 作版本，旧快照丢弃。

## 测试

`cd frontend && npm test`（vitest）：GameChannel (sender,seq) 去重/重放/跨 sender、SyncState 不去重（重连 seq 归零）、G1 编码往返/坏前缀/截断/错钥、genPwd 6 位与熵抽查、parsePastedLink/parsePastedAnswer 全分支。改 transport.ts 协议面必须同步补测试。

## Presence（同源发现/挑战，BroadcastChannel `goptop-presence-v1`）

announce（2s 心跳，7s 超时）/ challenge（可带 pwd=自动同意；可带 rtcAns=同源回执）/ accept / reject / bye。**跨设备完全不可用**（这是设计：跨设备信令只走链接）。

## 自适应布局（不许破坏）

- `main` 必须 `overflow:hidden`（曾改成 auto 导致整组撑开，自适应失效，commit a4ae1d5 修回）。
- `.play-stack` 是唯一宽度锚点：`width: min(720px, 92vw, calc(100dvh - 360px), calc(100vh - 360px))`。
- `.board-wrap > .brutal-card` `aspect-ratio: 1/1`，BoardSvg `width/height:100%`，viewBox 自缩放。

## 死代码（2026-09-07 已清理）

- 旧 `game/board.ts`+`game/rules.ts`、`components/Stone.tsx`、`state/gameStore.ts` 均已删除（零引用）。`game/board.ts` 现为唯一活跃规则文件（见模块地图）。

## 待办指向

跨设备观战（A1）、回执常驻/自动识别、断线重连见 `.agents/TODO.md` 与 `docs/已知限制与路线图.md`。
