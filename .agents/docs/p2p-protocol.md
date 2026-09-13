# P2P 协议规格（agent 开发文档）

> GoPtop 的全部线格式与流程，按当前实现（2026-09-13 整理轮后）逐条记录。改协议必须同步改本文与 `frontend/src/net/protocol.ts`。

## 0. 三种链路总览

| 链路 | 载体 | 用途 |
|---|---|---|
| 同源 Presence | BroadcastChannel `goptop-presence-v1` | 名册、挑战/同意/拒绝（仅同源标签页） |
| 对局数据 | 同源 BC `goptop-game-<gameId>` + WebRTC DataChannel `goptop` + 服务器 `relay` | GameMsg 三链路同发，(sender,seq) 去重 |
| 服务器信令 | WSS `signal`（任意 kind/payload 原样转发） | userId 链接建连、大厅挑战、观战房间、trickle 之外的一切点对点信令 |

服务器（crates/goptop-server）纯转发不落地；同服务器内才能互见互连，跨服务器不通。

## 1. URL 格式（分享面）

| 链接 | 格式 | 说明 |
|---|---|---|
| 邀请（服务器模式） | `/<inviterId>?pwd=<key>&kind=&size=` | 打开即经服务器 join，pwd 由房主本地校验（错转弹窗） |
| 邀请（无服务器跨设备） | `/<inviterId>?pwd=&kind=&size=&rtc=<G1offer>` | offer 编进链接一键直连 |
| 回执 | `/<inviterId>?pwd=&rtcAns=<G1answer>&game=&kind=&size=[&spec=1]` | 房主在等待页「回执」弹窗粘贴；spec=1 为观战回执（自动识别） |
| 观战（服务器模式） | `/<userId>?pwd=<specPwd>&spec=1` | specPwd 每局生成、整局有效、可关闭；错/无 → 聊天区私有申请 |
| 观战（无服务器） | `/watch/<gameId>` | 仅同源可用 |
| 主页 | `/<userId>` | 无 pwd，可手动挑战（服务器模式发 challenge 信令，同源走 presence） |

- 基地址 `shareOrigin()` 自动决定：Web=当前 origin；Tauri=`SHARE_ORIGIN_NATIVE`（goptop.pages.dev）。禁止做成用户设置（用户拍板 2026-09-07）。
- 解析 `parsePastedLink/parsePastedAnswer` 与域名无关；旧 query 风格（`?u=` `?room=` `?watch=`）兼容；单段路径必须 `u-` 前缀或带 pwd/rtc 才识别。
- 安全不变量：含 `rtcAns` 的 URL 被当页面打开时绝不触发 acceptInvite。

## 2. 信令编码（`G1` token，无服务器跨设备专用）

```
encodeRtcPayload(payload, pwd):
  json = JSON.stringify({s: sdp, t: "offer"|"answer", r: "player"|"spectator"})
  bytes = utf8(json) → deflate-raw → pwd 派生密钥流 XOR（fnv1a 双散列）
  token = "G1" + base64url(bytes)
```

混淆级非密码学级（pwd 在同一链接）；真实安全由 WebRTC DTLS 保证。服务器模式 offer/answer
走 WSS 已有 TLS，用明文 JSON（`createOfferPlain/acceptOfferPlain/acceptAnswerPlain`）。
offer/answer 均等全量 ICE gathering（8s 超时）后才交换——**无 trickle**（2026-09-13 删除了
从未生效的 trickle 死链路）。极旧内核无 CompressionStream 会抛错，上层报「直连建立失败」。

## 3. 对局消息（GameMsg，JSON）

```ts
GameMsg = { seq: number; sender: string(uuid); userId: string; kind: MsgKind }
MsgKind = Hello{kind,size} | Move{move: Place{coord}|Pass|Resign; by: StoneColor}
        | SyncState{sv?, board, toMove, winner, history, lastMove, kind, size}
        | SyncRequest | Chat{text} | Ping | Pong
        | Reset{kind,size} | Avatar{dataUrl}
        | UndoReq | UndoAck{ok} | ResetReq | ResetAck{ok} | SwapReq | SwapAck{ok}
```

- `Move.by`：发送端声明的执子颜色，接收端一律按 by 判定，不得从本地推断（历史 bug：任何一方认输观战者都判白胜）。无 by 的旧格式丢弃。
- **落子规则判定在 Rust（wasm）**：接收端把 (x,y) 交给 RulesEngine，权威棋盘（含围棋提子）上屏；wasm 拒绝即丢消息。合法性守卫（toMove/边界/占据）是防乱序的第一层，Rust 规则是第二层。
- `SyncState.sv`：回退纪元。本地悔棋/重开 +1 并随快照广播；接收端 `(sv, history.length)` 双键比较，sv 更旧或同 sv 更短才丢弃（B1：只比 length 会让观战者看不到回退）。快照到达经 `RulesEngine.adopt` 重建引擎与历史。
- 发送唯一口 `transport.send()`：BC + 所有 open RTC + 服务器 relay 同 seq 三发；接收端只对**有副作用的类型**按 (sender,seq) 单调去重（gameChannel.ts 的 DEDUP 集合：Move/UndoReq/UndoAck/ResetReq/ResetAck/SwapReq/SwapAck/Avatar/Chat），Hello/SyncState/SyncRequest/Ping/Pong 不去重（幂等或重连 seq 归零）。
- 协商语义：请求 → 对方弹窗 → Ack；同意后双方各自执行本地操作（走 Rust 引擎）并补发 SyncState(sv+1) 对齐观战者；拒绝仅提示。`Ctl` 消息已删除（观战房间控制走服务器信令）。

