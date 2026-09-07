# 2026-09-07 · 全量代码审查报告（主代理串行执行）

> 审查人：主代理（用户指示 API 不稳，禁子代理/Workflow，单代理串行）。
> 审查立场：**假设既有代码全错**，逐文件对照注释与实现，不信任文档宣称。
> 审查范围：全部源码（Rust 4 crate 文件组 + src-tauri + frontend 全部 TS/TSX/CSS），共 23 文件 ~4100 行。
> 本报告**只审查不修复**；修复由原窗口按 TODO 顺序执行，每项修完须回填「修复记录」。

## 0. 结论摘要

编译/测试全绿（cargo test 26 通过、tsc --noEmit 零错、vite build 成功），**但绿色不代表正确**——已确认 8 个真实逻辑 bug（A 级）、4 处架构断层（B 级）、8 处注释失实/缺失（C 级）、若干健壮性问题（D 级）。整体判断：

- **前端（App.tsx + transport.ts）是唯一活着的生产路径**，质量问题集中于此：P2P 流程存在多条「失败后不可恢复」的用户旅程断裂；协议设计缺「执子颜色」字段，导致一系列靠猜测推断的补丁逻辑。
- **整个 Rust workspace 目前与前端零耦合**：前端无任何 `wasm`/`invoke` 调用（grep 证实）；protocol.rs 的 GameMsg 与前端线格式是**两套不同的协议**；transport crate 无调用者。Rust 侧规则实现本身质量尚可（单测覆盖边界），但它的注释与协议文档严重失实。
- **注释问题达到用户预判的程度**：至少 8 处注释与实现不符或超前于实现（跨设备观战、WASM 消除重复、游戏店真源宣称等），违反 `.agents/DOCS.md`「文档不许超前于实现」的红线。

## 1. 验证证据

| 检查 | 结果 |
|---|---|
| `cargo test --workspace` | 26 passed, 0 failed（goptop-core 规则单测全绿） |
| `npx tsc --noEmit`（frontend） | 0 错误 |
| `npm run build`（frontend） | 成功，254.74 kB JS |
| 前端测试 | **不存在**（package.json 无 test script）——P2P 流程全靠人工，本次审查发现的 bug 无一被测试捕获 |
| goptop-transport 测试 | 0 个测试（crate 本身零调用者） |

## 2. A 级：真实逻辑 bug（必须修）

### A1. `acceptAnswer` 幂等位提前置位 → 回执永久失效
- 位置：`frontend/src/net/transport.ts:817-823`
- 现状：`acceptAnswer()` 在 `decodeRtcPayload` **之前**就 `this.answered = true`。若房主粘贴了损坏/不匹配的回执 token（解码抛错），`answered` 已被锁死；此后任何正确的 answer 都被 `if (this.answered) return` **静默吞掉**（不抛错）。
- 叠加效应：`finishHostRtc`（App.tsx:737-747）catch 后只提示「检查设置页线路」，误导用户去改 STUN。而 `answered` 注释宣称「回执可能同时经弹窗与 Presence 两条路径到达：answer 只允许应用一次」——该容错承诺在解码失败场景下完全失效。
- 修复方向：先 decode 成功、`setRemoteDescription` 成功后再置 `answered = true`；或 catch 时回滚。

### A2. Move 协议缺执子颜色 → Resign/Pass 判定靠猜，观战者必错
- 位置：`frontend/src/App.tsx:481-485`；根因 `transport.ts:33-48`（Move 消息无颜色字段）
- 现状：收到 `Resign` 时 `setWinner(myColorRef.current)`——即「谁收到谁赢」。对局者视角碰巧对（对手认输），但：
  - **观战者** `myColor` 恒为 `"white"`（App.tsx:911）→ 无论黑方还是白方认输，观战者一律判「白胜」。
  - `Pass` 直接翻转本地 `toMove`，不校验翻转方是否真是行棋方。
  - 未来接入「认输/停一手」按钮（协议已预留）时，自己发出的 Resign 若回流同样会错。
