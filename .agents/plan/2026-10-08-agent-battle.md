# GoPtop「Agent 对战」实施计划

## Context（为什么做 / 用户拍板口径）

新增**「Agent 对战」**：Agent 身份等同于玩家（非人类的模型玩家，真思考、会犯错，区别于 goptop-ai 的算法 AI），拥有人类玩家完整操作面（看棋盘、落子、停一手、悔棋请求/批复、聊天、认输、计分确认）。用户已拍板：

- **唯一入口**：不分子功能。`/agent` 一个页面，用户在对局设置里选**对手驱动方式：内置（应用配置的 LLM 循环）或 MCP（外部 Agent 经内嵌 MCP 服务器驱动）**。同一会话对、同一棋盘、同一聊天——差别只在 B 席的驱动者。
- **纯本地**：全程不经过服务器。「Agent 注册大厅自由对战」是下一个开发任务，本次不做，只留扩展缝。
- **Agent 无身份**：不注册、不落盘、无持久身份，就是本地对战中用户对面的那一席。
- **初始执子由用户定**：对局设置里选「我执黑 / 我执白」。
- **三种 LLM 协议**（内置模式）：Anthropic Messages / OpenAI Responses / OpenAI Chat Completions。
- **上下文上限默认 176_000**（hard ceiling，用户可设置；compact 触发线在上限之下弹性提前，触发线不是用户配置项）。
- **一切皆文件**（Linux 思路，防工具无限膨胀）：对局状态、规则、历史局面、聊天全部合成**动态只读文件**进虚拟文件系统，grep/read 即可灵活查看当前与历史局势；**动作=写文件**（坐标写进落子文件即落子；消息写进输入框文件再调 `send` 即发送）。工具收缩为文件族 read/write/edit/grep + send，MCP 模式另加 wait_events。
- **事件推送分模式**：内置模式**无需等待工具**——事件自动推送（工具间隙注入对话）；MCP 模式专用 `wait_events` 工具——队列里有事件则瞬间一次全量返回，没有则阻塞等待（它既是等待也是排空）。
- **循环纪律**：只回文字不算行动，循环不得因此停下；每轮以工具调用收束（不强制落子）。
- v1 不含 Agent vs Agent。MCP 模式仅桌面（TCP listener 固有）；内置模式桌面+Web。

关键洞察（已一手验证）：`UiCommand`（goptop-net/src/session/mod.rs:467-519）唯一命令口、`Session::snapshot()` 唯一读口、`tests/headless.rs` 已证明同进程双会话完整对局——**Agent 玩家 = 无头会话 + 决策层，协议零改动**。内置与 MCP 两模式共用同一会话对与同一工具 registry。

## 架构

```
frontend (React, 只做 UI)
  AgentPage(/agent) 唯一入口
    对局设置卡：棋种/路数/我执黑·我执白/对手驱动(内置|MCP)/LLM配置(内置时)/MCP连接卡(MCP时)
    BoardPanel+ChatPanel+ConfirmBanner（复用，渲染 A'） + Agent 状态卡(状态/工具日志/token/停止)
        │ Tauri invoke
src-tauri
  session.rs（不动）  agent.rs（新：AgentHub——agent_* 命令、会话对生命周期、session_cmd 拦截面、MCP server 生命周期）
        │
crates/goptop-agent（新，平台无关，不含 tauri/rmcp 主干）
  player.rs    PlayerHandle + NativePlayer + EmitWatch + HookHost(临时去重值/stun=[]/emit→watch)
  pair.rs      A'/B 进程内配对（复刻 headless.rs:90-125，方向随执子选择）
  tools.rs     ToolDefinition 纯数据 + 手写 JSON Schema（不引 schemars）
  vfs.rs       虚拟文件系统：路径解析（/game 动态合成器｜/memory VfsStore）＋动态文件合成
               （status/board/rules/history/history/<n>/chat/in/* 控制文件——写即动作）
  registry.rs  文件族工具表（read/write/edit/grep/send + wait_events/game_*）＋错误 RespondToModel/Fatal
  agent_loop.rs 决策循环（内置模式驱动 B 席；steering/预算/终止，骨架对齐 .ref/pi/packages/agent/src/agent-loop.ts）
  compact.rs   上下文上限 176k + 弹性触发线压缩（移植 .ref/pi/.../compaction/compaction.ts）
  prompt.rs    系统提示词（Claude Code 风格简化版，中文）
  llm/         LlmClient enum：anthropic / openai_responses / openai_chat / mock
               ＋HttpChannel trait（native=reqwest；web=TS fetch 钩子）
  memory.rs    虚拟文件系统 read/write/edit——VfsStore trait：
               native=rusqlite(~/.goptop/agent-memory.db)；web=IndexedDB（经 TS 钩子）
  mcp.rs [feature="mcp"]  rmcp ServerHandler + streamable HTTP 出口
               （对齐 .ref/codex/codex-rs/tui/src/dynamic_tools_mcp.rs）
frontend/src/net/agentVfs.ts（web 后端：IndexedDB + window.goptopVfs* 钩子）
```

