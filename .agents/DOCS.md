# .agents/DOCS.md — 开发文档索引（给 agent 看）

> 人类文档在 `docs/` 与 `README.md`，不在此列。改代码必须同步更新对应文档；文档不许超前于实现。

| 文档 | 内容 |
|---|---|
| [architecture.md](docs/architecture.md) | 前端模块地图、路由、状态/ref 双轨、对局状态机、消息双通道去重、自适应布局红线、已知死代码 |
| [p2p-protocol.md](docs/p2p-protocol.md) | URL 格式、G1 信令编码（deflate+XOR+base64url）、GameMsg 线格式、同源/跨设备建连时序、身份与钥匙、STUN |
| [../MEMORY.md](../MEMORY.md) | 关键拍板索引（指向 memory/ 下长文） |
| [../TODO.md](../TODO.md) | 共享待办（每项标注来源记忆） |
| [../review/2026-09-07-main-agent-code-review.md](../review/2026-09-07-main-agent-code-review.md) | **全量审查报告（2026-09-07）**：A 级 8 个真实 bug（含行号与修复方向）、架构断层 4 项、注释失实 8 项。修复轮必读，修完回填修复记录 |

## 记忆文件（memory/）

- `2026-09-07-p2p-direction-a-no-server-auto-share-origin.md` — A 路线纯无服务器；分享域名全自动禁用户设置
- `2026-09-07-receipt-ui-and-spectator-design.md` — 回执 UI 三要求；观战只与房主直连；同浏览器双窗口为极端情况

## 规范要点（新 agent 必读）

1. Rust edition 2024；版本 x.x.x-alpha/beta/rc.x。
2. 注释密度是硬性约定：每个文件/函数要有「为什么」，agent 也要能读懂。
3. 改动闭环：改 → `cargo check`/`npx tsc --noEmit` + build/浏览器验证 → git add+commit，不 commit 红。
4. UI/前端改动必须在浏览器里验证过才算完成（Playwright 或人工）。
5. 每次用户拍板/根因修复 → 落 `memory/` + 更新 `MEMORY.md`/`TODO.md`。