- 根因：GameMsg.Move 不带 `by: "black"|"white"`，接收端全靠本地 toMove/myColor 推断。**建议协议层加颜色字段**（同一改动覆盖 Resign/Pass/Place 的同步校验），这属于根因修复，符合 memory「对准根因最小改动」。

### A3. SyncState「后到覆盖先到」可吞掉已落之子
- 位置：`frontend/src/App.tsx:438-448`（无条件全量覆盖）+ `App.tsx:989-996`（RTC open 后 200ms 双方各发一次）
- 场景：直连 `open` 即 `setPeerConnected(true)`（App.tsx:984），本方立即可落子；若在对方 SyncState 到达前（200ms + 网络延迟窗口）落子，随后到达的对方空盘 SyncState 会**把这一手抹掉**，本地 history/lastMove 同步失真。
- 文档宣称「初局均为空板，无实际冲突」（.agents/docs/p2p-protocol.md §4.3）**不成立**。
- 修复方向：SyncState 带单调同步序号（或以 history 长度做版本），落子后产生的本地状态不被更旧的全量快照覆盖。

### A4. 粘贴损坏回执后整局不可恢复
- 位置链：`App.tsx:752-792`（hostAcceptReceipt）+ `App.tsx:737-747`（finishHostRtc）+ A1
- 时序：房主粘贴错误回执 → `finishHostRtc` 异步失败（仅 setNotice）→ 但 `hostAcceptReceipt` 早已同步完成 `phase="playing"`、`pwd=null`、弹窗已关。「输入回执」按钮只在 host+waiting 页存在（App.tsx:1258）→ **重试入口永久消失**，pwd 失效，唯一出路整局作废回主页。
- 另一半：offer 生成失败时 `hostRtcRef.current = null`（App.tsx:678-682），此后粘贴任何回执，`finishHostRtc` 第一行 `if (!peer || peer.state !== "waiting-guest") return;` **静默返回不报错**（App.tsx:739-740）→ 房主显示「回执已受理，直连建立中…」假成功进对局，客人永远等不到 open。
- 修复方向：finishHostRtc 失败时回滚 phase/pwd 并保留弹窗（或提供重试入口）；peer 缺失时向弹窗返回错误而非静默。

### A5. hostCreate 竞态：offer 未生成前的邀请链接缺 `rtc` 参数
- 位置：`App.tsx:664-675`
- 现状：`setInviteUrl(inviteToUrl(...))`（无 rtc）先执行，offer 异步生成后再覆盖。期间「复制邀请链接」按钮**未禁用**；复制到无 rtc 版本的链接发给跨设备客人 → `guestChallenge` 走无 hostOffer 分支（App.tsx:731）→ 只发 presence.challenge（跨设备不可达）→ 客人**无回执弹窗**（弹窗逻辑在 hostOffer 分支内）→ 双方死等。
- 修复方向：offer 未就绪时禁用复制按钮或复制时提示；或把「无 rtc 邀请」明确标记为仅同源。

### A6. 客人误关回执弹窗后无法找回
- 位置：`App.tsx:1536-1540`（点遮罩 `setModal(null)`）、`App.tsx:1546-1566`（receipt 弹窗）
- 现状：`answerBackUrl` 留在 state，但 guest waiting 页 UI 只有「取消」按钮，**没有任何按钮能重新打开 receipt 弹窗**。误触一次遮罩 = 丢失回执 = 只能重开对局。
- 关联拍板：TODO「回执按钮常驻」——此项修复时应一并设计「重新生成/重新查看回执」入口。

### A7. waiting 中可粘贴新邀请 → 旧局资源泄漏
- 位置：`App.tsx:806-808`（允许 `home || waiting` 两态粘贴邀请）+ `guestChallenge`（App.tsx:687-734，不清 `rtcPeersRef`/`hostRtcRef`）
- 场景：房主正在等待，粘贴他人邀请 → 直接变 guest 挑战别人；原等待局的 RTC peer、hostRtc、pwd 全部泄漏（只有 backHome 清理）。角色/PWD/gameId 被覆盖但底层连接未关。
- 修复方向：guestChallenge/hostCreate 入口统一先执行清理（等价于 backHome 的连接清理部分）。

