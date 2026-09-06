# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。

## 回执与观战（2026-09-07 用户拍板，见 memory/2026-09-07-receipt-ui-and-spectator-design.md）

- [ ] **回执按钮常驻**：「输入回执」按钮始终在页面上可见，不只在房主等待页出现。
- [ ] **回执确认按钮文案去「开局」**：现在写的是「受理回执并开局」，必须改掉——回执不一定是加入对局。
- [ ] **回执类型自动识别**：回执链接自带属性（如 `role=spectator` / 对局标识），粘贴后前端解析属性自动判断是「加入对局回执」还是「观战回执」，自动建立对应 P2P 连接，用户无需选择。
- [ ] **跨设备观战 P2P 补全**：现况 `transport.ts:12` 注释宣称「跨设备观战走独立 WebRTC 直连」但**从未实现**（观战者跨设备打开 `/watch/<id>` 永远空棋盘）。实现：房主邀请观战时生成含 spectator offer 的观战链接（每名观战者一条独立链接）；观战者打开后弹观战回执；房主粘贴回执只建连接、不进对局、pwd 不失效。拓扑：观战者只与房主直连，房主向所有连接（选手+观战者）广播。
- [ ] 修完上述后把 `transport.ts` 头部注释与实现对齐（不许注释超前于实现）。

## 已完成（本次会话，待提交）

- [x] 回执改为弹窗粘贴（原窗口），移除「打开回执链接」路径（曾致同机两窗互弈）
- [x] 粘贴解析与域名无关（`parsePastedLink` / `parsePastedAnswer`）
- [x] 分享域名全自动（Web=当前站点，非 Web=`SHARE_ORIGIN_NATIVE` 常量），移除用户设置入口
- [x] offer/answer 编码优化：deflate 压缩 + pwd XOR 混淆 + URL 安全 base64（`encodeRtcPayload`）
- [x] Move 消息按 (sender, seq) 去重：BroadcastChannel 与 WebRTC 双链路同消息只应用一次
- [x] 客人直连 open 后不再依赖主机 accept 信件，直接进对局（跨设备无 Presence）
