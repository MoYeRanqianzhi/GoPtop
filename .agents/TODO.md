# .agents/TODO.md — 共享待办

> 记忆规范见 CLAUDE.md 与 .agents/MEMORY.md。每个待办动手前先读对应记忆文件。
> 审查报告：review/（开放）；review/archive/（2026-09-07 闭环归档）。

## 2026-10-08 Agent 对战大轮（用户拍板立项，规格 plan/2026-10-08-agent-battle.md）

- [x] 阶段①+②：goptop-agent crate（会话对/一切皆文件工具面/决策循环/compact/三协议
      LLM），无头全量测试绿（Mock LLM）——提交链与 crate 结构见
      memory/2026-10-08-agent-battle-round.md。
- [x] 阶段③：接壳 + 唯一入口 AgentPage（/agent）+ 壳级 e2e（agent-builtin/agent-entry）
      + 两轮审查修复（157fb86 等）。
- [x] 阶段④：MCP 出口（rmcp 3.2 streamable HTTP，feature mcp 仅桌面编译）+ MCP 连接卡
      + agent-mcp.js e2e（b1365f2）。
- [x] 回归：`cargo test -p goptop-agent --features mcp` 100 绿、vitest 94/94
      （本轮实测口径，见记忆文件「数字与基线」）。
- [x] 文档与记忆同步：使用指南补「Agent 对战」章；本轮落地记录入 memory/。
- [ ] 遗留一（阶段⑤）：Web 内置模式——VfsStore web 后端（IndexedDB 经 agentVfs.ts
      钩子）、HttpChannel web 通道、循环 wasm 驱动（禁 tokio time）；当前 Web/鸿蒙
      进 /agent 只有降级横幅。
- [ ] 遗留二（phase ⑤ 后）：产物同步 build-wasm/build-ohos 冒烟 + 既有 e2e 全量
      （本轮纯桌面接线，wasm 产物零变化）。
- [ ] 遗留三：MCP 服务器大厅注册（下一任务，registry scope 与 handler-state 已留缝）。
- [ ] 既有遗留不动：A 方案（stable 态用原 offer 重建 peer）、壳/模拟器实测矩阵待
      内存宽裕重跑（见 2026-10-07 轮）。

## 2026-10-07 探查 bug 大轮（用户指令「加强测试和优化，探查bug」）

- [x] 九维并行审查上一轮原生传输改动：44 发现 → **对抗验证确认 36 / 驳回 8**
      （详见 memory/2026-10-07-bug-hunt-native-round.md）。
- [x] 八组修复全落位（128502f/7395c13/db54a99/7ff8eb2/81d8fb2/062d169/07976f4/a5bd52c/ab51082）：
      状态机 8 处（同源挑战 JoinChannel、观战者 Pass、sync_epoch 复位、坏回执重试、
      观战槽误判连接、SyncState 先校验后落账、Reset size 归一化、inflate 1MB 上限）、
      AI 4 处（komi 对齐 7.5、AnalyzeRequest 边界校验、gomoku 坐标校验、UCT_C 清除）、
      transport-native 任务生存期 3 处 + server_mode 补测、前端会话生命周期
      （dispose/session_drop/StrictMode adopt-guard/失败错误态）、鸿蒙存储原子写 +
      NAPI 票号表回收 + async work 泄漏、Tauri poll 空轮询契约、E2E 判负链路 6 处。
- [x] 回归：cargo workspace 全绿、vitest 70/70、tsc 干净；Web 基线
      run.js 58/58、go-capture 7/7、ai.js 9/9、noserver-pair 9/9、stress 120 手 8/8。
- [x] 产物同步（35ffdd3）：wasm 三件套、鸿蒙 .so 双 ABI、rawfile。
- [x] 遗留一（B 方案落地，7348822）：set_remote 失败 surface 成 `Event::RtcApplyFailed`，
      邀请者等待态给可行动提示；非等待态维持静默。**A 方案**（stable 态用原 offer
      重建 peer，使补发回执真正连上）仍留观——按实际发生率再决定。
- [x] 遗留二（7348822）：AI 执黑思考中重开的死局——genTick 渲染代次进 effect 依赖，
      重开必然重跑；ai.js 新增死局回归检查（红验证确认无修复时手数=0）。
- [ ] 遗留三：**桌面壳 shell-pair 与安卓/鸿蒙模拟器实测矩阵均未随本轮修复重跑**——
      壳 release 构建在链接前被系统内存回收（纪律：不自行重启，待内存宽裕或用户指示
      再构建；当前 target/release/goptop.exe 仍是 9月20日 旧二进制，跑壳测属无效验证）。

## 2026-09-20 大轮（用户指令「全部做完 → 大规模代码审查和修复 → 换无头前端验证一切功能皆 Rust」）

- [x] 架构口径订正落地：**wasm 只是 Web 端的编译目标**。桌面/Android 经 Tauri
      command 直连 Rust；**鸿蒙经 NAPI 原生模块直连 Rust**（本轮补上最后一个）。