### A8. 挑战 waiting 中的人被静默忽略
- 位置：`App.tsx:545-555`（drain effect：waiting 且 pwd 不匹配 → 直接忽略，注释写明「第三人拿旧 pwd 无法加入」）
- 场景：A 开局等待，B 从在线列表点「挑战」（无 pwd）→ challenge 事件被静默吞 → B 永远停在「已发送挑战，等待对方同意…」，无超时、无拒绝通知。
- 修复方向：被忽略时回 reject（B 侧已有 reject 处理），或 B 侧挑战加超时提示。

## 3. B 级：架构断层（注释宣称 vs 实际实现）

### B1. 前后端是两套互不相识的协议
- `crates/goptop-core/src/protocol.rs:12-36`：`GameMsg { seq, kind }`，MsgKind = `Hello{kind}` | Move | UndoReq | UndoAck | Chat | Ping | Pong。
- `frontend/src/net/transport.ts:38-56`：`GameMsg { seq, sender, userId, kind }`，MsgKind 多出 `SyncState/SyncRequest/Reset`，Hello 带 `size/name`，SyncState.history 是 `Coord[]` 而 Rust 是 `Vec<Move>`（含 Pass/Resign）。
- protocol.rs 头注释自称「联机消息 GameMsg（serde）」的唯一真源——**实际前端从未使用它**。任何以「Rust 是协议真源」为前提的改动都会踩空。

### B2. iroh 路线注释全面残留
- `crates/goptop-transport/src/lib.rs:1-8`：「iroh 官方公有 relay」「Web 经 relay WS」——实际 P2P 全在 TS WebRTC(STUN)/BroadcastChannel，**transport crate 无任何调用者，整个 crate 等价死代码**（grep 证实：仅 self 引用）。
- `crates/goptop-transport/Cargo.toml:2-3,17,20-22`：同上，`iroh` feature 是空占位。
- `src-tauri/src/p2p.rs:1-4`：「Phase 3 将在此持有 iroh::Endpoint」——方向已弃，占位注释指向不存在的未来。
- `crates/goptop-core/src/lib.rs:8`：「外层由 iroh 端到端加密」——实际是 WebRTC DTLS。

### B3. 前端从未调用 Rust 核心，但注释宣称「避免重复实现」
- `crates/goptop-core/src/wasm.rs:3-4`：「避免在 TS 侧重复实现围棋/五子棋规则」——**事实相反**：TS 侧 `checkFive` 存在三份（`App.tsx:30-47` 内联生效版、`state/gameStore.ts:18-35` 死代码、间接重复的 `game/` 目录），前端无任何 `wasm`/`invoke` 调用（grep 证实）。
- `src-tauri/src/commands.rs`：`greet` 探针前端零调用。

### B4. gameStore.ts 头注释双重失实
- `frontend/src/state/gameStore.ts:1-8`：「前端唯一真源的 JS 侧镜像」——App 实际用的是内联副本，此文件死代码（`.agents/docs/architecture.md` 已承认但代码未清）。
- 更危险：`tryPlace`（:70-98）对 `kind==="go"` **直接落子成功，无提子/自杀/气检查**，注释完全未声明围棋未实现——将来有人把 App 迁到该 store（注释正是这么怂恿的），围棋会静默产生错误规则。

## 4. C 级：注释审查（用户重点：注释与代码一致性）

