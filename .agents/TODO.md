# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。
> 审查报告：review/2026-09-07-main-agent-code-review.md（修复记录已回填）。

## 回执与观战（2026-09-07 用户拍板，见 memory/2026-09-07-receipt-ui-and-spectator-design.md）

- [x] **回执按钮常驻**：header 常驻「输入回执」（任何页面可见），等待页/大厅入口保留。
- [x] **回执确认按钮文案去「开局」**：已改「确认回执」。
- [x] **回执类型自动识别（协议就位）**：回执链接带 `spec=1` 属性，`parsePastedAnswer` 解析 `spectator`，`hostAcceptReceipt` 自动分派；观战连接建立依赖下一项。
- [ ] **跨设备观战 P2P 补全**（下一轮，功能级）：房主生成含 spectator offer 的观战链接（每名观战者一条独立链接，观战钥匙 `sk` 独立于 pwd）；观战者打开后走回执模型；房主受理只建连接、不进对局、pwd 不失效。拓扑：观战者只与房主直连，房主向所有连接广播。

## 审查遗留（2026-09-07 修复轮未覆盖，见审查报告分级）

- [ ] **A3 SyncState 覆盖守卫**：直连 open 后 200ms 全量快照可吞掉本方先落之子。需设计同步版本号（如 history 长度作版本），单独一轮。
- [ ] **D1 App.tsx 拆分**：~1600 行单文件按「页面组件 / 对局状态机 / 信令编排」三刀拆分。
- [ ] **D6 presence beforeunload 监听器累积**：start→stop→start 循环重复注册。
- [ ] **D8 notice setTimeout 竞态**：连续 setNotice 时旧 timer 清掉新消息；收敛为 showNotice(text, ms) 单入口。
- [ ] **D11 前端测试**：vitest 覆盖 GameChannel 去重、parsePastedLink/parsePastedAnswer、keyStream/编码往返、genPwd 长度。
- [ ] **C4 Rust 错误分类改枚举**：上层用字符串 contains 匹配 RuleError 文本，脆弱。

## 已完成

### 2026-09-07 审查修复轮（commit 见 review 修复记录）
- [x] A1 acceptAnswer 幂等位改为成功后置位（坏回执可重试）；A2 Move 协议加 by 颜色字段（Resign/Pass/Place 不再靠本地推断）；A4 回执先受理后推进+弹窗内重试；A5 邀请链接 offer 就绪后才可复制；A6 客人「查看回执」重开入口；A7 换局入口 closeAllRtcPeers 清理；A8 钥匙不符/满员挑战回 reject
- [x] D2/C7/B4 死代码清理（game/ 目录、Stone.tsx 删除；gameStore.ts 瘦身并改实况注释）；D3 genPwd 改 CSPRNG 固定 6 位；D5 disconnected 不判死；D7 关闭/失败 peer 出列；D9 观战者 presence 显示空闲；D10 core 尺寸不变量校验
- [x] C1/C2/C3/C5/C6/C8 + B1/B2/B3 注释与实现对齐

### 2026-09-07 早前（弹窗回执轮）
- [x] 回执改为弹窗粘贴（原窗口），移除「打开回执链接」路径（曾致同机两窗互弈）
- [x] 粘贴解析与域名无关（`parsePastedLink` / `parsePastedAnswer`）；单段路径收紧（u- 前缀/邀请参数）
- [x] 分享域名全自动（Web=当前站点，非 Web=`SHARE_ORIGIN_NATIVE` 常量），移除用户设置入口
- [x] offer/answer 编码：deflate 压缩 + pwd XOR 混淆 + URL 安全 base64（`G1` 编码，`encodeRtcPayload`）
- [x] Move 消息按 (sender, seq) 去重：BroadcastChannel 与 WebRTC 双链路同消息只应用一次
- [x] 客人直连 open 后不再依赖主机 accept 信件，直接进对局（跨设备无 Presence）
