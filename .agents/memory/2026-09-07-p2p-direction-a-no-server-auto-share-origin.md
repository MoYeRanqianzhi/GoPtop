# 2026-09-07 · P2P 连接方向拍板：A 路线（纯无服务器）+ 自动分享域名

## 决策

用户明确拍板，后续开发必须遵守：

1. **保持 A 路线（回执机制保留）**：跨设备 P2P 信令只靠「邀请链接 → 回执链接」两次复制粘贴，不引入任何信令服务器。未来会支持自建服务器作为可选，但**始终以无服务器为主**。
   - 用户理由：无服务器的优势是有服务器无法相比的——基本相当于永生，不依赖任何会被外界因素暂停的服务。业务逻辑上（邀请/回执/直连）不得引入服务器依赖。

2. **分享域名必须全自动，禁止做成用户可设置项**：曾把「设置分享域名」做成设置页入口 + 弹窗编辑，用户明确否决（「完全有问题，完全不可取」）。
   - 正确形态：Web 下自动用 `window.location.origin`；非 Web（Tauri）下用代码里的常量 `SHARE_ORIGIN_NATIVE`（默认 `https://goptop.pages.dev`，改一处常量即全局切换）。用户不可见、不可改。

## 落地状态

- `frontend/src/net/transport.ts`：`getShareBase()` 改为纯自动（无 localStorage、无 setShareBase）；回执/邀请/主页/观战链接全部基于它。
- `frontend/src/App.tsx`：设置页「分享域名」区块、`share-base` 弹窗分支全部移除。
- 回执流程：受邀者生成回执弹窗（receipt）→ 邀请者等待页「输入回执」弹窗（paste-answer）粘贴。回执链接含 `pwd + rtcAns + game + kind + size`，邀请者校验后切到受邀者 game channel 进对局。
- 消息去重：GameChannel.dispatch 对 `Move` 按 `(sender, seq)` 单调去重（BroadcastChannel 与 WebRTC 双链路送达同一消息只应用一次）；`SyncState/SyncRequest` 不去重（幂等全量同步）。
- 回执链接被当页面打开时：只提示「请在邀请者等待页输入回执」，绝不据此发起挑战（防止同机两窗互弈）。

## 教训

- UI 暴露「应该自动决定」的量是设计错误，用户当场否决；此类量（部署地址等）一律走代码常量。