| # | 位置 | 问题 | 类型 |
|---|---|---|---|
| C1 | `transport.ts:12-13` | 「跨设备观战走与主机之间的独立 WebRTC 直连」——未实现（TODO.md 已认领：观战者跨设备打开 `/watch/<id>` 永远空棋盘；`DirectRtcPeer.role="spectator"` 类型备好、流程未接） | 注释超前于实现 |
| C2 | `transport.ts:603-606` | 「现代内核原生支持，含 Tauri WebView2」——`CompressionStream("deflate-raw")` 在旧 Safari（<16.4 支持不全）直接 TypeError，`acceptOffer` 无降级无 try/catch，失败提示「检查线路」误导。承诺的兼容性无任何测试背书 | 注释承诺未经证实 |
| C3 | `BoardSvg.tsx:3` | 「选中橙色高亮」——实现是 **hover 悬停**高亮（`onHover`），组件无「选中」概念 | 用词失实 |
| C4 | `game.rs:183-193` + `go.rs:61-65` | 上层用 `e.contains("suicide")`/`"occupied"`/`"bounds"` **字符串匹配**给错误分类——go.rs 返回 `Result<_, String>`。错误文本一改，分类静默断裂。应改枚举 | 脆弱设计+注释未提示耦合 |
| C5 | `protocol.rs:47-52` | 「Phase 3 由 iroh::NodeAddr 编码」占位——路线已改 WebRTC 链接 | 指向废弃未来 |
| C6 | `App.tsx:461-486` | Move 分支用 `toMoveRef.current` 推断来方颜色（协议缺陷补丁）——**无任何注释解释为什么这么做、何时会错**（恰是 A2 的雷区） | 关键逻辑缺 WHY |
| C7 | `game/board.ts:1-5`、`game/rules.ts:1-3` | 自称「供非 React 场景复用」「便于未来接入 WASM」——零引用，描述的用途不存在 | 注释虚构用途 |
| C8 | `App.tsx:1588`、`App.tsx:1258` | 弹窗按钮「受理回执并开局」违反用户拍板（回执不一定是开局）；「输入回执」按钮仅 host waiting 页有——TODO 三条硬性要求均未落实 | 拍板未落地 |

注释密度总体评价：App.tsx / transport.ts 的**流程性注释质量高于平均**（信件队列、StrictMode 双挂载、双链路去重等 WHY 说明到位，这部分值得保留）；失实集中在「跨层宣称」（协议真源、WASM 消重、跨设备观战）——恰是最误导后续 agent 的位置。

## 5. D 级：质量与健壮性（择要）

- D1. **App.tsx 1602 行单文件**：路由+8 页面+弹窗+全流程+消息处理内联。文档已列，建议修复轮按「页面组件 / 对局状态机 / 信令编排」三刀拆分（与 A4/A6 修复同批做，避免二次碰同一区域）。
- D2. **死代码四件套**：`gameStore.ts` 聚合、`game/board.ts`、`game/rules.ts`、`components/Stone.tsx`、`components/BrutalCard.tsx`——零引用。按 CLAUDE.md「未用即删」，先与 B4 的注释失实一起清理或接回。
- D3. **`genPwd` 用 `Math.random`**（transport.ts:89-91）：pwd 是唯一准入钥匙+信令 XOR 密钥，非 CSPRNG；且 `Math.random().toString(36).slice(2,8)` 长度可能不足 6 位。改 `crypto.getRandomValues`（一行改动）。
- D4. **`parsePastedLink` 过度宽容**（transport.ts:289-323）：任意可解析文本（裸单词、`ip:port`）补 `https://` 强解，单段路径一律判「用户主页」——粘普通文本会误触发挑战弹窗。建议要求至少命中「已知域名或含 pwd/rtc 参数」。
- D5. **`disconnected` 即报 error**（transport.ts:738）：WebRTC `disconnected` 常可自愈，直接置 error 会提前撕掉「已直连」状态。应仅 `failed` 判死。
- D6. **presence `beforeunload` 监听器累积**（transport.ts:404-406）：stop→start 循环重复注册（闭包引用旧 bc，被 catch 兜住，但泄漏监听器）。
- D7. **`rtcPeersRef` 只 push 不移除**（App.tsx:999）：close 过的 peer 留数组，跨局增长（backHome 才清）。
- D8. **notice setTimeout 竞态**：连续 `setNotice` 时旧 timer 提前清掉新消息（App.tsx 多处同模式）。
- D9. **观战者 presence 显示「对局中」**：`joinAsSpectator` 设 `phase="playing"`（App.tsx:908）→ 状态上报 in-game。语义失真。
- D10. **核心层不校验 GameKind 尺寸不变量**（game.rs:93-103）：`GameKind::Gomoku{size:9}` 造出 B19 物理盘+逻辑 9 的混搭；前端 URL `kind=gomoku&size=9` 也可通过。核心构造函数应拒绝非法组合（边界校验属系统边界）。
- D11. **前端零测试**：P2P 信令/去重/同步这类状态机逻辑无任何单测；Rust transport crate 亦 0 测试。建议至少为 GameChannel 去重、parsePastedLink/parsePastedAnswer、keyStream 往返补 vitest。

