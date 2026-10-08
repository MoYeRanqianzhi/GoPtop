# 2026-10-08 Agent 对战大轮（阶段①-④）：一切皆文件工具面 + 内置/MCP 双驱动落地

用户拍板立项（权威规格 `.agents/plan/2026-10-08-agent-battle.md`，计划先行 commit 80eb5b4）：
新增「Agent 对战」——模型玩家等同人类玩家（真思考、会犯错、完整操作面），唯一入口
`/agent`，对手驱动二选一：**内置**（应用配置的 LLM 循环驱动 B 席）或 **MCP**（外部
Agent 经内嵌 rmcp streamable HTTP 服务器认领席位）。本轮落地阶段①-④；⑤（Web 内置
模式）与⑥（本文档收尾）见 TODO。

## 数字与基线

- 提交链：骨架 7482f50 → A/C/D/E 四路并行 8ec4f4d/283e98b/a0b34a7/727060d（B 路超时，
  见「编排插曲」）→ 集成 5b114b0 → 审查修复 01d4466 → 阶段③ G 路 9d777b9 / F 路
  637855c / 对齐修复 466f201 / e2e 3ab5466 / 审查修复 157fb86 → 紧凑修复 1e60ab7
  （用户反馈驱动）→ 阶段④ 真 LLM 脚本 58f48bb + H 路 d969b27 + 壳接线与 e2e b1365f2
  → 文档 bdb61f4 → ④审查修复 6f9326b。
- 回归（本轮实测）：`cargo test -p goptop-agent --features mcp` 100 绿
  （单测 78 + loop_headless 17 + mcp_server 2 + memory 3）；frontend vitest 94/94。
- e2e 面（本轮实测计数）：agent-builtin.js 22/22（壳级 Mock 全链）、agent-entry.js
  22/22（入口唯一性与降级）、agent-mcp.js 44/44（node 最小 MCP 客户端打整局：401×2/
  tools/list 8 工具/game_start 认领/幽灵子→submit→实子/聊天往返/resign 终局/game_leave/
  关停）、agent-mock-llm.js（本地协议桩）、agent-reallm.js（真 LLM 门槛，见下节）。

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

## 编排插曲（只有编排者知道的叙事，为什么代码长这样）

- **B 路（store+vfs）API 超时阵亡**：5 路并行里唯一死掉的——vfs.rs 整文件 23 处
  `todo!()` 由集成 agent 兜底全量落地。兜底可行正因骨架先行：契约 doc 写死了语义，
  接手者不需要猜任何意图。**骨架的第二个价值是可救场。**
- **D 路的骨架缺口与窄例外**：统一消息形态缺 `Block::ToolCall` 变体——没有它三协议
  都无法回放工具调用配对（Anthropic 严格交替），compact 的「绝不拆对」也无从判定。
  D 路在自己名下补了变体，但连带 compact.rs 两处穷尽 match 编译红；按所有权纪律
  不碰 E 路文件，报编排者裁决。批准**窄例外**（仅补两分支、其余 compact 语义不动），
  并行阶段才有快速裁决通道——否则一路被别人文件的红编译卡死整个 parallel。
- **文档由上下文持有者亲写（用户拍板）**：本轮最初把落地记录派给独立文档 agent，
  用户指出「谁知道开发的 agent 在想什么为什么这么写」——开发叙事（本节这类 WHY）
  只存在于编排者与开发路的上下文里，新 agent 只能从代码反推。今后：开发叙事并入
  开发路任务书（边开发边记）或编排者亲笔；独立文档 agent 只做规格可推导的机械部分
  （使用手册章节）且须编排者审校。

## 阶段③集成的四处真接缝（壳级实测才暴露——单元/类型层全绿盖不住接线）

1. `RunInner.staging` 声明并读取但**从未赋值**：stagedMove 恒 null、幽灵子永不渲染
   ——类型系统抓不住「接线断了」这种错，只有端到端点亮 UI 才现形。
2. 我执黑流程初版**先等邀请就绪再 bind**，而代发 `CreateInvite` 的正是 bind——互相
   等死 40s。正确顺序：建局→bind→等 rtc→start。
3. A' 的泵把状态机的 `Nav("/p2p")` 应到 SPA（受理回执/挑战时 lobby.rs 必发），整页被
   拖离 /agent→cleanup 直接拆局。修法：`createSession(..., { suppressNav })` 页面级
   导航对 A' 失效。
4. B 的 rtcAns 回执以 challenge 形态走进程内 presence 广播（无 to 过滤），主会话误弹
   「Agent 邀请你加入对局」——/agent 下不渲染邀请弹窗。

## 阶段④审查修复（6f9326b，4 条全实证）

- **token 重新生成不生效**：token 只在 `McpServer::start` 烘焙进中间件，按钮只写
  store——运行中旧串仍过鉴权、新串一律 401，与 UI/文档宣称恰好相反。修：重新生成时
  服务器在跑则同步重启；「复制 .mcp.json」按真相分型（运行中取实例、否则取输入框）。
- **KEY_MCP_ENABLED 只写不读**：重启后开关显示已启用而服务器未运行，开局报错引导
  用户去开一个开着的开关。修：setup 挂 `mcp_autostart`（桌面门）。
- **真 LLM 门槛的「≥5 手」数的是下达命令不是落盘子**（LogPlayer ok 恒 true）——改
  盘面口径（终局手数−我方落盘），环计数降为旁证。
- **e2e 落子是 dispatchEvent 合成事件**（isTrusted=false，头注却称真实点击）——改
  `page.mouse.click` 名实相符（今天等价因 BoardSvg 无 isTrusted 判定与 stopPropagation，
  但描述失真会掩盖未来覆盖层回归）。

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
- **门槛实过（run 5 终局判定）**：我执黑 vs deepseek-v4.1-flash（Agent 执白）——
  Agent 5 手真实落子第 8 行连五**真赢**（(8,8)(7,8)(6,8)(9,8)(10,8)，我方脚本盲下未挡）；
  聊天 2 条且告别词准确复述制胜逻辑（「第8行连成四子、两边活口、落 10,8 成五」）；
  32 次 LLM 调用、入 10,146 tok / 出 19,137 tok、压缩 0、整局 134.3s、全程无 error 态；
  9/9 PASS 退出码 0，终局截图目检（白胜横幅/连五盘面/双聊天/用量卡/[llm] HTTP 200）。
  run 1-4 迭代失败均非零退出并留诊断——门槛不是一次跑出来的。

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
