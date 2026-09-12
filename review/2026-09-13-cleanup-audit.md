# 2026-09-13 · 全量整理审查报告（整理轮开工前审计）

> 范围：frontend/src 全部 13 文件（~5000 行）、crates 全部（~1530 行）、src-tauri、docs/ 与 .agents/docs/ 全部文档、README。
> 方法：逐文件全文精读（非抽样），基线验证 cargo test（core 26 过）/ tsc 零错 / vitest 30 过。
> 结论先行：**架构合规审查不通过**（规则与传输全部在 TS，Rust 未接线）；存在 4 个确认级 bug、
> 6 处死代码、两份 agent 文档与 README/已知限制大面积失真。修复方案见 §6，执行计划见 .agents/plan/。

---

## 1. 架构合规审查（用户要求原文逐条核对）

要求：**Rust 承接一切功能，tauri 为主，ts 只负责 UI；web 只是额外兼容；核心功能全部通过 rust 编译 wasm 供 web 使用；tauri 则正常编译 rust。**

| # | 要求 | 现状 | 判定 |
|---|---|---|---|
| 1 | 核心规则 Rust 实现 | `goptop-core` 完整（棋盘/五连/提子/自杀/GameState，26 单测） | Rust 侧 ✅ |
| 2 | 规则经 wasm 供 web 使用 | `wasm.rs` 存在但**从未接线**（自己头注释承认）；前端规则是 TS 重复实现 `game/board.ts`（仅 checkFive/emptyBoard） | ❌ |
| 3 | 围棋功能可用 | **TS 前端完全没有提子/自杀判定**（handlePlace/Move 处理/LocalPage 三处都只是置子）；Rust 有但前端用不上。规则卡却向用户承诺「无气的棋子被提掉；禁自杀」——UI 承诺与实现相反 | ❌ 功能级缺陷 |
| 4 | Tauri 为主、原生走 Rust | src-tauri 只有 43 行（greet 探针 + 空 p2p 模块）；Cargo.toml 依赖 goptop-core/goptop-transport 但**代码零使用**（未用依赖）；桌面端与 Web 走同一份 TS 逻辑 | ❌（壳是真壳） |
| 5 | 传输层 Rust | `goptop-transport` 零调用者（未接线），且其注释仍指向已废弃的 iroh/Phase 3 路线；现行传输全在 TS（transport.ts 1064 行） | ❌ |
| 6 | 协议 Rust 参考实现 | `protocol.rs` 未接线且与 TS 线格式不一致（自己声明「唯一真源是 transport.ts」） | ❌（注释诚实） |

**结论：不满足。** 差距是结构性的：除 UI 外一切核心逻辑（规则执行、信令、建连、状态机、编解码）都在 TS。
本轮落地 **第一阶段合规格：规则下沉 wasm**（§6 Phase 5）——web 与 Tauri webview 统一由 Rust-wasm
执行落子规则（含围棋提子/自杀，顺带修复 #3 功能缺陷）。传输层/状态机 Rust 化工程量大、风险高，
列为路线图后续阶段，不在本轮冒进。

## 2. 上帝结构

| 文件 | 行数 | 问题 | 处置 |
|---|---|---|---|
| `state/useGameSession.tsx` | 1786 | 六个域混在单 hook：棋盘状态、presence 挑战握手、服务器信令（含观战房间全套）、协商、聊天/头像、URL 意图；60+ state/ref 对 | 按域拆 `state/` 子模块（行为保持式搬移），棋盘操作抽纯函数与 LocalPage 共用 |
| `net/transport.ts` | 1064 | 九个域混一文件：协议类型/身份/STUN 配置/URL 模型/分享基地址/服务器配置/Presence/GameChannel/G1 编解码+RTC | 拆 `net/` 八文件，`transport.ts` 删除（不留转发壳） |
| `App.tsx` | 754 | 六个页面 JSX 全部内联；指示灯行/URL 行/notice 行/棋种尺寸选择器（展开态+弹窗态两份）重复 | 页面拆 `pages/*.tsx`，重复 JSX 抽共用组件 |
| `pages/components.tsx` | 480 | Avatar+裁剪+设置、ChatPanel、PeerList、两套设置面板、BoardPanel、loadDefaults 全在一起 | 按组件拆文件；loadDefaults 归 state 域 |

