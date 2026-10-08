# 2026-10-08 Agent 对战大轮（阶段①-④）：一切皆文件工具面 + 内置/MCP 双驱动落地

用户拍板立项（权威规格 `.agents/plan/2026-10-08-agent-battle.md`，计划先行 commit 80eb5b4）：
新增「Agent 对战」——模型玩家等同人类玩家（真思考、会犯错、完整操作面），唯一入口
`/agent`，对手驱动二选一：**内置**（应用配置的 LLM 循环驱动 B 席）或 **MCP**（外部
Agent 经内嵌 rmcp streamable HTTP 服务器认领席位）。本轮落地阶段①-④；⑤（Web 内置
模式）与⑥（本文档收尾）见 TODO。

## 数字与基线

- 提交链：骨架 7482f50 → A/C/D/E 四路并行 8ec4f4d/283e98b/a0b34a7/727060d → 集成
  5b114b0 → 审查修复 01d4466 → 阶段③ G 路 9d777b9 / F 路 637855c / 对齐修复 466f201 /
  e2e 3ab5466 / 审查修复 157fb86 → 阶段④ H 路 d969b27 + 壳接线与 e2e b1365f2。
- 回归（本轮实测）：`cargo test -p goptop-agent --features mcp` 100 绿
  （单测 78 + loop_headless 17 + mcp_server 2 + memory 3）；frontend vitest 94/94。
- e2e 面：agent-builtin.js（壳级 Mock 全链）、agent-entry.js（入口唯一性）、
  agent-mcp.js（node 最小 MCP 客户端打整局）、agent-mock-llm.js（本地协议桩）、
  agent-reallm.js（真 LLM 门槛）。

## crate 结构（crates/goptop-agent，平台无关）

`player.rs`（PlayerHandle/EmitWatch watch 通道/HookHost 装饰 TauriHost：随机临时
userId+stun 强制空+emit 写 watch——零 transport 改动的事件物化）；`pair.rs`（进程内
双向配对，复刻 tests/headless.rs 的 Boot 路径，方向随执色）；`tools.rs`（9 工具
`static` 纯数据+手写 JSON Schema——**不引 schemars**，一份 schema 喂 rmcp 与三协议；
`static` 而非 `const` 是因为要借 `&'static ToolDef`）；`registry.rs`（execute 一口
分发；`ToolError::{RespondToModel→isError 文本, Fatal→循环退出}` 两级错误）；`vfs.rs`
（/game 动态即时合成、/game/in/ 两段式「写=暂存、submit=执行」、/index）；`agent_loop.rs`
（五退出条件：winner/resign 终止/用户停止/Fatal/硬预算 max_llm_calls=240；事件自动
推送进对话；**纯文字轮不终止**）；`compact.rs`（上限 176k 默认、触发线=上限−RESERVE、
切点只在 user 消息处）；`llm/`（enum LlmClient：anthropic/openai_responses/openai_chat/
mock + HttpChannel trait，native=reqwest、web 钩子留给⑤）；`store.rs`（VfsStore trait，
native=rusqlite bundled `~/.goptop/agent-memory.db`，ns 按驱动分 builtin/mcp，
单文件 256KB/每 ns 8MB）；`mcp.rs`（feature "mcp" 只在桌面编译）。

接缝：`src-tauri/src/agent.rs` AgentHub 只做装配（agent_* 命令面+会话对结对+循环
tokio 任务），一行决策语义不重做；`frontend/src/pages/AgentPage.tsx` 唯一入口。

## 并行实现编排（本轮验证有效的手法）

- **骨架先行**：先落全 crate 类型契约（所有模块的 struct/enum/函数签名，编译不过
  不许并行），再 5 路并行填肉——各路只依赖骨架签名，互不碰文件，集成时才发现的
  语义分歧几乎为零。
