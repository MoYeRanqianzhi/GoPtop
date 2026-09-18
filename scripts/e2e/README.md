# E2E / 浏览器回归脚本

浏览器回归基线（审查轮 #6 B1 起版本化入库）。前置：`npm install`（本目录，
playwright 依赖）+ `npx playwright install chromium`；本机 dist 静态服务 +
本地信令服务器已起（见下）。所有脚本无框架依赖，`node <script>` 直接跑。

## 环境准备（三个终端或后台任务）

```bash
# 1) 构建前端 + 起两份静态服务（SPA 回退 index.html）
npm --prefix frontend run build
node scripts/e2e/serve.js 5173 localhost frontend/dist
node scripts/e2e/serve.js 5174 127.0.0.1 frontend/dist
# 2) 本地信令服务器（run.js 用 ws://127.0.0.1:9527/ws）
cargo run -p goptop-server -- --listen=127.0.0.1:9527
# 3) 依赖（本目录）
cd scripts/e2e && npm install && npx playwright install chromium
```

## 脚本清单

| 脚本 | 用途 | 断言/产物 |
|---|---|---|
| `run.js` | 服务器模式全流程 E2E：A/B 对局+聊天+悔棋/换棋/重开协商+C 观战全链+D/E/F/G 大厅挑战+聊天停靠栏三档几何 | 58 项 check，退出码即结果 |
| `go-capture.js` | 围棋提子回归（wasm 规则下沉）：9 路 7 手提白+悔棋还原 | 7 项 check + shots/phase5/ |
| `ui-audit.js` | UI 审查：515px 窄屏遍历全部页面/状态（纯按钮导航，非人类路径不触碰） | shots/audit-*.png 人工核验 |
| `challenge-server.js` | 大厅挑战握手专项 | 控制台 check 行 |
| `crossnet-local.js` / `crossnet-remote.js` | P2P 跨网实测驱动（本机侧/远端侧，配合 ssh remote 使用） | 见 .agents/docs 内跨网纪要 |
| `device.js` | 跨端端点抽象（browser/mob/android/cdp）+ 真实输入原语 + 五子棋对局策略 | 被下面各驱动复用 |
| `match.js` | 两两对战三场景：game / chat / watch | 控制台断言行 + 退出码 |
| `matrix.js` | 矩阵批跑（内置五端 11 对组合） | 汇总表，退出码即结果 |
| `features.js` | 设计功能：local / go / challenge / resign（聊天区内）/ specchat / kick | 每场景 PASS/FAIL 计数 |
| `diag-pair.js` / `diag-watch.js` | 配对与观战镜像诊断（逐手打印各端手数与盘面差异） | 控制台逐手行 |
| `serve.js` | 静态 dist 服务器（SPA 回退） | `node serve.js <port> <host> <distDir>` |

跨端（桌面壳/安卓/鸿蒙）测试请先读 `.agents/docs/real-device-testing.md`（端点规格串、
启动方式、驱动层坑位）。

回归口径：`run.js` 58 项 + `go-capture.js` 7 项全绿 = 浏览器面基线；vitest
（frontend/）与 `cargo test -p goptop-core` 为单测面。改动信令/协商/观战/
服务器后必须复跑 `run.js`。
