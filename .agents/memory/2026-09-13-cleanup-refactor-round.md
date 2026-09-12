# 2026-09-13 · 整理轮：三层拆分 + 规则下沉 wasm + B1-B4

## 背景与拍板

- 用户重申架构总要求：**Rust 承接一切功能，Tauri 为主，TS 只负责 UI；核心功能全部经 Rust 编译 wasm 供 Web 使用；Tauri 正常编译 Rust。** 审查结论：原状不满足（规则 TS 重复实现、wasm 未接线、src-tauri 空壳、传输全 TS）。
- 用户解除 2026-09-07 的多代理禁令（「允许多代理」）；并发修改代理仍须文件作用域互斥，git 提交不并行。

## 本轮落地（commits 58aea04→95f25f2）

1. **net 层拆分**（7aebe95）：transport.ts 1064 行 → protocol/identity/stun/servers/links/presence/gameChannel/rtc 八模块；死代码删除（短码链接、Ctl 消息、trickle ICE 死链路——对端从无 "ice" case 且 waitGathering 覆盖 onicecandidate，功能从未生效）；类型统一到 net/protocol。
2. **state 层拆分**（7644938）：useGameSession 1786 → 1208 编排层 + serverSignaling/negotiation/chat 三域工厂（Pick<SessionCtx>，函数体逐字搬移，闭包语义不变）。
3. **B1-B4 修复 + UI 修复**（ea97dfd），根因：
   - **B1 观战者看不到悔棋/重开回退**：SyncState 守卫只比 history.length，回退的更短快照被当旧快照丢弃 → 加 `sv` 回退纪元，(sv, length) 双键比较；E2E 9h-9j 回归。
   - **B2 对手识别**：relayTargets 插入序首元素当对手，观战者先于对手加入（等待期可分享 spec 链接）时房间同步/聊天发给观战者 → `opponentRef`。
   - **B3 红灯误亮**：closeAllRtcPeers 在 phase 仍 playing 时触发 close 回调误置 connLost；观战者离开也亮红灯 → 显式复位 + 只认 role="player"。
   - **B4 跨局残留**：specCanChat/specChatOk/specRequestDenied 不随换局复位 → closeAllRtcPeers 统一清。
   - **对局内反馈消失**：1d4497f 卡片瘦身把 notice 显示面一并删了（E2E 10 断言同时暴露）→ 恢复条件渲染单行。**教训：删 UI 行前先 grep E2E/文案依赖。**
4. **UI 拆分**（6bd2a58）：App 765 → 243 壳；pages/{Menu,P2p,Users,Settings,User,Watch}Page + components/{StatusLamp,UrlRow,NoticeLine,KindSizePicker,InviteModal,PasteModal,ConfirmBanner}；页面收整个 `GameSession` 类型（ReturnType），不逐 props 穿透。
5. **规则下沉 wasm**（95f25f2）：wasm.rs 重写为 `WasmGame`（try_place/undo_last/adopt/reset，JSON 数据面与 protocol.ts 同构）；`game/rules.ts` TLA 门面；落子/悔棋/SyncState 采纳全走 Rust；checkFive 副本删除。围棋提子/禁自杀在 Web 首次生效（浏览器实测 7 断言 + 截图）。
6. **文档收口**：architecture.md/p2p-protocol.md 重写；README/已知限制与路线图对齐实况；goptop-transport crate 删除（零调用者 + iroh 注释误导）。

## 关键坑（新 agent 必读）

- **pickKind 中间帧 wasm trap**：kind/size 分两次 setState 时存在 (Go,15) 中间渲染帧，Rust `GameState::new` 尺寸断言 panic = wasm `unreachable`，直接掀翻 React 树白屏。修法：pickKind 原子 setKind+setSize + rules.ts 边界守卫。**Rust 断言跨 wasm 边界就是崩溃，不是异常。**
- **wasm-bindgen 构造函数不能返回 Option** → 用静态工厂 `new_game`；Option 映射为 undefined。
- **E2E 断言依赖 UI 文案面**：断言文本必须真的有渲染载体（notice/p2pStatusText 被删 = 断言必挂，且是真实 UX 回归的信号）。
- wasm 产物（frontend/src/wasm/）**入库提交**——前端构建/CF Pages 无 Rust 工具链；改 core 后跑 `scripts/build-wash.sh`（见 scripts/build-wasm.sh）重生成。wasm-bindgen-cli 版本必须与 Cargo.lock 锁定版本一致（本轮 0.2.127）。
- 失败的 Agent 派发（"Model request failed"）可能留下半成品文件——下次派发要在 brief 里声明并要求核对。

## 验证基线（本轮结束时）

cargo test 26 过（零警告）；tsc 零错；vitest 30 过；build 绿（wasm 154KB/gzip 65KB）；E2E 42/42；围棋提子/悔棋还原/五连浏览器实测 7/7；截图 shots/phase4 与 shots/phase5。

## 关联

- [[2026-09-07-no-parallel-agents-single-serial]]（禁令已解除）
- [[2026-09-13-lobby-challenge-modal]]、[[2026-09-12-userid-link-negotiation-chat-spectator]]