新增 workspace 依赖：`reqwest 0.13 (rustls-tls+json)`、`rusqlite 0.37 (bundled)`、`axum 0.8`、`rmcp =3.2.0 (default-features=false；feature 侧开 server + transport-streamable-http-server，实施首日验证 3.2 的 feature 实际拼写)`。**不引 schemars**（lock 内 0.8/1.x 并存；手写 `serde_json::json!` schema 一份喂 rmcp 与三协议）。

## 会话对（两模式共用地基）

1. **A 是专用会话 A'，不是全局会话**：开始对局时经门面创建（固定 `serverMode:false`），自管 poll；`frontend/src/net/session.ts` 适配器加**可选 onChange 注入**（缺省仍走 window 钩子，主会话零改动）。理由：全局会话可能开着服务器模式（无 `rtc=` offer，配对路径不通），且 `window.goptopOnChange` 是单槽。
2. **Agent 无身份**：不注册、不落盘。仅因 bc.rs:129-131 按会话 userId 过滤回声（`from==me` 丢弃，读取点 ensure_user_id session.rs:178-188），B 的会话需要一个与 A' 不同的**会话级临时随机值**：`HookHost` 装饰 TauriHost，对 `goptop:userId` 返回每局随机的临时值（**不写存储**）；`goptop:stun` 强制 `"[]"`（免 8s gathering，headless.rs:26-30 手法）；其余键透传；`emit` 覆写为写 watch 后透传。这只是去重键，无任何身份语义。
3. **初始执子由用户定**：「邀请人=黑」的构造结果（lobby.rs:221/286）恰好实现谁邀请谁执黑。我执黑（默认）=A' 先 `CreateInvite`、B 以其链接 Boot 入局；我执白=反向——B（无头会话）先 `CreateInvite`，A' 携 B 的链接经 Boot 路径创建（session_new 的 url 参数）。**两个方向都走已验证的 Boot 路径**，避开 AcceptInvite+rtc 的已知空档（lobby.rs:256-279、headless.rs:84-89 注释）。
4. **单局互斥**：进程内 BC hub 全局无局号（bc.rs:12-17 自证缺口）→ 同一时刻最多一个 Agent 局；Agent 局存活期间 `session_cmd` 拦截面拒绝主会话开局类命令（CreateInvite/AcceptInvite/AcceptReceipt/ServerChallenge/AcceptChallenge，豁免 A' id），回 notice「Agent 对局进行中」。
5. **配对流程**（pair.rs，方向随执子选择）：邀请方 `PickKind/PickSize`→`CreateInvite`→轮询 `inviteUrl` 含 `"rtc="`（≤40s）→ 受邀方以该 URL 经 Boot 路径创建 → 双方 `phase=="playing"` 且 `peerConnected`；受邀方创建即 `start_pump()`。失败→清理、报错可重试。
6. **事件等待**：`EmitWatch = tokio::sync::watch<(seq, snapshot)>`，由 `HookHost::emit` 推送（命中 bridge.rs:21-27 既有钩子，不改 transport/net 一行）。工具层做谓词等待：对手落子/新 confirmReq/chat 增长/winner/scoring。

## 一切皆文件：虚拟文件系统布局（内置/MCP 同一 registry、同一布局）

两棵子树：**`/game`（动态合成，对局存活期存在）**与 **`/memory`（持久记忆，VfsStore 落库）**。动态文件不落盘——read/grep 时从会话快照即时合成；`/history/<n>` 按需合成。`/index` 列出全部路径与一句话说明（模型第一眼的可发现性）。

