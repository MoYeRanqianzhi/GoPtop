# P2P 协议规格（agent 开发文档）

> GoPtop 无服务器 P2P 的全部线格式与流程，按当前实现（commit e00b561）逐条记录。改协议必须同步改本文与 `transport.ts` 头注释。

## 1. URL 格式（分享面）

| 链接 | 格式 | 说明 |
|---|---|---|
| 邀请 | `/<inviterId>?pwd=<key>&kind=gomoku\|go&size=<9\|13\|15\|19>[&rtc=<G1offer>]` | `rtc` 为邀请者预生成 offer（跨设备一键直连）；同源不需要 |
| 回执 | `/<inviterId>?pwd=<key>&rtcAns=<G1answer>&game=<gameId>&kind=<kind>&size=<size>[&spec=1]` | 受邀者→邀请者的 answer 回传；`game/kind/size` 让邀请者凭回执即可进受邀者的 channel；`spec=1` 标记观战回执（自动识别用，观战连接待实现） |
| 观战 | `/watch/<gameId>` | 目前仅同源可用 |
| 主页 | `/<userId>` | 无 pwd，可手动挑战 |

- 基地址：`shareOrigin()` 自动决定——Web=当前 origin；Tauri=`SHARE_ORIGIN_NATIVE`（`https://goptop.pages.dev`，改常量一处即全局）。**禁止做成用户设置**（用户拍板，见 memory/2026-09-07）。
- 解析：`parsePastedLink(text)` / `parsePastedAnswer(text)` **与域名无关**——只取路径段与 query；无协议前缀自动补 `https://`。旧 query 风格（`?u=` `?room=` `?watch=`）兼容。
- 安全不变量：含 `rtcAns` 的 URL 被当页面打开时绝不触发 acceptInvite（防同机两窗互弈）。

## 2. 信令编码（`G1` token）

```
encodeRtcPayload(payload, pwd):
  json = JSON.stringify({s: sdpString, t: "offer"|"answer", r: "player"|"spectator"})
  bytes = utf8(json) → deflate-raw
  token = "G1" + base64url( xor(bytes, keystream(pwd)) )
```

- `keystream(pwd)`：pwd 字节经 FNV 系双状态（h1=0x811c9dc5×0x01000193，h2=0x1b873593×0x85ebca6b）逐字节扩散；每输出字节 h1=imul(h1,0x01000193)^(h1>>>15)，h2=imul(h2,0x85ebca6b)^(h2>>>13)，byte=(h1^h2)&0xff。
- **混淆级非密码学级**（pwd 公开在同一链接）。真实安全：WebRTC DTLS。
- 版本头 `G1`：解码时校验前缀，不符抛错 → 上层弹窗报「无法识别」。
- 兼容性：无。旧明文 base64 链接作废（alpha 阶段无兼容包袱）。

## 3. 对局消息（GameMsg，JSON over BC + DataChannel）

```ts
GameMsg = { seq: number; sender: string(uuid); userId: string; kind: MsgKind }
MsgKind = Hello{kind,size} | Move{move: Place{coord}|Pass|Resign; by: StoneColor}
        | SyncState{board,toMove,winner,history,lastMove,kind,size}
        | SyncRequest | Chat{text} | Ping | Pong
        | Reset{kind,size} | UndoReq | UndoAck{ok}
```

`Move.by`：发送端声明的执子颜色（2026-09-07 协议变更）。接收端一律按 by 判定
（Place/Pass 须 by===本地 toMove 才应用；Resign 胜者=by 的对手），**不得**从本地
toMove/myColor 推断——观战者没有"我的颜色"。无 by 的旧格式消息直接丢弃。

发送（唯一口）：`transport.send()` → BC postMessage + `DirectRtc.broadcast`（App 注入 `wireRtcBroadcast` → 所有 open RTC peer）→ **同 seq 双通道**。
接收：`GameChannel.dispatch()` → `Move` 按 `(sender, seq)` 单调去重（lastSeq Map）；其余不去重。
未接线：Chat/Ping/Pong/UndoReq/UndoAck 收到即忽略；Pass 无 UI 入口。

## 4. 建连流程