## 3. Bug 清单

### 确认级（本轮修复）
- **B1 观战者永远看不到悔棋/重开回退**：SyncState 覆盖守卫（A3）只比 `history.length`；玩家 undo/reset 后补发的全量快照 history 更短，被守卫丢弃 → 观战者棋盘永久发散，后续 Move 因占位被弃。修：SyncState 增加单调 `sv`（回退纪元），守卫改 (sv, length) 双键比较。
- **B2 服务器 relay 目标首元素≠对手**：`pushSpecSync`/`sendChat` 都用 `relayTargetsRef` 插入序第一个当「对方」。观战者在等待期先于对手加入（spec 链接等待期即可分享）时，首元素是观战者 → 对手永远收不到房间同步，玩家聊天会发给观战者。修：独立 `opponentRef`，relay 广播才用全集合。
- **B3 connLost 生命周期错**：(a) `closeAllRtcPeers` 在 backHome 内触发 peer close 回调时 phase 仍是 playing → setConnLost(true)，之后新等待卡亮红灯「已中断」直到新连接 open；(b) **观战者**断开也满足 wasOpen&&playing → 玩家对局卡误亮红灯。修：backHome/建局入口复位；closed 回调只对 role==="player" 的 peer 置位。
- **B4 协商/观战权限跨局残留**：`specCanChatRef/specChatOkRef/specRequestDeniedRef` 在 backHome/新局不复位——上局获准聊天/被拒申请的状态带进下一局。修：backHome 与建局入口统一复位。

### 顺手修（小、明确）
- 用户主页 `/<userId>` 的「挑战」按钮在服务器模式仍走 presence 路径（跨设备不可达）——TODO 已记录的 ~3 行修复，改按 serverMode 分派 `serverChallengePeer`。
- 服务器模式下 presence「peers」事件与服务器名册互相覆盖（两个写者轮流 setPeers）→ 服务器模式忽略 presence 名册事件。
- 观战聊天批准后 `specCanChat` UI 态与 ref 双写易漂移——随 B4 一并收口。

### 记录不修（有意为之/低危，写进已知限制）
- confirmReq 弹窗无队列： undo 请求未决时被 spec-chat 请求覆盖，前一请求方收不到回执（低频；队列化属新功能）。
- p2pStatusText/boardDisabled 的 useMemo 依赖 ref（relayTargets），存在一拍滞后（下次渲染自愈）。
- 围棋无劫争/数目判定（Rust 侧也标注 Phase 4）；五子棋无禁手（设计取舍）。

## 4. 死代码清单（本轮删除）

| 项 | 证据 |
|---|---|
| `joinCodeUrl`/`spectateCodeUrl`（短码链接） | src 全库零引用；userId 链接回归（09-12 拍板）后短码路线已弃 |
| `MsgKind` 的 `Ctl` 变体（mute/unmute/kick/chat-approve/chat-reject） | 零引用；观战房间控制已改服务器 signal（spec-*） |
| trickle ICE 全链路：`DirectRtcPeer.onCandidate`、`addRemoteCandidate`、`attachPeer` 的 onCandidate 转发 | 对端 `serverHandleSignal` 无 "ice" case，收到即静默丢弃；且 waitGathering 覆盖 pc.onicecandidate，本端候选从未外发——功能整体从未生效。offer/answer 均等全量 gathering 后才发，无需 trickle |
| `Presence.myName()` | 零引用 |
| `components/BrutalCard.tsx` | 零引用（注释自称「持续零引用应删除」——到期了） |
| `crates/goptop-transport` 整个 crate | 零调用者 + iroh 注释误导；git 历史可寻回。workspace 成员与 src-tauri 未用依赖一并清理 |
| src-tauri 未用依赖 | goptop-core/goptop-transport/thiserror/tokio 声明而未用（保留 serde/serde_json 亦未用——全部复核后删） |