```
/index                全部路径索引与说明（只读，动态）
/game/status          轮次/我的执色/是否我行棋/终局与胜者/计分状态/待批复请求/对方名（只读，JSON）
/game/board           当前局面·稀疏 JSON（stones 坐标表 + staged_move + last_move；只读；默认最省 token）
/game/board/grid      全量模拟棋盘 JSON：二维数组 grid[y][x]，每格 "empty"|"black"|"white"|"staged"
                      ——**不分 stones/staged，整盘每格都有描述**（模拟棋盘的 JSON 版，有时更直观）
/game/board/ascii     纯 ASCII 模拟棋盘；/game/board/pretty = ●○+边框模拟棋盘；
                      /game/board/image.png = PNG 渲染（协议/模型不支持视觉→占位符文本）
/game/rules           当前棋种规则文本（五子棋/围棋各自内容；只读，markdown）
/game/history         JSONL 落子历史（{"n":1,"by":"black","x":7,"y":7}，pass 记 "pass":true；只读）
/game/history/<n>     第 n 手后的局面，**与 /game/board 同款五变体**（默认 JSON/grid/ascii/pretty/image.png）
/game/chat            聊天记录全文（散文逐行，含自己发的；只读）
/game/events          **全部事件历史 JSONL**（seq 单调递增：move/pass/chat/request_received/
                      request_resolved/scoring_started/score_result/game_over；只读；
                      与 chat 的区别=完整事件流，read offset/limit 翻页、grep 回查任意过往事件）
/game/in/move         【写=暂存】着法 "x,y" 或 "pass"（围棋）；write 只做格式校验并回预览；
                      **暂存落子上盘可见**：/game/board 与 status 以 `*`/pending 字段渲染，
                      AgentHub 同步把暂存坐标报给前端——AgentPage 棋盘画「幽灵子」
                      （半透明描边，与黑白实子明确区分），人能提前看到 Agent 打算下哪
/game/in/chat         【写=暂存】消息文本
/game/in/request      【写=暂存】"undo"|"reset"|"swap"
/game/in/confirm      【写=暂存】"approve"|"reject"（批复当前待决请求）
/game/in/score        【写=暂存】"ok"（确认计分）
/game/in/resign       【写=暂存】任意内容（认输）
                      —— **in/ 全部两段式：write=暂存（可反复覆盖、可 read 回看），
                      submit(路径) 才执行**。给 agent「落笔后、提交前」的反悔窗口；
                      格式错误在 write 即拒，对局规则错误（未轮到/占点/无待决/非计分态）
                      在 submit 执行时拒并回人话
/memory/...           长期记忆（持久；路径规范化拒 ../空/>256 字符；单文件 256KB、每 ns 8MB）
```

**工具清单（内置 6 个（子代理默认关）/ MCP 7 个）**——`ToolDef{name, description, input_schema}` 纯数据 + 手写 JSON Schema；`execute → Result<Value, ToolError>`，`ToolError::{RespondToModel(String), Fatal(String)}`。路径解析规则：`/game` → 动态合成器；`/memory` → VfsStore；未知路径→错误并附 `/index` 提示。**工具描述里写全各文件的功能职责与用法**（in/ 各文件的取值协议、只读文件的内容结构都进 description——模型不看 /index 也能用对），/index 只是冗余索引。

| 工具 | 参数要点 | 返回要点 | 两模式 |
|---|---|---|---|
| `read` | path；**offset（起始行）、limit（最多行数）**——防长文件（聊天记录/history）撑爆上下文；返回含 total_lines 供翻页 | 文件内容；动态文件即时合成 | 共用 |
| `write` | path, content（整体覆盖）。对 `/game/in/*` 写=**暂存**（只做格式校验并回预览，不执行）；对只读路径写→拒绝；对 /memory 写=持久 | 暂存回执（路径+内容预览+「用 submit 提交」）或记忆写入回执 | 共用 |
| `submit` | path（**按路径自动判别提交什么**：in/move→落子、in/chat→发消息、in/request→发起请求、in/confirm→批复、in/score→确认计分、in/resign→认输） | 执行结果（落子结果/发送回执/请求已发/批复结果/计分结果/认输 terminate） | 共用 |
| `edit` | path, old_string, new_string, replace_all?=false（仅 /memory；0 次或 >1 次不唯一→报错回模型，Claude Code 语义） | occurrences/size | 共用 |
| `grep` | pattern（正则），path 前缀（如 `/game/history`、`/memory`） | 匹配行+所在路径——对动态文件即时合成后匹配，**grep 历史局面=灵活回看任意手数后的局势** | 共用 |
| `delegate` | task（交给子代理的任务描述，如「深思当前局面给出候选点评估」） | 子代理最终结论文本。**子代理=同 LLM 独立上下文的小循环**（只读工具 read/grep，深度 1 不许再嵌套，独立小预算 maxLlmCalls），结论作为工具结果回父代理——深思不污染主上下文。**默认不启用，设置卡可选启用**（`goptop:llm-config.enableSubagent`） | 仅内置 |
| `wait_events` | timeout_secs=0（0=只排空；>0≤120=阻塞等） | 队列里**全部**待处理事件一次返回（`move{coord,by}`/`chat{text}`/`request_received/resolved`/`scoring_started`/`score_result`/`game_over{winner}`）；空超时=`[]`（正常返回非错误） | 仅 MCP |
| `game_start` | 无必填（棋种/路数/执色以用户 UI 配置为权威） | started/my_color | 仅 MCP |
| `game_leave` | — | 局中自动认输后离场 | 仅 MCP |

**事件物化**（player 层，零 transport 改动）：EmitWatch 每拍快照与上一拍 diff → 同一事件流喂两个出口：**事件队列**（新事件待消费）+ **`/game/events` 全量历史文件**（append-only，seq 单调）。事件种类：move_count 增→move（附坐标）、pass→pass、chatLog 增长→chat（附全文）、confirmReq 出现/消失→request_received/resolved、scoring 翻真→scoring_started、scoreResult→score_result、winner→game_over。**内置模式：循环在工具间隙自动排空队列、以 `<event>` 块注入对话（无等待工具）；MCP 模式：wait_events 排空/阻塞**。

