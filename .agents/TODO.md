# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。
> 审查报告：review/2026-09-07-main-agent-code-review.md（修复记录已回填）。

## 回执与观战（2026-09-07 用户拍板，见 memory/2026-09-07-receipt-ui-and-spectator-design.md）

- [x] **回执按钮常驻**：header 常驻「输入回执」（任何页面可见），等待页/大厅入口保留。
- [x] **回执确认按钮文案去「开局」**：已改「确认回执」。
- [x] **回执类型自动识别（协议就位）**：回执链接带 `spec=1` 属性，`parsePastedAnswer` 解析 `spectator`，`acceptReceipt` 自动分派；观战连接建立依赖下一项。
- [ ] **跨设备观战 P2P 补全**（下一轮，功能级）：邀请者生成含 spectator offer 的观战链接（每名观战者一条独立链接，观战钥匙 `sk` 独立于 pwd）；观战者打开后走回执模型；邀请者受理只建连接、不进对局、pwd 不失效。拓扑：观战者只与邀请者直连，邀请者向所有连接广播。

## 审查遗留（2026-09-07 修复轮 + 同日第二轮全部落地）

- [x] **A3 SyncState 覆盖守卫**（commit be7fac7）：接收端按 history.length 作快照版本，旧快照丢弃；浏览器验证守卫生效且正常同步不受影响。
- [x] **D1 App.tsx 拆分**（commit 60fc02e + 99fbb4c）：三刀完成——game/board.ts（规则唯一实现）、pages/components.tsx + pages/LocalPage.tsx（展示组件）、state/useGameSession.tsx（对局状态机+信令编排）。App.tsx 1698→544 行。
- [x] **D6 presence beforeunload 监听器累积**（commit 0bf204c）：处理器改实例字段，start 注册一次/stop 注销。
- [x] **D8 notice setTimeout 竞态**（commit 0bf204c）：showNotice(text, ms?) 单入口，接管全部调用点。
- [x] **D11 前端测试**（commit be7fac7）：vitest 30 例——GameChannel 去重、G1 编码往返、genPwd、链接解析全分支。`npm test`。
- [x] **C4 Rust 错误枚举化**（commit 0d24ea0）：go::try_place 返回 RuleError，删除字符串匹配分类。

## R 级（2026-09-07 修复轮审查新发现，同日全部修复）

- [x] **R1** gameStore.ts 死代码复活+三处注释矛盾：已 git rm，architecture.md 同步。
- [x] **R2** acceptReceipt await 间隙竞态：续体前复查 phase/role；观察项 1（createInvite 异步续体）同模式加 pwdRef 复查。跨设备回执路径浏览器验证。
- [x] **R3** 两处 setNotice 直调改 showNotice(null)；showNotice 内部回调保持直调（非调用点）。
- [x] **R4** components.tsx/App.tsx 头注释对齐两刀完成后的实况。
- [x] **R5** transport.ts/p2p-protocol.md/测试夹具的 4 处 host 术语残留清理。
- [x] **R6** genPwd 注释改实测数字（偏差桶 2118184960/2^32，单值差 ~2.3e-10；原注释 2.7% 与审查口径 1.4% 均不准）。

## 术语轮（2026-09-07 用户拍板：P2P 对等无主客）

- [x] **房主/客人 → 邀请者/受邀者**（commit 0bf204c）：host/guest 全套标识符与文案改 inviter/invitee（hostCreate→createInvite、guestChallenge→acceptInvite、finishHostRtc→applyAnswer、hostAcceptReceipt→acceptReceipt、waiting-guest→waiting-invitee 等）。ICE 术语 host 候选保留（WebRTC 协议词汇）。

## 下一步候选（未获指令，不动工）

- **R2 残留（R2a/R2b，修复轮审查新发现，低）**：acceptReceipt 守卫补 `pwdRef.current !== r.pwd` 对比（防取消后同阶段开新局被骗过）；createInvite catch 分支补同款三重守卫（成功路径已有）。合计 ~6 行，见 review/2026-09-07-fix-round-review.md「修复记录审查」。

- **D8 后续（可选）**：App.tsx 剩余 544 行中的页面拼装（menu/p2p/users/settings/user/watch JSX）可再拆 pages/*.tsx——纯机械移动，收益是文件更小，无行为面。
- **D11 扩展（可选）**：useGameSession 状态机尚未有测试（需 renderHook 或拆纯函数）；DirectRtcPeer 集成路径未测（需真实 ICE）。
- Cloudflare Pages 真实上线（用户预告过，未明确下令）。

## 已完成（历史）

### 2026-09-07 审查修复轮（commit 7df4dd5）
- [x] A1 acceptAnswer 幂等位改为成功后置位（坏回执可重试）；A2 Move 协议加 by 颜色字段（Resign/Pass/Place 不再靠本地推断）；A4 回执先受理后推进+弹窗内重试；A5 邀请链接 offer 就绪后才可复制；A6 受邀者「查看回执」重开入口；A7 换局入口 closeAllRtcPeers 清理；A8 钥匙不符/满员挑战回 reject
- [x] D2/C7/B4 死代码清理（旧 game/ 目录、Stone.tsx 删除）；D3 genPwd 改 CSPRNG 固定 6 位；D5 disconnected 不判死；D7 关闭/失败 peer 出列；D9 观战者 presence 显示空闲；D10 core 尺寸不变量校验
- [x] C1/C2/C3/C5/C6/C8 + B1/B2/B3 注释与实现对齐

### 2026-09-07 早前（弹窗回执轮）
- [x] 回执改为弹窗粘贴，移除「打开回执链接」路径（曾致同机两窗互弈）
- [x] 粘贴解析与域名无关（`parsePastedLink` / `parsePastedAnswer`）；单段路径收紧
- [x] 分享域名全自动（Web=当前站点，非 Web=`SHARE_ORIGIN_NATIVE` 常量）
- [x] offer/answer 编码：deflate 压缩 + pwd XOR 混淆 + URL 安全 base64（`G1` 编码）
- [x] Move 消息按 (sender, seq) 去重：BroadcastChannel 与 WebRTC 双链路同消息只应用一次
- [x] 受邀者直连 open 后不再依赖邀请者 accept 信件，直接进对局（跨设备无 Presence）