### 4.1 同源（BroadcastChannel 可达）

```
inviter: createInvite → waiting, offer 编进邀请 URL
invitee: 开邀请 URL → processIntent → acceptInvite(inviterId,pwd,k,s,rtc)
       → acceptOffer → answer
       → presence.challenge(inviterId,pwd,k,s,inviteeGameId,ans)
inviter:  drain 队列见 pwd 匹配 → acceptChallenge → applyAnswer(ans,pwd)
       → join(inviteeGameId) → playing → presence.accept
invitee: accept 信件 → enterPlayingAsInvitee（幂等；RTC open 也会触发它）
```

### 4.2 跨设备（presence 不可达）

```
inviter: 同上，等回执
invitee: 打开/粘贴邀请链接 → acceptOffer → answer → 回执弹窗（1.2s 后仍未 open 才弹，
       弹前 answerBackUrl 已生成）
       受邀者复制回执链接发回（任何渠道）
inviter: 「输入回执」弹窗粘贴 → parsePastedAnswer → acceptReceipt:
       校验(inviterId==me && pwd==本局) → applyAnswer(ans, pwd)
       → join(invitee.gameId) → playing（pwd 置 null）
invitee: RTC open → enterPlayingAsInvitee
```

### 4.3 连接建立后的同步

`attachPeer` 的 onState("open")：setPeerConnected(true)；受邀者若 waiting 则进对局；200ms 后双方各发一次 SyncState（全量）。**A3 覆盖守卫（已修，commit be7fac7）**：接收端以 `history.length` 作快照版本，比本地旧的快照直接丢弃——open 后本方先落子不再被对方空板快照吞掉；同长快照正常覆盖。closed/error 的 peer 从 rtcPeersRef 出列。

## 5. 身份与钥匙

- `userId`：`sessionStorage.goptop:tabUser`，每标签页一个（跨页即新身份，设计如此）。
- `myId`（GameChannel.sender）：crypto.randomUUID，每次页面加载随机——这是去重按 sender 隔离、且重载后 seq 归零不撞车的前提。
- `pwd`：`genPwd()` 6 位 base36（crypto.getRandomValues CSPRNG），createInvite 生成；两人进局即作废。回执/URL 自动挑战都校验它，且是 G1 编码密钥。
- 邀请链接时序：createInvite 后台异步生成 offer，**生成成功前不提供可复制链接**（防发出无 rtc 的废链接）；失败退化为无 rtc 同源链接并如实提示。
- `gameId`：**受邀者生成**（acceptInvite），对局 channel 名 `goptop-game-<gameId>`；邀请者 acceptChallenge/acceptReceipt 时切换过去。观战 channel 同名复用。

## 6. STUN

`stunServers()` 读 localStorage（`goptop:stun`）里启用的线路。内置：国服A `stun.miwifi.com:3478`（开）、国服B `stun.chat.bilibili.com:3478`（开）、外服 `stun.l.google.com:19302`（关）。**不配 TURN**——设计红线，不得添加。

## 7. 代码结构（D1 拆分，commit 60fc02e+99fbb4c）

- `state/useGameSession.tsx`——对局状态机+信令编排唯一所在（原 App.tsx 主体）。
- `App.tsx`——纯壳：header/页面拼装/footer。
- `pages/components.tsx`——PeerList/StunSettings/RtcStatusLine/BoardPanel；`pages/LocalPage.tsx`——本地对战页。
- `game/board.ts`——checkFive/emptyBoard 前端唯一实现（改规则须与 crates/goptop-core 语义对齐）。
- `net/transport.ts`——P2P 全部底层；`encodeRtcPayload`/`decodeRtcPayload` 已导出（协议核心纯函数）。
- 测试：`frontend/src/net/transport.test.ts`（vitest 30 例，`npm test`）——去重/编码往返/解析/genPwd。

## 8. 已知限制（与 docs/已知限制与路线图.md 对应）

- 跨设备观战未实现（/watch 仅同源；`DirectRtcPeer.role="spectator"` 类型已备、流程未接）。
- 断线无重连；围棋无提子/禁自杀/数目；Undo/Chat/Pass 未接线。