## 4. 建连流程

### 4.1 同源（presence 可达）
```
inviter: createInvite → waiting（offer 编进邀请 URL）
invitee: 开/粘链接 → acceptInvite → acceptOffer → answer
       → presence.challenge(inviterId,pwd,k,s,inviteeGameId,ans)
inviter: drain 队列见 pwd 匹配 → acceptChallenge → applyAnswer → join(inviteeGameId) → playing → presence.accept
invitee: accept 信 / RTC open → enterPlayingAsInvitee（幂等）
```

### 4.2 跨设备无服务器（回执模型）
```
invitee: 打开/粘贴带 rtc 的链接 → 自动 acceptOffer/answer → 回执弹窗（1.2s 后仍未 open 才弹）
       → 复制回执链接发回（任意渠道）
inviter: 等待页「回执」粘贴 → acceptReceipt：校验(inviterId==me && pwd==本局) → applyAnswer
       → join(invitee.gameId) → playing（pwd 置 null）
```

### 4.3 服务器模式（userId 链接，当前主路径）
```
invitee: 打开 /<inviterId>?pwd= → 连接 ready 后 signal "join"{pwd,name}
inviter: 本地校验 pwd（错 → wrong-pwd 弹窗）→ "accept"{} → createOfferPlain → "offer"{name,kind,size,gameId,offer}
invitee: serverAcceptOffer → 直连 → "answer"{answer} → 双方 playing（Hello+SyncRequest）
挑战:    "challenge"{name,kind,size} → 邀请弹窗 → "challenge-accepted"{} → 发起方 serverAdmitChallenger
        （建局：新 gameId/pwd/specPwd，执黑 inviter）→ "offer" → 同上
观战:    "spec-join"{pwd,name} → pwd 对：admit（spectator 直连 + "spec-offer"{gameId,offer}）→ "answer"
        pwd 错/无 → "spec-pending" + 聊天区私有申请 → 批准 admit / "spec-reply"{ok:false,reason}
房间:    "spec-sync"{list,enabled}（名单/开关全量广播，双方对局者各自维护）、"spec-kick"/"spec-kicked"、
        "spec-chat"（观战发言经 host 转发给另一位对局者）、"spec-chat-req"{relay?,applicant?}（双 host 批准）、
        "spec-chat-ack"{ok}、对局者聊天 "chat"{userId,name,text}、头像 "spec-avatar"{dataUrl}
拒绝:    "reject"{reason}（对局/观战通用）
```

### 4.4 连接建立后的同步
RTC open：setPeerConnected(true)；受邀者 waiting 则进对局；150ms 后互发 Avatar；200ms 后双方各发一次
SyncState(sv)。A3 守卫见 §3。closed/error 的 peer 出列 rtcPeersRef；**红灯只跟 role="player" 的连接**
（观战者离开不亮「已中断」，B3）。

## 5. 身份与钥匙

- `userId`（`u-` 前缀）：sessionStorage 每标签页一个；服务器名册用它（与链接一致），同 ID 重连顶替旧连接。服务器分配的 `s-` 短 ID 仅 welcome 后存在，relay 目标用它。
- `myId`（GameChannel.sender）：crypto.randomUUID，每次页面加载随机——去重按 sender 隔离、重载后 seq 归零不撞车的前提。
- `pwd`：genPwd() 6 位 base36（CSPRNG），createInvite 生成；两人进局即作废；同时是 G1 编码密钥。
- `specPwd`：观战钥匙，同 genPwd；每局生成、整局有效，disableSpectate 时清除。
- `gameId`：无服务器模式受邀者生成；服务器模式房主/挑战发起方生成（offer 携带）。channel 名 `goptop-game-<gameId>`。

## 6. 服务器 WS 协议（C2S/S2C，字段 camelCase）

```
C2S: {t:"hello",name,userId?} | {t:"announce",status,gameId?} | {t:"signal",to,kind,payload}
   | {t:"relay",to,payload} | {t:"ping"}
S2C: {t:"welcome",id} | {t:"peers",users:[{id,name,status,gameId}]} | {t:"signal",from,kind,payload}
   | {t:"relayed",from,payload} | {t:"pong"} | {t:"error",code?,msg}
```
- `error.code:"taken-over"`：同一 userId 在别处重新连接，旧连接被顶替踢出；客户端收到后应主动断开旧 socket 并走重连（serverChannel.ts 已处理，见 main.rs 顶替逻辑）。
限制：hello 10s 超时；每连接 10s 窗口 500 条；signal payload ≤128 键、relay ≤64 键；60s 空闲断开。
客户端 25s 心跳（低于 CF 代理 100s 空闲阈值），指数退避重连（封顶 10s），重连后重放 announce。

## 7. 规则执行（Rust 真源）

落子/悔棋/重开判定全在 crates/goptop-core（wasm）：五连、围棋提子、禁自杀。TS 无规则副本
（checkFive 三重实现的教训）。劫争/终局数目未实现（core go.rs 标注 Phase 4）。构建见
scripts/build-wasm.sh，绑定语义见 crates/goptop-core/src/wasm.rs 头注释。

## 8. 已知限制（与 docs/已知限制与路线图.md 对应）

- 传输层（WebRTC/信令/编解码）仍在 TS——架构合规格后续把传输迁入 Rust。
- 无服务器模式跨设备观战未实现（/watch 仅同源）；服务器模式观战已全链可用。
- P2P 数据面断线无重连（服务器信令面有）；围棋无劫/数目；confirmReq 弹窗无队列（未决时被新请求覆盖）。
