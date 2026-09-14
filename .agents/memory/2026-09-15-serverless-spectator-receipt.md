# 2026-09-15 · 无服务器跨设备观战设计拍板（回执模型扩展）

## 决策（Rust 化轮内实现，已知限制 A1 计划的落地形态）

- spec 链接扩展：`/<userId>?pwd=<specPwd>&spec=1[&specrtc=<G1 观战 offer>]`。
  无服务器模式下对局者 createInvite 时**预生成一个观战 offer**（DirectRtcPeer
  isInviter role=spectator）编进链接。
- 观众流程（跨设备）：打开 spec 链接 → 用 specrtc 解出 offer → 生成 answer →
  页面展示**观战回执链接**（answerToUrl 同款 + spec=1 + rtcAns）→ 观众发给对局者 →
  对局者在对局页观战区「输入观战回执」粘贴 → applyAnswer → 直连建立 → SyncState 推流。
- **offer 一次性消费**：一个 offer 只能被一个观众用。对局者受理观战回执后**自动重新
  生成新观战 offer 并更新 specUrl**（P2P 页展示的链接自动换新，无需用户操作）；
  拿着旧链接（已被消费）的观众直连失败，提示向房主要新链接。
- 同源观众仍走 /watch/<gameId> BroadcastChannel（不变，零成本）。
- 服务器模式不需要这些（有 spec 链接信令），specrtc 仅无服务器生成。
- 对局回执与观战回执共用「输入回执」弹窗（parsePastedAnswer 的 spectator 字段已
  自动识别，旧分支的「尚未实现」报错替换为真实受理流程）。

**Why**：无服务器模式没有任何在线设施，跨设备首连只能靠静态链接交换信令；
offer 单向可入链接，answer 只能走回执——这是回执模型的对称扩展，与 A1 路线一致。

**How to apply**：session 状态机里 specPendingAnswer/specRtc 字段 + SpecReceipt 流程；
E2E 需覆盖「观众带 specrtc 链接 → 生成观战回执 → 对局者粘贴 → 直连看棋 → 链接自动换新」。

## 关联

- [[2026-09-15-rustification-direction-wasm-websys]]
- [[2026-09-07-receipt-ui-and-spectator-design]]
