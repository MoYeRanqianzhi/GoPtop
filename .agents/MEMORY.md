# .agents/MEMORY.md — 共享关键记忆索引

- [2026-09-07 修复轮审查完成](../review/2026-09-07-fix-round-review.md) — 两轮修复核实全部通过（A1-A8/B/C/D 逐项对照代码）；新发现 R1（gameStore.ts 死代码+三方注释矛盾）/R2（acceptReceipt await 后未复查 phase 的竞态）待修，R3-R5 注释对齐可攒批
- [2026-09-07 P2P 对等术语拍板](memory/2026-09-07-p2p-peer-terminology-inviter-invitee.md) — 房主/客人全面废弃，改邀请者/受邀者（inviter/invitee）；ICE host 候选是协议词汇除外
- [2026-09-07 全量代码审查完成](../review/2026-09-07-main-agent-code-review.md) — 主代理串行审查：8 个真实 bug（A 级）+ 两轮修复记录（全部 A/C/D 级已落地，A3/D1/D6/D8/D11/C4 见第二轮）。**修复前必读报告**
- [2026-09-07 P2P 方向拍板](memory/2026-09-07-p2p-direction-a-no-server-auto-share-origin.md) — A 路线纯无服务器（回执保留）；分享域名必须全自动（代码常量），禁止做成用户设置项
- [2026-09-07 回执与观战设计](memory/2026-09-07-receipt-ui-and-spectator-design.md) — 同浏览器双窗口是极端情况不考虑；回执按钮常驻、确认文案去「开局」、回执类型按链接属性自动识别；观战者只与邀请者直连；此前开发无记忆是违规，今后拍板必落记忆
- [2026-09-07 禁多代理并发](memory/2026-09-07-no-parallel-agents-single-serial.md) — 并发 Workflow 打爆 API 中断；审查/修复一律单代理串行，禁 parallel() 扇出
