# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。
> 审查报告：review/（开放）；review/archive/（2026-09-07 闭环归档）。

## 架构合规路线（2026-09-13 用户重申总要求：Rust 承接一切功能，TS 只做 UI）

- [x] **第一阶段：规则下沉 wasm**（commit 95f25f2）：core 经 WasmGame 绑定在 Web/Tauri 执行
  全部落子/悔棋判定；围棋提子/禁自杀在 Web 生效；TS checkFive 副本删除。
- [ ] **第二阶段：传输层 Rust 化**（大工程，未动工）：WebRTC/信令/编解码/对局状态机仍在 TS。
  方向待拍板：wasm WebRTC（如 wasm-webrtc）或 Tauri 原生桥接 + Web 降级。详见 docs/已知限制与路线图.md C0。

## 审查遗留（2026-09-07 两轮全部落地；2026-09-13 复核确认）

- [x] A3 SyncState 覆盖守卫：已升级为 (sv, history.length) 双键（2026-09-13，修复观战回退失同步 B1）。
- [x] R2a/R2b 复核：acceptReceipt 的 pwd 对比与 createInvite 续体守卫在现行代码中均已存在，关闭。
- [x] D1 App 拆分：2026-09-13 完成第三刀（App 765→243，pages/* + components/* 共用组件）与第四刀
  （useGameSession 1786→1208，serverSignaling/negotiation/chat 域模块 + SessionCtx）。
- [x] D11 前端测试：vitest 33 例 + E2E 42 断言（run.js，含观战回退可见性 9h-9j）。

## 2026-09-13 整理轮（记录，全部完成）

- [x] 死代码清理：joinCodeUrl/spectateCodeUrl/Ctl 消息/Presence.myName/trickle ICE 死链路/
  BrutalCard/goptop-transport crate/src-tauri 未用依赖。
- [x] Bug 修复：B1 观战回退失同步、B2 对手识别（opponentRef）、B3 connLost 生命周期、
  B4 观战权限跨局残留；恢复对局内提示行；用户主页挑战按 serverMode 分派。
- [x] 规则下沉 wasm + pickKind 原子化（Go{size:15} 中间帧曾致 wasm trap 白屏）。
- [x] 文档收口：architecture.md/p2p-protocol.md 重写；README/已知限制与路线图/DOCS.md 对齐实况。

## 下一步候选（未获指令，不动工）

- **围棋劫争与终局数目**：在 crates/goptop-core 实现（go.rs Phase 4 标注），重跑 scripts/build-wasm.sh。
- **协商弹窗队列**：undo/reset/swap/spec-chat 共用 confirmReq，未决时被覆盖（docs/已知限制与路线图.md A4）。
- **无服务器模式跨设备观战**：/watch 仅同源（docs/已知限制与路线图.md A1）。

## 已完成（历史）

### 2026-09-12/13（userId 链接 + 协商 + 聊天 + 观战房间 + 大厅挑战弹窗）
- [x] 跨设备观战（服务器模式）：spec 链接 + 观战房间管理（批准/禁言/踢人/关闭）+ 观战发言双 host 批准。
- [x] 悔棋/重开/换棋协商（UndoReq/ResetReq/SwapReq 对方同意制）+ 聊天面板 + 头像交换。
- [x] 大厅挑战修复：challenge-accepted 守卫错位 + /users 走错信令；邀请统一居中弹窗。
- [x] 用户主页挑战入口服务器模式分派 serverChallengePeer（2026-09-13）。

### 2026-09-07 审查修复轮（闭环，详见 review/archive/）
- [x] A1-A8、C1-C8、D1-D11、R1-R6 全部落地；术语轮（房主/客人→邀请者/受邀者）完成。
