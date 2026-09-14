# 2026-09-15 · 传输层 Rust 化方向拍板（wasm + web-sys）

## 决策（用户指令：「完整进行 Rust 化，以上全部需要完成」）

- **方向：wasm-bindgen + web-sys**，否决「Tauri 原生桥接」。
- 理由：GoPtop 四端（Web、Tauri Windows/Linux、Android、HarmonyOS ArkWeb 壳）
  **全部运行在 WebView**。web-sys 路线一份 wasm 产物覆盖四端；原生桥接只覆盖
  Tauri 系两端，Web 与鸿蒙端会被砍掉，违背「Rust 承接一切功能」的总要求。
- 分层：
  - `goptop-net`（纯逻辑，无 IO）：协议线格式 / URL 链接 / G1 codec / 去重 / 身份 /
    会话状态机（含协商队列、观战房间、无服务器观战回执、围棋 scoring 同步）。serde 对齐
    现有线格式，服务器协议零改动（纯转发）。
  - `goptop-transport`（wasm-bindgen + web-sys）：RTCPeerConnection / WebSocket /
    BroadcastChannel / localStorage 封装，导出 WasmSession（事件快照流 + 方法集）。
  - TS 只剩 React 渲染 + DOM 事件绑定。
- **Why**：用户 2026-09-13 重申总要求「Rust 承接一切功能，TS 只做 UI」，本次指令为
  第二阶段（传输层）完整落地。
- **How to apply**：新传输/协议逻辑一律进 goptop-net/-transport；禁止在 TS 侧新增
  协议或状态机代码；线格式改动必须同时核对服务器转发兼容（payload 原样转发则兼容）。

## 关联

- [[2026-09-12-userid-link-negotiation-chat-spectator]]
- [[2026-09-13-cleanup-refactor-round]]
