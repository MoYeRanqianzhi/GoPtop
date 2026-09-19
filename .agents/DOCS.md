# .agents/DOCS.md — 开发文档索引（给 agent 看）

> 人类文档在 `docs/` 与 `README.md`，不在此列。改代码必须同步更新对应文档；文档不许超前于实现。
> 注意：本文件内链接相对 `.agents/` 解析。

| 文档 | 内容 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 前端模块地图（net/state/pages 三层）、wasm 规则执行、状态/ref 双轨、对局状态机、消息三链路去重与 sv 纪元、自适应布局红线、测试（vitest + E2E 42 断言 + 浏览器脚本）、历史教训索引 |
| [docs/real-device-testing.md](docs/real-device-testing.md) | 全维度实机测试手册：五端驱动方式（桌面壳 custom-protocol/WebView2 调试端口、安卓 `_android` 驱动禁 connectOverCDP、鸿蒙 hdc fport 与 hvigor 构建）、测试脚本用法与规格串、驱动层 9 个坑、布局红线（棋盘正反馈塌陷/窄屏横幅遮挡）、回归口径 |
| [docs/p2p-protocol.md](docs/p2p-protocol.md) | 三链路总览、URL 格式（邀请/回执/观战 spec 链接）、G1 信令编码、GameMsg 线格式（含 sv）、同源/回执/服务器三种建连时序、观战房间信令、身份与钥匙、服务器 WS 协议、Rust 规则真源 |
| [../review/2026-09-13-cleanup-audit.md](../review/2026-09-13-cleanup-audit.md) | 2026-09-13 整理轮开工审计（frontend/crates/文档全量精读；历史审查归档在 review/archive/） |
| [../review/2026-09-13-review-round.md](../review/2026-09-13-review-round.md) | 六维代码审查唯一台账（各维度发现与修复状态全在此跟踪） |
| [../MEMORY.md](../MEMORY.md) | 关键拍板索引（指向 memory/ 下长文） |
| [../TODO.md](../TODO.md) | 共享待办（每项标注来源记忆） |

## 记忆文件（memory/）

- `2026-09-07-p2p-direction-a-no-server-auto-share-origin.md` — A 路线纯无服务器；分享域名全自动禁用户设置
- `2026-09-07-receipt-ui-and-spectator-design.md` — 回执 UI 三要求；观战只与邀请者直连；同浏览器双窗口为极端情况
- `2026-09-07-p2p-peer-terminology-inviter-invitee.md` — 房主/客人 → 邀请者/受邀者
- `2026-09-07-no-parallel-agents-single-serial.md` — 多代理并发规则（2026-09-13 用户解除禁令）
- `2026-09-10-p2p-crossnet-stun-turn.md` — 跨网实测：失败根因在运营商/云防火墙网络层；三台测试机网络特性
- `2026-09-12-userid-link-negotiation-chat-spectator.md` — userId 链接回归；服务器纯转发；协商三链路去重
- `2026-09-13-lobby-challenge-modal.md` — 大厅挑战两根因修复 + 邀请统一弹窗 + 跨设备实测
- `2026-09-13-cleanup-refactor-round.md` — 整理轮：三层拆分、规则下沉 wasm、B1-B4 修复的根因与拍板
- `2026-09-18-multi-device-real-test-round.md` — 全维度实机测试轮：五端两两对战/聊天/观战/设计功能全通；7 个产品缺陷与 2 个布局陷阱；测试内容与结果明细
- `2026-09-19-chat-dock-three-tier-layout.md` — 聊天停靠栏三档（等宽/压缩/弹窗）实测规则；棋盘不许被压窄的 flex 红线；run.js 基线 42→49
- `2026-09-19-platform-storage.md` — 平台存储落盘位置（桌面 ~/.goptop / 安卓应用私有根 / 鸿蒙 filesDir / Web localStorage）；宿主钩子让 wasm 也走门面；四端实测与迁移
- `2026-09-19-comment-audit-and-doctrine.md` — 注释专项审查（workflow 14 切片 × 对抗核实，确认 104/驳回 11）与注释口径收紧：只写非显然 WHY；顺带暴露两处真缺陷

## 规范要点（新 agent 必读）

1. Rust edition 2024；版本 x.x.x-alpha/beta/rc.x。
2. 注释密度是硬性约定：每个文件/函数要有「为什么」，agent 也要能读懂。
3. 改动闭环：改 → `cargo check`/`npx tsc --noEmit` + build/浏览器验证 → git add+commit，不 commit 红。
4. UI/前端改动必须在浏览器里验证过才算完成（Playwright 或人工）。
5. 每次用户拍板/根因修复 → 落 `memory/` + 更新 `MEMORY.md`/`TODO.md`。
6. **规则判定禁止在 TS 新增副本**：改规则只动 crates/goptop-core，然后 `bash scripts/build-wasm.sh` 并提交 frontend/src/wasm/ 产物。