**记忆库双后端**（VfsStore trait，只服务 `/memory` 子树；表结构/配额/路径规范两端一致）：`files(ns, path, content, size, updated_at, PK(ns,path))` + `meta(ns, bytes_used)`；ns 按驱动分：内置=`builtin`、MCP=`mcp`。native=rusqlite bundled `~/.goptop/agent-memory.db`；**web=IndexedDB，经 `frontend/src/net/agentVfs.ts` 暴露的 `window.goptopVfs*` 钩子（与仓库既有 window.goptop* 钩子模式同款），wasm 编译期 cfg 选后端**。不用 store.json/store KV（单值 4MB 上限、无事务语义、避免与 UI 设置互相膨胀）。

### 格式样例（权威基线——实现与审查逐字比对，防止偏移）

`/game/status`：
```json
{ "kind": "gomoku", "size": 15, "you": "white", "opponent": "小明",
  "phase": "playing", "to_move": "black", "move_count": 1, "winner": null,
  "scoring": false, "pending_request": null, "staged_move": { "x": 8, "y": 8 } }
```

`/game/board`（默认·稀疏 JSON，最省 token）：
```json
{ "kind": "gomoku", "size": 15, "you": "white", "to_move": "black",
  "move_count": 1, "winner": null, "last_move": { "by": "black", "x": 7, "y": 7 },
  "stones": { "black": [[7,7]], "white": [] },
  "staged_move": { "x": 8, "y": 8 } }
```

`/game/board/grid`（全量模拟棋盘 JSON——不分 stones/staged，整盘每格都有描述）：
```json
{ "kind": "gomoku", "size": 15,
  "grid": [ ["empty","empty","…"], …, 
    ["empty","empty","empty","empty","empty","empty","empty","black","empty","empty","empty","empty","empty","empty","empty"],
    ["empty","empty","empty","empty","empty","empty","empty","empty","staged","empty","empty","empty","empty","empty","empty"], … ] }
```
（grid[y][x]，值域 `"empty"|"black"|"white"|"staged"`；上例 y=7/x=7 黑子、y=8/x=8 暂存）

`/game/board/ascii`（单空格分隔；`/game/board/pretty` 用标准制表符 `┌┬┐├┼┤└┴┘` 画围棋式交叉点盘，`●`黑 `○`白 `◍`暂存）：
```
15x15 gomoku   X=black O=white *=staged . =empty
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14
 0  . . . . . . . . . .  .  .  .  .  .
 7  . . . . . . . X . .  .  .  .  .  .
 8  . . . . . . . . * .  .  .  .  .  .
```

`/game/board/image.png`：Rust `image` crate 手绘栅格（网格线+黑白圆子+暂存虚线圈，无字体无坐标标签——精确坐标以 JSON 为准）。**承载能力矩阵**：Anthropic tool_result 原生 image block ✓；MCP `CallToolResult type:"image"` ✓；OpenAI Chat Completions / Responses 工具结果纯字符串 ✗ → 占位符文本 `"image unavailable for this model/protocol — read /game/board/ascii instead"`。

`/game/history`（JSONL；`/game/history/<n>`=第 n 手后局面，五变体与 board 同款）：
```
{"n":1,"by":"black","x":7,"y":7}
{"n":2,"by":"white","x":8,"y":8}
{"n":3,"by":"black","pass":true}
```

`/game/events`（全量事件历史 JSONL，seq 单调；与事件队列同源两出口）：
```
{"seq":1,"t":"chat","from":"小明","text":"你好，请多指教"}
{"seq":2,"t":"move","by":"black","x":7,"y":7}
{"seq":3,"t":"chat","from":"Agent","text":"你好！请多指教。"}
{"seq":4,"t":"request_received","kind":"undo","from":"小明"}
{"seq":5,"t":"request_resolved","kind":"undo","approved":true}
```

`/game/chat`（散文逐行）、`/game/rules`（markdown）、`/index`（文档）：
```
# Virtual filesystem index
Read-only (synthesized from live game):
  /game/status     turn, colors, winner, scoring, pending request, staged move
  /game/board      current board (default JSON, cheapest) — variants: /grid /ascii /pretty /image.png
  /game/rules      rules of the current game kind
  /game/history    move list (JSONL)  |  /game/history/<n> = position after move n (same five variants)
  /game/chat       chat log (plain lines; long → read with offset/limit)
  /game/events     full event history (JSONL, seq-increasing)
Stage-then-submit (write to stage, submit(path) to commit):
  /game/in/move     "x,y" or "pass"          → submit places your stone
  /game/in/chat     message text             → submit sends it
  /game/in/request  "undo"|"reset"|"swap"    → submit sends the request
  /game/in/confirm  "approve"|"reject"       → submit answers pending request
  /game/in/score    "ok"                     → submit confirms scoring
  /game/in/resign   (anything)               → submit resigns
Persistent memory (survives across games):
  /memory/...       free-form files, e.g. /memory/notes/opponent-style.md
```

