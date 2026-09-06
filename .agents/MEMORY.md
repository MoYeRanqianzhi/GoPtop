# .agents/MEMORY.md — 共享关键记忆索引

- [2026-09-07 全量代码审查完成](../review/2026-09-07-main-agent-code-review.md) — 主代理串行审查：8 个真实 bug（A 级，含 acceptAnswer 幂等位顺序、Move 协议缺颜色字段、SyncState 覆盖竞态、回执旅程不可恢复）、前后端两套协议断层、8 处注释失实。**修复前必读报告第 7 节顺序**
- [2026-09-07 P2P 方向拍板](memory/2026-09-07-p2p-direction-a-no-server-auto-share-origin.md) — A 路线纯无服务器（回执保留）；分享域名必须全自动（代码常量），禁止做成用户设置项
- [2026-09-07 回执与观战设计](memory/2026-09-07-receipt-ui-and-spectator-design.md) — 同浏览器双窗口是极端情况不考虑；回执按钮常驻、确认文案去「开局」、回执类型按链接属性自动识别；观战者只与房主直连；此前开发无记忆是违规，今后拍板必落记忆
- [2026-09-07 禁多代理并发](memory/2026-09-07-no-parallel-agents-single-serial.md) — 并发 Workflow 打爆 API 中断；审查/修复一律单代理串行，禁 parallel() 扇出
