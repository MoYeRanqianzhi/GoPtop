# 2026-09-12 userId 链接回归 + 协商/聊天/观战房间（大功能轮拍板与实现记录）

## 用户拍板（本轮多次迭代后最终形态）
1. 邀请/观战链接恢复 `/<userId>?pwd=` 形式（短码机制废弃）；观战链接带 `spec=1`，
   pwd 是**观战特有钥匙（整局有效）**。
2. pwd 全部**客户端校验**：对→自动同意；错→弹窗询问是否接受（对局邀请）/进聊天区
   私有申请（观战）；对局中→自动拒绝并说明「正在对局中」。
3. 服务器定位收窄为**纯转发管道**（名册 + signal 通用转发 + relay 兜底），
   **不落地任何数据**——pwd/offer 只过手不存储，全内存断线即清。
4. 官服 = free28（wss://goptopserver.meowoo.org/ws），1Panel openresty 反代 + acme.sh 证书。

## 关键实现决策
- **名册 ID = 客户端持久 userId**（hello 带 userId 注册，u- 前缀；同 ID 重连顶替旧连接）。
  链接目标与信令路由必须同一套 ID——曾因服务器自分配 s- 短 ID 导致 join 静默丢弃。
- **signal 信封**：`{t:"signal", kind, from, payload}`（payload 嵌套）——曾把 kind 平铺进 t
  导致客户端分发静默落空。
- **协商消息必须去重**：三链路（BroadcastChannel/直连 DC/relay）重复送达，
  SwapAck 被应用两次颜色翻回。GameChannel.dispatch 去重范围从仅 Move 扩展到
  Move/Undo*/Reset*/Swap*/Avatar；SyncState/SyncRequest/Chat 保持不去重（seq 归零）。
- 观战发言批准：第一 host 批准后转发申请（payload 带 `applicant` + `relay:true`），
  第二 host 只向申请者回 ack——applicant 字段缺失会把 ack 错发给转发者。
- 踢人顺序：**先发 spec-kicked 通知再从名单移除**（移除后 spec-sync 送不到被踢者）。
- game-layout 必须占满 main 剩余高度（flex:1+min-height:0），否则棋盘高度塌缩、
  等宽缩放把整组算成最小宽（等待页卡片变窄条的 UI bug 根因）。

## 测试基建
- Playwright 三端 E2E：`%TEMP%/goptop-e2e/run.js`（26 断言全过），chromium 用
  `ms-playwright/chromium-1228/chrome-win64/chrome.exe`（本机缓存版本与 npm 包要匹配）。
- vite HMR 会在编辑 useGameSession 时打断长流程测试——多端流程验证必须用独立脚本一次跑完。
- 遗留待办：官服跨设备观战 E2E（本机已验）；错 pwd 弹窗接受流、禁言/关闭观战 UI 细节
  在 E2E 里未覆盖（代码已实现）。
