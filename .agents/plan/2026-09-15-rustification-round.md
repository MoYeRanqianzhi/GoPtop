# 2026-09-15 大工程轮：传输层 Rust 化 + 全部遗留候选

> 用户指令：「完整进行 Rust 化，以上全部需要完成」。
> 即 TODO「下一步候选」五项全部完成，Rust 化是核心。
> 执行纪律：单代理串行（账号并发受限）；每阶段独立 commit，验证链全绿才提交。

## 范围（五项）

1. **围棋劫争 + 终局数目**（crates/goptop-core Phase 4）
   - ko：简单劫（禁止立即回提同形）。ko_point 随 try_play 更新，undo 重放自然恢复。
   - 终局：连续双 Pass → scoring 阶段。
   - 数目：中国规则区域法（子+地），贴目 7.5；死子由双方标记（交集为死），双确认出结果。
   - 同步协议：ScoreMark/ScoreConfirm 走协商通道（goptop-net 定义）。
   - adopt 改为历史重放重建 captures/ko（顺带修复 P3-3 captures 清零）。
2. **协商弹窗队列**：confirmReq 单槽 → VecDeque 队列（在 goptop-net session 状态机实现）。
3. **传输层 Rust 化（第二阶段）**——主体工程
   - 方向拍板：**wasm-bindgen + web-sys**（唯一覆盖四端的路线：Web/Tauri/Android/鸿蒙 ArkWeb
     全是 WebView；Tauri 原生桥接会砍掉 Web 与鸿蒙端）。
   - 新 crate `goptop-net`：纯逻辑（协议线格式/URL 链接编解码/G1 codec/去重/身份/会话状态机/
     观战房间/协商队列），serde 对齐现有线格式，充分单测。服务器协议不变（纯转发）。
   - 新 crate `goptop-transport`：wasm-bindgen + web-sys 封装 RTCPeerConnection/WebSocket/
     BroadcastChannel/localStorage，导出 WasmSession（状态快照事件流 + 方法集）。
   - 前端 useGameSession 瘦身为 UI 绑定壳；net/* TS 逻辑层删除。
4. **无服务器跨设备观战**：观战回执流（spec 链接 → 观战者 offer → 回执回传 → 直连），
   在 goptop-net 状态机内设计实现。
5. **审查低危残留**：台账 2026-09-13-review-round.md 全部「待修」项
   （服务器侧 unreachable/muted 强制/转发目标/字节上限/tiebreak；UI 侧 P2/P3 批量）。

## 顺序与提交切分

- [x] T0 计划与方向拍板落盘
- [ ] T1 围棋 ko+终局+数目（core 单测 → wasm 绑定 → 前端 UI）
- [ ] T2 服务器 P3 快修（main.rs 等，独立于前端）
- [ ] T3 goptop-net 纯逻辑 crate + 单测（含协商队列、无服务器观战回执、scoring 同步状态机）
- [ ] T4 goptop-transport wasm crate（web-sys 三通道 + WasmSession）
- [ ] T5 前端接线瘦身 + E2E 42 断言全绿 + 删 TS 死代码
- [ ] T6 无服务器跨设备观战 UI + E2E
- [ ] T7 UI/文档 P3 残留批量清理
- [ ] T8 文档收口（architecture/p2p-protocol/已知限制/使用指南/部署指南）+ memory + 官服部署