- **F/G 双路接壳的接口漂移**：F 路（src-tauri）初版让 Hub 自建 A'，G 路（前端）
  实装成 A' 归前端会话表（onChange 单槽注不进第二个会话，A' 必须前端自管）——
  466f201 收敛为「A' 归前端，`agent_bind` 登记+代发邀请；我执白方向 B 先出链、
  链接经 `agent_status.detail` 送回」。**教训：并行两路共享的「谁拥有 X」所有权
  问题必须在骨架阶段拍死，不能各自假设。**
- 壳构建串行（内存纪律），多 Workflow 文件范围互斥。

## 审查发现模式（两轮审查的共性）

- **防恒真**：e2e 断言先证明它能红（157fb86 修掉一处恒真断言）——与 2026-10-07 轮
  「报告级发现必须实证」同源。
- **拦截面豁免要随局记账**：Agent 局豁免主会话开局命令的豁免列表，局终必须清，
  否则下一局/主会话被永久拦死。
- **平台门要门在编译期**：MCP 命令 `cfg(desktop)` 才注册（不是「注册了但报错」），
  前端 `isTauri()` 门卡渲染——两端一致才不出现「按钮点了没反应」。
- **设置落盘必须可等待**：store_set fire-and-forget 与 agent_start 并发无先后承诺，
  开局会读到上一份配置（错 key/错模型）——`storeSetAsync` + await 后才 invoke。
- **UI 紧凑红线再验证**：棋盘 18px 状态行长文案必折行挤棋盘（1e60ab7），Agent 页
  沿用 P2P 极短文案「黑 落子」，语义交给侧栏状态卡。

## rmcp 3.2 接入（R3 风险的实测解法）

feature 实际拼写 `server` + `transport-streamable-http-server`，实施首日**双重验证**
（下载源 `~/.cargo/registry/src/*/rmcp-3.2.0/Cargo.toml` 的 [features] 定义 + 范本
`.ref/codex/codex-rs/tui/src/dynamic_tools_mcp.rs` 的 Cargo.toml 同款挂法），与计划
预写一致零调整。workspace 统一 default-features=false（auth/schemars 宏面/client
不进树；rmcp 内部对 schemars 的依赖是它的实现细节，与本仓「不引 schemars」不冲突）。

## 真 LLM 门槛（用户拍板，agent-reallm.js）

- **凭据红线**：端点/key/模型只从环境变量读（GOPTOP_TEST_LLM_URL/KEY/MODEL），
  三缺一即 SKIP(0)，**绝不写进任何进 git 的文件**——脚本本身可安全入库。
- **诚实性口径**：门槛是「真实 LLM 链路功能成立」（开局聊天在案、真实落子 ≥5 手、
  全程无 error 态、llmCalls>0 且 tokensIn>0）；模型棋力差/拒聊/思考慢不是失败，
  链路级失败（Fatal/卡死/无落子）才是 FAIL 且附完整诊断。
- e2e 失败必非零退出、finally 清理子进程（a5bd52c 与 agent-builtin.js 先例延续）。

## 遗留（记录在案，未获指令不动工）

1. **阶段⑤ Web 内置模式**：VfsStore web 后端（IndexedDB 经 agentVfs.ts 的
   window.goptopVfs* 钩子）+ HttpChannel web 通道（window.goptopAgentHttp）+
   循环 wasm 驱动（await 走 wasm-bindgen-futures，禁 tokio time）；当前 Web/鸿蒙
   进 /agent 只见降级横幅。
2. **phase ⑤ 后产物同步**：build-wasm/build-ohos 冒烟与既有 e2e 全量（本轮纯桌面
   接线，wasm 产物零变化——goptop-transport/net/core 未动）。
3. **MCP 服务器大厅注册**属下一任务（registry scope 枚举与 handler-state 已留缝）。
4. 既有遗留不动：A 方案（stable 态用原 offer 重建 peer）、壳/模拟器实测矩阵待内存
   宽裕重跑（见 2026-10-07 轮）。