## 6. 安全评估（无高危，记录在案）

- pwd 明文出现在邀请/回执 URL——转发链路可见。**符合邀请链接产品语义**（本来就是发给对方），但需知悉「任何转发者可加入」。pwd 6 位熵低（D3 加重），爆破不可行性依赖「每局轮换+短时效」。
- 信令 XOR（G1）注释已明确自认「混淆级非密码学级」，真实安全靠 DTLS——该段注释与实现一致，**合格**。
- 无 TURN 中转红线、无服务器依赖——与 memory 拍板一致，实现符合。
- React 渲染层无 XSS 面（动态文本均经转义）；`fromName` 未限长，可被滥用撑爆 UI（小）。

## 7. 建议修复顺序（供原窗口执行，非本报告职责）

1. **协议根因**：Move/Hello 加 `by` 颜色字段（A2），顺手在 Rust protocol.rs 与前端之间二选一：要么删 Rust protocol.rs 的假协议，要么让前端类型注明「TS 侧才是现行协议」——消除 B1。
2. **transport.ts 四连修**：A1（answered 顺序）、D3（genPwd）、D5（disconnected）、C1（删超前注释或标记未实现）。
3. **回执旅程闭环**：A4 + A6 + A5 + C8（含 TODO 三条硬性要求）。
4. **状态清理**：A7 + D7 + A8。
5. **A3 同步覆盖守卫**（需要设计 SyncState 版本号，改动面较大，单独一轮）。
6. **清理批**：D2 死代码 + B2/B3/B4 失实注释 + C3/C7 + D10。
7. **测试批**：D11 前端 vitest + transport crate 最小测试。

> 每步修完遵守闭环规范：`cargo check`/`tsc --noEmit` + 浏览器验证（Playwright）→ commit。修复完成后在本文件「修复记录」区回填。

## 修复记录

**2026-09-07 修复轮（原窗口主代理执行）**——覆盖 A1-A8、C1-C3/C5-C8、D2-D5/D7/D9/D10、B1-B4；遗留项见 .agents/TODO.md「审查遗留」。