- [x] 无头验证：界面完全移除，仅靠 `HeadlessHost` 驱动全部功能（5/5，commit 6158454）。
- [x] 大规模代码审查 + 修复：无服务器直连链路三处根因（钥匙判定/回执带钥/槽位 tag）、
      原生传输 6 处（relay 兜底、订阅竞态、连接重建、编解码钥匙、ws 重连泄漏、
      LeaveChannel 语义）、前端原生后端 3 处（新局未落地取局面、实例生命周期、
      thinking 收回）、**邀请者受理回执后卡等待进不了对局**。
- [x] 换一个前端验证：`frontend/` 的 vitest 49、`cargo test` 全绿、无头 5/5。
- [x] 浏览器面基线：`run.js` 58/58、`go-capture.js` 7/7、`ai.js` 9/9、`stress.js` 8/8
      （120 手长跑，堆稳定在 10MB）、新增 `noserver-pair.js` 9/9（无服务器跨设备直连，
      此前**无任何自动化覆盖**，只能人肉双机）。
- [x] 全平台实机：
      - **Web**：`run.js` 58/58、`go-capture.js` 7/7、`ai.js` 9/9、`stress.js` 8/8（120 手）。
      - **桌面壳**（Tauri，原生 Rust）：`ai.js cdp:9222` 9/9、`stress.js cdp:9222 120` 8/8。
      - **鸿蒙**（NAPI 原生 Rust）：`ohos-native-probe.js` 11/11、`ai.js cdp:9444` 9/9、
        `stress.js cdp:9444 120` 8/8（120 手，堆 11→11MB）。
      - **安卓**（AVD `goptop_test`，x86_64）：`ai.js android` 9/9、
        `stress.js android 120` 8/8。**后端判定已确证走原生**（不是"能下棋"就算数）：
        `__TAURI_INTERNALS__` 存在且 `invoke` 可用；资源时间线里**没有** `goptop_core`
        与 `goptop_ai`（规则与 AI 的 wasm 都是懒加载，回落才会出现）、没有 AI Worker；
        `__store.backend()` 为 `tauri`。
        （首轮因模拟器与 gradle 构建同时跑把内存压到临界被回收；改成**先构建后起模拟器**
        串行执行即通过，峰值内存减半。）
- [x] **原生平台的传输层已不再跑 wasm**（本轮收口）。规则/AI 之后，P2P 会话也在
      三端直连 Rust：`crates/goptop-transport-native` 接进了 `src-tauri`（桌面/Android）
      与 `crates/goptop-ohos`（鸿蒙），前端由 `frontend/src/net/session.ts` 分派。
      - 命令面只有一个口子：给 `UiCommand` 加 serde，`session_cmd(id, cmdJson)`，
        线上形态即该枚举——**不在宿主里再抄一份分派表**（两处契约迟早分叉）。
      - 状态回推走拉模式（`session_poll` = 泵一次 + 快照 + 宿主动作队列），与 wasm 侧
        50ms 泵同构，前端 `start_pump()` 的语义两端一致。
      - 实测：桌面双实例 **10/10**；**桌面 ↔ 鸿蒙跨宿主 10/10**（两个宿主各写各的
        `Host` 实现，跑同一份状态机）；两端资源时间线里都没有 transport wasm。
      - **安卓**（AVD `goptop_test`，x86_64）：`ai.js android` 9/9、
        `stress.js android 120` 8/8（120 手，堆 18→18MB）；资源时间线复核
        **`goptop_transport` 与 `goptop_core` 皆为空**（上一轮 transport 一直在），
        即 P2P 会话与规则在安卓都走原生 Rust；`__store.backend()` 为 `tauri`。
        构建经验：`tauri android build` 与模拟器**不能同时跑**（两次都因此被系统
        在内存临界时回收），**先前台单独跑构建、再起模拟器**即可通过。
- [x] 遗留清账：`crates/goptop-ai/src/go.rs` 的 `UCT_C` 死常量已随 2026-10-07 轮
  （db54a99）删除，wasm 产物与三端壳资源同步重出（35ffdd3）。

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

## 2026-09-18 全维度实机测试轮（用户指令「完成全部实机测试」）

- [x] 五端（web 桌面/手机浏览器、桌面壳、安卓、鸿蒙）两两对战矩阵 11/11 全通，真实点击下到分出胜负。
- [x] 聊天/悔棋/换棋/重开矩阵 11/11；观战矩阵（含观战者申请发言、踢人）8/8 全通。
- [x] 设计功能实机验证：本地对战（5 端）、围棋提子+双停一手+终局计分（6 组合）、
      大厅挑战（3 组合）、认输（5 组合）、观战发言、踢人。
- [x] 修复 7 个产品缺陷（详见 memory/2026-09-18-multi-device-real-test-round.md）：
      鸿蒙分享链接不可达、服务器模式粘贴邀请无效、服务器模式无观战链接、
      粘贴观战链接被当对局 join、乱序落子永久丢失、围棋终局 UI 缺失、认输判错胜负。
- [x] 两个布局/交互修复：棋盘尺寸正反馈塌陷（桌面壳棋盘 50px→202px）、
      窄屏聊天弹窗遮挡协商横幅。
- [ ] 待跟进（本轮未做）：短窗口（约 760px 高）下棋盘仍受卡片总高限制（约 200px），
      若要更大需重排对局页卡片结构——属设计取舍，需用户拍板。

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