## 5. 代码-注释-文档不符清单

| 处 | 失真 |
|---|---|
| `net/transport.ts` 头注释 | 「禁用中转」与服务器 relay 兜底矛盾；「跨设备观战尚未实现」已过时（spec 链接已实现） |
| `net/serverChannel.ts` 头注释 | 「offer/answer/ice」中的 ice 属死链路 |
| `.agents/DOCS.md` | ~~链接指向不存在的 docs/~~ **审计误报**：链接相对 `.agents/` 解析（→ .agents/docs/，文件存在）。仅内容描述过时，已随文档重写更新 |
| `.agents/docs/architecture.md` | 停在 09-07：行数全错；仍列 RtcStatusLine（已删）；模块地图缺 serverChannel/服务器模式/观战/聊天/头像/协商；布局段描述已被 --stack-max 机制取代的旧公式 |
| `.agents/docs/p2p-protocol.md` | 「Chat/UndoReq 未接线」「跨设备观战未实现」「STUN 三节点表」全过时；MsgKind 清单缺 Swap/Reset/Avatar/Ctl；缺服务器模式协议整章 |
| `README.md` | 「无服务器、无中转」宣言与内置服务器模式矛盾；`/<hostId>` 用废弃术语 host；STUN 三节点表过时；目录结构称 transport 有 iroh（已弃）；称 core 为「前端唯一真源」不实 |
| `docs/已知限制与路线图.md` | A1 跨设备观战未实现（已实现）、A4 悔棋仅本地（已实现）、D1 回执入口（已改）；缺服务器模式限制条目 |
| `crates/goptop-server/Cargo.toml` | description「短码邀请/观战」已弃 |
| `src-tauri/Cargo.toml` 头注释 | 「iroh Endpoint（P2P 真源）」已弃；依赖未用 |
| `crates/goptop-transport/src/lib.rs` | 指向已弃 iroh 路线（随 crate 删除一并消解） |
| `frontend/src/game/board.ts` 头注释 | 「未来规则下沉 WASM 的对齐点」——本轮落地后需改写为 wasm 绑定层说明 |

## 6. 重构方案（阶段与验证）

| Phase | 内容 | 文件边界 | 验证 |
|---|---|---|---|
| 1 | net 层拆分 + 死代码删除 + 头注释重写 | `frontend/src/net/**`（含全库 import 更新） | tsc + vitest + build |
| 2 | useGameSession 按域拆分（纯搬移，不改行为） | `frontend/src/state/**` | tsc + vitest + build |
| 3 | B1-B4 + 顺手修（协议加 sv 字段） | `frontend/src/net/protocol.ts`、`state/**`、`App.tsx`(users/主页按钮) | vitest 新增用例 + tsc |
| 4 | UI 拆分：pages/*.tsx + 共用组件；App 只剩壳 | `frontend/src/App.tsx`、`pages/**`、`components/**` | build + 浏览器截图（宽/窄） |
| 5 | **规则下沉 wasm**：goptop-core → wasm32 → 前端落子/悔棋/重开全部经 Rust 规则；围棋提子/自杀在 web 生效 | `crates/goptop-core/src/wasm.rs`、`frontend/src/game/**`、`state/*board*` | cargo test + wasm 构建 + vitest + E2E 39 + 围棋提子截图 |
| 6 | 文档收口：architecture/p2p-protocol 重写，README/已知限制/DOCS.md/TODO/memory 更新；goptop-transport 删除 | `docs/**`、`.agents/**`、`crates/goptop-transport`、workspace Cargo.toml | 链接核对 + cargo check |
| 7 | 全量回归 + 提交推送 | — | cargo test + tsc + vitest + build + E2E + 截图 |

排序理由：net 先行（所有人 import 它）；state 其次；bug 修复在搬移完成后（避免边搬边改）；UI 独立；
wasm 依赖 state 拆分后的落子路径；文档最后（不许超前于实现）。每 Phase 一提交，红不落。
