---
name: p2p-peer-terminology-inviter-invitee
description: 用户拍板 P2P 对等无主客——房主/客人术语全面废弃，改邀请者/受邀者（inviter/invitee）
metadata:
  type: feedback
---

用户拍板（2026-09-07）：P2P 是对等网络，两个人之间没有主客之分——原来的差别只是「谁先发出了邀请链接」这一时间先后，不是地位差。「房主/客人（host/guest）」是中心化房间服务器的概念，从术语层面就误导后续开发。

**Why：** 术语塑造思维。沿用 host/guest 会让人误以为存在权威方/服务器语义，偏离无服务器 P2P 的架构本质。

**How to apply：**
- 全部代码标识符、UI 文案、文档一律用「邀请者（inviter）/ 受邀者（invitee）」。映射：hostCreate→createInvite、guestChallenge→acceptInvite、finishHostRtc→applyAnswer、hostAcceptReceipt→acceptReceipt、enterPlayingAsGuest→enterPlayingAsInvitee、waiting-guest→waiting-invitee、hostRtcRef→inviterRtcRef、hostId→inviterId。
- 例外：ICE 术语「host 候选（host/srflx）」「stun:host:port」是 WebRTC 协议词汇，保留不改。
- 新代码禁止再引入 host/guest/房主/客人 表述。

相关：[[p2p-direction-a-no-server-auto-share-origin]]、[[receipt-ui-and-spectator-design]]