write/submit 返回与错误文案：
```json
{ "ok": true, "staged": { "path": "/game/in/move", "value": {"x":8,"y":8} }, "note": "call submit(\"/game/in/move\") to place" }
{ "ok": true, "action": "move", "move_count": 2, "finished": false }
{ "error": "(7,7) is occupied by black. Choose an empty intersection — read /game/board." }
{ "error": "not your turn (black to move). Opponent action arrives as a pushed event." }
{ "error": "invalid move \"abc\". Expected \"x,y\" (e.g. \"7,7\") or \"pass\"." }
{ "error": "no pending request. Check pending_request in /game/status." }
{ "error": "/game/board is read-only (synthesized from live game state)." }
```

**文档双轨（用户拍板）**：`/index` 列全各文件职责；**核心文件（in/ 六个的取值协议、board 变体清单）必须同时写进 write/submit/read 的工具描述**——提示词、/index、工具描述三处互为冗余，模型漏看其一也能用对。

记忆库（**VfsStore trait，双后端，表结构/配额/路径规范两端一致**）：`files(ns, path, content, size, updated_at, PK(ns,path))` + `meta(ns, bytes_used)`；ns 按对手驱动分：内置=`builtin`、MCP=`mcp`。native 后端=rusqlite bundled `~/.goptop/agent-memory.db`；**web 后端=IndexedDB，经 `frontend/src/net/agentVfs.ts` 暴露的 `window.goptopVfs*` 钩子（与仓库既有 window.goptop* 钩子模式同款），wasm 编译期 cfg 选后端**。路径规范化（拒 `..`/空/>256 字符）；单文件 256KB、每 ns 8MB。不用 store.json/store KV（单值 4MB 上限、无事务语义、避免与 UI 设置互相膨胀）。

## 内置模式：决策循环（agent_loop.rs）

maybe_compact → `llm.chat(ctx, tools)` → ToolCalls 逐个 execute 回填（RespondToModel→isError 文本），全部 terminate 则 break；**事件自动推送（内置模式无等待工具）**：每轮工具执行完毕后，排空事件队列，把新事件以 `<event>` 块注入 user 消息再进下一轮 LLM 调用——对手落子/消息/请求/终局无需模型主动查询；队列为空且轮到对手时，循环注入「等待对方行动中」心跳说明，模型可选择做别的（写记忆/委托 delegate 深思/聊天）或直接等。**TextOnly（只回文字、零工具调用）≠ 行动：注入提醒「必须通过工具行动（write 暂存 + submit 提交皆可）」并继续循环，绝不因此终止、绝不自动认输**。循环退出仅五种：winner 出现（write /game/in/chat 收尾语 + submit 后 break）、submit("/game/in/resign") 的 terminate、用户 agent_stop、Fatal（连接/配置损坏）、硬预算 maxLlmCalls=240。不强制每轮落子。stopReason=="length" 时本轮工具调用作废回错误（照 agent-loop.ts:474-500）。

**compact（compact.rs，移植 pi compaction.ts）**：上限=用户配置 176_000（`goptop:agent-ctx-limit`，clamp [8k, 1M]，设置卡可改；MCP 模式同值不另设参数）；触发线=`used > ceiling − 16_384`（RESERVE），保留尾部 ≈20_000 tokens（KEEP_RECENT；对齐 pi 默认 compaction.ts:157-161,246-249；触发线是引擎内部弹性空间，不暴露为配置）。计量：provider usage 为主（三协议 usage 归一）+其后消息 chars/4 估算。切点只在 user 消息处，绝不拆 assistant 工具调用与 tool_result 对。摘要：独立一次 LLM 请求，有 previousSummary 走增量更新；压缩后结构=`[system]+[user:<compaction-summary> 摘要]+[retainedTail]`。超窗兜底：错误特征 context_length_exceeded → 生效上限取 min(用户上限, 模型实限) → 强制压缩 → 重试一次 → 再失败 Fatal。

**系统提示词**（prompt.rs，**英文撰写，仿 Claude Code 分段风格**；末尾 `# Language` 段注入回复语言——默认=用户 UI 语言（前端经 agent_start cfg 传入），用户可在设置卡改成**任意字符串**（"简体中文"、"English"、"喵语"、"摩斯密码"……不校验、原样注入））：Identity（equal-footing player, allowed to err, admits being an AI; seat color per game setup）／**World model（置顶）：a virtual filesystem — `/game` holds live game files (board/status/rules/history/chat, read-only, dynamically synthesized), `/memory` is your long-term memory; read `/index` first. Workflow is always: write to stage, `submit(path)` to commit — staging gives you a chance to re-read and reconsider before anything happens. Placing = write coords to `/game/in/move` + submit; sending a message = write `/game/in/chat` + submit; undo/reset/swap = `/game/in/request`; approvals = `/game/in/confirm`; scoring = `/game/in/score`; resign = `/game/in/resign`. Use read's offset/limit for long files; grep `/game/history` to review the position after any move**／Rules digest（gomoku 15×15 five-in-a-row；go captures/suicide ban/two-pass scoring Chinese rules; full rules via /game/rules）／Event mechanism（builtin: opponent's moves/messages/requests are pushed to you automatically — no polling tool）／Action discipline（text-only is NOT an action, the loop never stops for it; you are never forced to move each turn; on rejection read the error, never retry the same coords blindly）／Etiquette（greet at start, resign gracefully, no spam）／Ending（on winner, write a farewell to /game/in/chat and submit）／**# Language（Always respond in {replyLang}）**。工具描述（ToolDef.description）写全每个文件的职责与用法——提示词与描述互为冗余，模型漏看一个也能用对。

