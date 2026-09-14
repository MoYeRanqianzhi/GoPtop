# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。
> 审查报告：review/（开放）；review/archive/（2026-09-07 闭环归档）。

## 架构合规路线（2026-09-13 用户重申总要求：Rust 承接一切功能，TS 只做 UI）

- [x] **第一阶段：规则下沉 wasm**（commit 95f25f2）：core 经 WasmGame 绑定在 Web/Tauri 执行
  全部落子/悔棋判定；围棋提子/禁自杀在 Web 生效；TS checkFive 副本删除。
- [x] **第二阶段：传输层 Rust 化**（2026-09-15 完成，commit 见 memory/2026-09-15-rustification-phase2-landed）：
  goptop-net（协议/链接/编解码/去重/会话状态机）+ goptop-transport（web-sys 三通道）
  + 前端 wasm 壳接线，E2E 42/42。TS 只剩 UI 绑定与少量展示辅助。
  - 收尾项：本地对局改走 WasmSession 后删除 rules.ts（见已知限制 C3）。

## 2026-09-15 大工程轮（用户指令「完整进行 Rust 化，以上全部需要完成」）

- [x] 围棋劫争 + 双 Pass 终局 + 区域计分（含死子标记同步与双确认）；9/13 路盘外假气修复。
- [x] 协商弹窗队列（confirm_queue，未决不再覆盖）。
- [x] 传输层 Rust 化（goptop-net / goptop-transport / 前端接线）。
- [x] 无服务器跨设备观战（specrtc 回执模型；状态机单测覆盖；跨设备 E2E 待补真实双机验证）。
- [x] 审查低危残留（服务器 unreachable/字节上限/注释失真；join 互斥/挑战 tiebreak/
  spec 转发目标 opponent 化随状态机迁移完成；UI P3 见 review 台账——大部分随 D1 拆分与
  本轮接线消化，剩余纯 UI 细节（favicon/safe-area 等）未动）。

## 下一步候选（未获指令，不动工）

- **官服部署更新**：本轮服务器协议零改动（纯转发兼容），现网版本仍可用；下次发版时统一部署。
- **跨设备无服务器观战真实双机 E2E**（单测已覆盖状态机全流程）。
- **协商队列 E2E**（单测已覆盖队列消费语义）。
- **对局数据面断线重连**（已知限制 A1）。
- **本地对局统一走 WasmSession**（删 rules.ts）。
- **UI 细节残留**：favicon、safe-area（review #4 P3-13/14 尾项）。

## 已完成（历史）

### 2026-09-13/14（整理轮 + 四端实机 + 六维审查）
- [x] 规则下沉 wasm + pickKind 原子化；死代码清理；B1-B4 修复；文档收口。
- [x] 真 Windows 标题栏 + 四端实机测试全通过 + 六维审查 #1-#6 + E2E 基线入库。
- [x] 2026-09-12/13：userId 链接回归 + 协商 + 聊天 + 观战房间 + 大厅挑战修复。
- [x] 2026-09-07：审查修复轮 A1-A8/C/D/R 全闭环；术语轮（邀请者/受邀者）。