- **A1** `acceptAnswer` 幂等位移到 setRemoteDescription 成功后置位；catch 内双路径竞态容错（peer.answered / remoteDescription 已设则视为成功）。
- **A2** `MsgKind.Move` 增加 `by: StoneColor`（协议变更，发送端 handlePlace 带 by；接收端 Place/Pass 按 by 校验行棋方、Resign 按 by 判胜者，注释说明"不得从本地推断"）；无 by 的旧格式消息直接丢弃。
- **A3 未修**（报告建议单独一轮），列入 TODO。
- **A4** `finishHostRtc` 改为返回错误文案；`hostAcceptReceipt` 改 async：先 await 受理成功、再推进 phase/pwd/join；peer 缺失显式报错。浏览器验证：坏 token → 弹窗内「回执无法解码」+ 仍 waiting + 弹窗保留；同弹窗贴正确回执 → 进对局且直连建立。
- **A5** hostCreate 不再先发无 rtc 链接；offer 生成成功才出现邀请链接；失败退化为无 rtc 链接并如实提示「跨设备不可用」。
- **A6** 客人等待页新增「查看回执」按钮（answerBackUrl 非空即可重开回执弹窗）。
- **A7** 抽 `closeAllRtcPeers()`（关 rtcPeers/hostRtc/modal 状态），hostCreate/guestChallenge/joinAsSpectator/backHome 入口统一调用。
- **A8** drain effect：对局中、等待中钥匙不符、guest 等待中收到挑战——一律 `presence.reject` 回拒绝信。
- **C1** transport 头注释观战行改为「跨设备观战尚未实现」。
- **C2** CompressionStream 兼容面写实（Chromium 103+/FF 113+/Safari 16.4+，无降级）；错误文案不再误导为线路问题。
- **C3** BoardSvg「选中」→「悬停」。
- **C5** protocol.rs RoomTicket 注释去除 iroh Phase 3。
- **C6** Move 分支 WHY 注释随 A2 重写补齐。
- **C7** game/board.ts、game/rules.ts、components/Stone.tsx 删除（零引用，git rm）。
- **C8** 弹窗按钮「受理回执并开局」→「确认回执」；header 常驻「输入回执」（用户拍板三条硬性要求之二落地，自动识别=协议+分派就位）。
- **B1** protocol.rs/lib.rs 头注释明确：现行协议真源是 `frontend/src/net/transport.ts`，本模块为未接线参考实现。
- **B2** goptop-transport lib.rs/Cargo.toml 去 iroh 路线残留，标明未接线；src-tauri/p2p.rs 同步。
- **B3** wasm.rs/commands.rs 注释改实况（前端未调用 WASM/invoke）。
- **B4** state/gameStore.ts 瘦身为 checkFive 单函数+实况注释（tryPlace/GameState 等零引用且围棋规则残缺，删除）；BrutalCard 注明暂无调用者。
- **D2** 见 C7/B4。
- **D3** genPwd 改 crypto.getRandomValues，固定 6 位 base36（36^6 取模）。
- **D4** parsePastedLink 单段路径收紧：须 u- 前缀或带 pwd/rtc 参数，否则视为普通文本返回 null。浏览器验证裸中文文本被拒。
- **D5** connectionState 仅 failed 判死，disconnected 留观（可自愈）。
- **D7** attachPeer onState closed/error 时从 rtcPeersRef 出列。
- **D9** presence 上报：观战者显示 idle（不占对战席位）。
- **D10** GameState::new 校验尺寸不变量（Gomoku=15，Go∈{9,13,19}，非法 panic）；cargo test 26 全绿。
- **D1/D6/D8/D11/C4 未修**，列入 TODO（拆分/监听器累积/notice 竞态/测试/错误枚举）。

验证：`npx tsc --noEmit` 0 错；`npm run build` 成功；`cargo test --workspace` 26 passed；浏览器回归（Playwright 双窗口）：邀请粘贴→直连→落子同步（手数一致）→ by 协议注入测试（旧格式丢弃、Resign 判色正确）→ 坏回执重试闭环 → 裸文本拒识。

## 修复记录（第二轮，2026-09-07 同日续）

- **A3** SyncState 接收端以 history.length 作快照版本守卫（commit be7fac7）；浏览器验证：注入过时空板快照手数不变、正常落子双端同步。
- **D1** App.tsx 三刀拆分完成（commit 60fc02e + 99fbb4c）：1698→544 行；game/board.ts 规则唯一实现、pages/components.tsx、pages/LocalPage.tsx、state/useGameSession.tsx。
- **D6** presence beforeunload 处理器改实例字段（commit 0bf204c）。
- **D8** showNotice 单入口接管全部 setNotice 调用点（commit 0bf204c）。
- **D11** vitest 30 例落地（commit be7fac7）：去重/编码往返/解析/genPwd；顺修 deflate/inflate writer promise 未 catch 的 unhandled rejection。
- **C4** go::try_place 返回 RuleError 枚举，删除字符串匹配分类（commit 0d24ea0）。
- **术语轮（用户新拍板，非审查项）**：P2P 对等无主客，host/guest 全套改 inviter/invitee（commit 0bf204c）；文档树同步。
- 验证链：tsc 0 错、vite build 成功、vitest 30/30、cargo test 26 通过、Playwright 双窗口邀请→直连→落子同步（拆分前后各一轮）。