**LLM 客户端**（llm/，非流式 v1）：`enum LlmClient { Anthropic, OpenAiResponses, OpenAiChat, Mock }`（enum 免 async-trait；Mock 供测试与无 key 演示）。统一 `ChatRequest{system, messages, tools, max_output_tokens}` / `ChatResponse{content, tool_calls, stop, usage}`。**HTTP 通道走 `HttpChannel` trait：native=reqwest；web=TS fetch 钩子（`window.goptopAgentHttp`，Promise 经 wasm-bindgen 回填）**——循环体与通道解耦，为 Web 端内置模式铺路。超时 60s，429/5xx 退避重试 2 次。三协议映射要点：Anthropic（顶层 `system`；tool_use 的 `input` 是对象；工具结果=**user 消息** tool_result；`max_tokens` 必填）；OpenAI Chat Completions（`messages[0]{role:"system"}`；`arguments` 是 **JSON 字符串**需 parse；结果=`{role:"tool"}` 独立消息）；OpenAI Responses（顶层 `instructions`；工具定义不嵌 function 壳；`function_call`/`function_call_output` 项；v1 `store:false` 全量重传）。错误分类：401/403/模型不存在→Fatal；context_length_exceeded→紧急压缩重试；429/5xx→退避，连续 3 轮失败→error 态。配置存 `store_set` 链：`goptop:llm-config`（protocol/baseUrl/model/maxOutputTokens=1024/**replyLang**）、`goptop:llm-key`（**明文，风险如实注明**）、`goptop:agent-ctx-limit`。replyLang 任意字符串、不校验，null=跟随 UI 语言（agent_start cfg 由前端传入当前 UI 语言作默认）。

## MCP 模式：内嵌服务器（本地，无服务器依赖）

- 生命周期：AgentPage 的 MCP 连接卡开关（默认关，`goptop:agent-mcp-enabled`）→ `TcpListener::bind("127.0.0.1:9537")`（端口可配 `goptop:agent-mcp-port`，占用回退随机口并回显）→ `StreamableHttpService` + `Router::nest_service("/mcp")` + Bearer token 中间件（照 dynamic_tools_mcp.rs:151-196）；token 随机持久化 `goptop:agent-mcp-token`，UI 可重新生成；UI 一键复制 `.mcp.json` 片段（url + Authorization 头）。
- 流程：用户选「对手驱动=MCP」+ 棋种/路数/执子 → 点开始 → A' 创建并显示「等待 MCP Agent 接入…」→ 外部 Agent `game_start()` 认领席位并完成配对（同一 pair() 函数；棋种/路数/执色以用户 UI 配置为权威）→ 整局文件系统与工具路由到该局（`/game` 动态文件、`/memory` ns=`mcp`）→ 事件经 `wait_events` 获取（队列非空瞬间全量返回，空则阻塞，它既是等待也是排空）→ `game_leave` 收尾。人侧未就绪 → `game_start` 回业务错误文本。对局就在 AgentPage 棋盘上进行，人正常落子/聊天/批复。服务器大厅注册属下一任务，仅在 registry scope 枚举与 handler-state 留缝。
- **条件编译（用户拍板）**：`mcp` feature 只在桌面目标启用（src-tauri 按 `[target.'cfg(desktop)'.dependencies]` 挂 feature；安卓/鸿蒙目标与 wasm 完全不编译 MCP 代码，非「编译了但禁用」）；前端相应**不渲染** MCP 卡（isTauri+桌面判定，Web 端连提示都不出）。MCP 相关 Tauri 命令（agent_mcp_set/info）仅在桌面目标注册。

## AgentPage（唯一入口，frontend/src/pages/AgentPage.tsx 新建）

- 路由：`links.ts` 加 `{mode:"agent"}` 与 `/agent`、`App.tsx` 分支、`MenuPage.tsx` 按钮「Agent 对战」。
- **对局设置卡**：棋种/路数、**我执黑/我执白（用户定）**、对手驱动（内置 | MCP）；内置→LLM 配置（三协议/baseURL/model/key/上下文上限/**回复语言（任意字符串，默认跟随界面语言）**/**子代理开关（默认关）**/测试连接）；MCP→连接卡（开关/端口/token/连接串复制/状态「等待接入·已连接」，仅桌面渲染）。
- 对局视图：BoardPanel 渲染 A'（disabled 同 boardDisabled 公式 snapshot.rs:134-145）+ ChatPanel（人的聊天/悔棋/换棋走真实按钮；Agent 的悔棋请求由既有 ConfirmBanner 弹给人）+ Agent 状态卡（状态/工具日志流/token 用量/停止按钮=认输+终止）。
- Tauri 命令（src-tauri/src/agent.rs 新建，lib.rs 注册）：`agent_start(cfg{driver, kind, size, myColor, agentName?})→u32`（agentName 默认「Agent」，仅聊天展示，非身份）、`agent_stop`（先 Resign 再 drop B，防孤儿局卡死 A'）、`agent_status`（pairing|waiting_mcp|thinking|waiting|done|error + llm_calls/tokens/compactions）、`agent_events(id, since)`（工具日志环≤200 条）、`agent_bind(sessionId)`、`agent_llm_test`、`agent_mcp_set/info`。
- 平台降级：Web=内置模式可用（LLM 经 HttpChannel web 通道，受端点 CORS 约束需在文档注明）、MCP 卡显式提示仅桌面；鸿蒙=整页显式降级（HTTP/钩子通道未接，列后续）。
- 围棋计分 v1 取舍：Agent 不标死子（工具集不含），循环见 scoring 自动 ConfirmScore，死子由人类单方标记（交集规则下生效）；`mark_dead` 工具列后续。

