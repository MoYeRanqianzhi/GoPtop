# .agents/MEMORY.md — 共享关键记忆索引

- **[归档记忆]** 2026-09-07 全量代码审查 → 两轮修复 → 修复轮审查 → R 级修复 → 复审，全循环闭环（A1-A8/B/C/D + R1-R6 落地；残留 R2a/R2b 低危守卫补漏跟踪于 TODO.md）。审查报告已移至 `review/archive/`（2026-09-07-main-agent-code-review.md、2026-09-07-fix-round-review.md），文件头有归档标注，仅作历史记录勿回填。
- [2026-09-07 P2P 对等术语拍板](memory/2026-09-07-p2p-peer-terminology-inviter-invitee.md) — 房主/客人全面废弃，改邀请者/受邀者（inviter/invitee）；ICE host 候选是协议词汇除外
- [2026-09-07 P2P 方向拍板](memory/2026-09-07-p2p-direction-a-no-server-auto-share-origin.md) — A 路线纯无服务器（回执保留）；分享域名必须全自动（代码常量），禁止做成用户设置项
- [2026-09-07 回执与观战设计](memory/2026-09-07-receipt-ui-and-spectator-design.md) — 同浏览器双窗口是极端情况不考虑；回执按钮常驻、确认文案去「开局」、回执类型按链接属性自动识别；观战者只与邀请者直连；此前开发无记忆是违规，今后拍板必落记忆
- [2026-09-07 禁多代理并发](memory/2026-09-07-no-parallel-agents-single-serial.md) — 并发 Workflow 打爆 API 中断；审查/修复一律单代理串行，禁 parallel() 扇出
- [2026-09-10 P2P 跨网实测](memory/2026-09-10-p2p-crossnet-stun-turn.md) — 代码无 bug；失败根因=移动丢国际来向 UDP+阿里云安全组；STUN-only 天花板已实测，TURN 兜底方向待拍板；三台测试机部署位置与网络特性
- [2026-09-12 userId 链接+协商+聊天+观战房间](memory/2026-09-12-userid-link-negotiation-chat-spectator.md) — 链接回归 /userId?pwd=；服务器纯转发不落地；协商消息必须三链路去重；名册 ID=客户端持久 userId