## 测试计划

1. **无头单测**（goptop-agent，真状态机+Mock LLM，复用 headless.rs 的 SERIAL 锁/pump_until 手法）：基础对局至五连（write in/move 暂存→submit 落子）／**submit 两段式语义**（write 只暂存不执行——断言落子前盘面不变；submit 前可覆盖重写；未 write 直接 submit→报错回模型；格式错误 write 即拒、对局规则错误 submit 时拒）／我执白（反向配对）各跑一遍／人请求悔棋→Agent write+submit in/confirm approve→两端一致／Agent write+submit in/request undo 被拒继续（事件送达 approved:false）／write+submit in/chat 双向（对方收到全文）／write+submit in/resign →循环 terminate、两端 winner 一致／文件面：/index 与 /rules 可读、/game/board 与快照一致、**/game/board/grid 每格值正确（staged 格正确呈现）**、/game/history/<n> 各手盘面与各变体正确、grep /game/history 命中指定手数、**/game/events 全量事件历史完整（seq 连续、grep 回查过往事件）**、**read 的 offset/limit 翻页与 total_lines**、写只读路径拒绝、in/confirm 无待决 submit 失败、in/score 计分态外 submit 失败／围棋双 pass→scoring→write+submit in/score 双确认到 score_result／记忆 round-trip 与 edit 不唯一报错／**delegate 子代理**（默认关：未启用时工具不在清单；启用后 Mock 子循环回结论、深度 1 不嵌套）／**纯文字轮不终止**（Mock 剧本先回零工具调用的纯文本→断言循环注入提醒继续、会话未停，随后只写 in/chat+submit 的回合也照常存活→再正常落子到终局）／**内置事件自动推送**（人落子后不调用任何查询工具，下一轮 LLM 请求的注入消息里含 move 事件坐标）。
2. **compact 单测**：超触发线切点不拆工具对／摘要后消息结构／previousSummary 增量／usage 计量。
3. **LLM 协议测试**：本地 TcpListener 裸 HTTP 桩逐协议断言请求关键字段、响应解析、usage 归一、错误分类。
4. **MCP 测试**（`--features mcp`）：进程内起 server，裸 JSON-RPC over HTTP 客户端 `initialize→tools/list（断言工具集与 read_only 注解）→tools/call` 驱动一局。
5. **E2E**：`scripts/e2e/agent-mock-llm.js`（HTTP 桩按剧本返回工具调用，带 CORS 头供 Web 用）+ `agent-builtin.js`（壳 CDP 与浏览器 5173 各跑：预置 llm-config→开始对局→断言 Agent 落子/日志/终局）+ `agent-mcp.js`（node 最小 MCP 客户端与人页打完整局）+ `agent-entry.js`（入口唯一性：菜单仅一个「Agent 对战」，设置卡驱动切换渲染正确）+ vitest 覆盖 `agentVfs.ts`。
   **真 LLM 实测（必过门槛，用户拍板）**：真 LLM 实测不可跳过——内置 Agent 必须用本地 Anthropic 协议网关**真实对局**（端点/key/模型存执行者本地记忆，绝不写入任何进 git 的文件；运行测试时由执行者读取注入）。E2E 脚本（playwright 驱动 UI）定性为**集成测试**；真 LLM 实测（Agent 循环与真实模型打完整局：落子/聊天/悔棋批复/终局）是更高一层的验收，**两者都必须跑**。无头单测仍用 Mock（确定性断言剧本）。
6. **回归**：每阶段 `cargo test --workspace` 绿才 commit；UI 阶段后跑既有 run.js 基线；末段 `build-wasm.sh`（goptop-transport/net/core 不动则 wasm 零变化）+ `build-ohos.sh` 冒烟。

## 分阶段切分（每阶段独立 commit，测试绿才 commit）

| 阶段 | 内容 | 主要文件 | 验证 |
|---|---|---|---|
| ① 文件系统+会话对+无头循环 | crate 骨架、PlayerHandle/EmitWatch/HookHost、pair（双向）、vfs.rs（动态合成+in/ 控制文件）、文件族工具（read/write/edit/grep/send/wait_events/game_*）、VfsStore trait+native 后端、Mock 循环（事件自动推送+纯文字轮不终止） | `crates/goptop-agent/**`；根 Cargo.toml members+rusqlite | `cargo test -p goptop-agent` |
| ② 三协议+compact+提示词 | 三适配器+HttpChannel trait+错误分类+usage、compact、prompt | `src/llm/{anthropic,openai_chat,openai_responses}.rs`、`src/compact.rs`、`src/prompt.rs`、tests/{llm_protocol,compact}.rs；Cargo.toml 加 reqwest | `cargo test -p goptop-agent` |
| ③ 接壳+唯一入口 UI+E2E | AgentHub+命令+拦截面、AgentPage（设置卡双驱动切换/对局视图/状态卡）、路由菜单、session.ts onChange 注入 | `src-tauri/src/agent.rs`(新)+lib.rs+session.rs；`frontend/src/{net/session.ts,net/links.ts,App.tsx,MenuPage.tsx}`、`pages/AgentPage.tsx`(新)；e2e 新脚本 | 全仓 cargo test + agent-builtin.js(壳) + agent-entry.js + 壳手动冒烟（真 key 打 gomoku/go 各一、执黑执白各一） |
| ④ MCP 出口 | rmcp handler+HTTP/Bearer/生命周期+game_start/leave 接线、MCP 连接卡、MCP e2e | `src/mcp.rs`、tests/mcp_server.rs；`agent.rs`、AgentPage.tsx；Cargo.toml workspace rmcp/axum+feature | `cargo test -p goptop-agent --features mcp` + agent-mcp.js + Claude Code `.mcp.json` 冒烟 |
| ⑤ Web 内置模式 | VfsStore web 后端（agentVfs.ts+钩子）、HttpChannel web 通道、循环 wasm 驱动（await 走 wasm-bindgen-futures，不用 tokio time）、Web e2e | `src/memory.rs` web cfg、`src/llm/` web cfg、循环驱动抽象、`frontend/src/net/agentVfs.ts`(新) | agent-builtin.js(浏览器) + vitest(agentVfs) |
| ⑥ 文档+产物确认 | 使用指南/已知限制（CORS 注记、平台矩阵）、.agents/memory 入账 | docs/（以实际文件名为准） | build-wasm/ohos 冒烟 + 既有 e2e 全量 |

依赖：①→②→③→④→⑤→⑥（④ 只依赖 ①③，可提前并行）。实施遵守 CLAUDE.md 工作流：并行 agent 互斥文件范围，各自 edit→test→commit。

## 风险

- **R1 key 明文**（store.json 无加密）：如实注明；后续可选 keyring/DPAPI。
- **R2 成本失控**：176k 上限+压缩、maxOutputTokens、maxLlmCalls、UI token 计数、停止按钮；MCP 模式成本在外部 Agent 侧。
- **R3 rmcp 3.2 feature 拼写未验证**：实施首日 `cargo add` 验证，备选按迁移指南调整。
- **R4 MCP 客户端单工具超时**：wait_events 阻塞参数≤120s 可续等；文档建议客户端调大超时。
- **R5 BC hub 全局串扰**：单局互斥+拦截面（根治=per-game topic，独立任务）。
- **R6 配对偶发超时**：40s 上限+失败清理+UI 重试；B 的 stun 强制空。
- **R7 平台矩阵（条件编译）**：桌面=全部；Web=内置模式（CORS 受限需注记）、MCP 代码不编译不渲染；安卓/鸿蒙=MCP feature 不开（不编译）、内置模式后续评估。wasm 产物不受影响（goptop-transport/net/core 不动）。
- **R8 176k 超模型真实窗口**：超窗兜底（强制压缩重试）。
- **R9 Web 循环驱动为新增移植面**：await 点全部走 wasm-bindgen-futures 钩子（禁 tokio time）；⑤阶段若证实代价失衡，向用户呈报后再裁剪。

## 验证（整体完成定义）

cargo workspace 全绿 + vitest/tsc 干净 + 新增四类测试全过（无头/compact/协议/MCP）+ e2e 全绿（内置壳+内置 Web+MCP+入口）+ 壳里人工各打一局（内置真 LLM、Claude Code 连 `.mcp.json` 真打；执黑执白各验一次）+ 截图目检 AgentPage 全部新 UI 状态（设置卡双驱动/等待接入/对局中工具日志/终局）+ wasm/ohos 产物确认零变化或同步。
