# 2026-09-19 离线 AI 层（人机对战 + 实时胜率）

用户拍板：「全部都需要做」（五子棋 + 围棋，两个功能都要）；允许「参照算法原理自行 Rust 重实现，
工作量不是问题」；**避免主线程冻结**；胜率「全部模式」显示；人机对战**独立成页** `/ai`；
胜率形态为**红蓝单挑线（红＝我方、蓝＝对手）+ 走势图**，放底部与「对局」卡切换。

## 选型的硬约束（不是偏好，是查证结果）

- **Rapfi 是 GPL-3.0**，本项目 `MIT OR Apache-2.0` → 源码路线直接出局。iomrascálaí 同为 GPL。
  **任何"复用现成引擎"的建议都必须先查许可证**，这是上一轮外部调研完全漏掉的一条。
- **KataGo 虽 MIT，但 Eigen 后端仅 10–20 playouts/s**，wasm 更慢，围棋不可用。
- **围棋没有可直接复用的宽松许可引擎**（go_game_board 只有 playout；yamcts 仅 327 行、停更）
  → 自建 MCTS 是唯一选项。

## 两个必踩的 wasm 陷阱（编译期不报错，运行时才炸）

1. **`std::time::Instant::now()` 在 wasm32-unknown-unknown 上 panic**（"time not implemented"），
   被编译成 `unreachable` 陷阱。figrid-board 的 search.rs/vct.rs/tss.rs/dfpn.rs 有 20+ 处 →
   必须 vendor + 把 `use std::time::` 换成 `use web_time::`（见 `crates/goptop-ai/vendor/README.md`）。
   **`time_limit=None` 也躲不掉**（有鉴于此，VCT 路径无条件调用）。
   自建代码一律用 `web_time::Instant`。
2. **cargo 默认 feature 会带进平台专属依赖**：go_game_board 的默认 feature 含 `perf-event`
   （依赖 `libc::clockid_t`），wasm 编不过 → 必须 `default-features = false`。

## 实测数据（可复现）

| 项 | 数值 |
|---|---|
| 围棋 playout（wasm32 + node） | **31,746 playouts/s**，平均 115 手/局，等效 ~365 万 moves/s |
| 同上 native | 35,356 p/s（**wasm 只慢 10%**） |
| 五子棋搜索（native 与 wasm 一致） | 1s 预算 → depth 7；时间控制精准（1000ms 预算实耗 1003ms） |
| AI wasm 体积 | 2.56MB（含内嵌 1.7MB gzip 权重；LTO+opt-level="s" 之后） |
| 围棋 playout 库体积 | 65KB |

## 五子棋胜率的标定（不能拍脑袋）

figrid 的 `score` 是 **negamax 的「轮到走的一方」视角**（实测：同一局面黑先 +999000、
白先 -998998）。`|score| >= 998_000` 是"已算杀"哨兵，真实中盘分只有百位量级。
自我对弈采样 42 个非哨兵样本（p50=90）→ 取 logistic 尺度 **s=150**，均势 ≈0.5、
中盘 ±90 分走到 0.35/0.65、必杀分饱和到端点。标定表钉在 `odds.rs` 的测试里。

## 架构要点

- `crates/goptop-ai` 是**独立 wasm**（与规则引擎分开）：含 1.7MB 权重，且只在需要分析时加载。
- **分析必须走 Web Worker**：wasm 单线程，1 秒搜索中间没有任何可让步的点，会整个冻住主线程。
- **局面一律由 Rust 序列化**：本地页 `WasmGame.state_json()`、P2P/观战 `WasmSession.state_json()`。
  前端手工拼不可行——围棋的劫点/提子数只在引擎里，拼错不报错、只表现为 AI 下怪棋。
- AI 的着法**仍走 `RulesEngine.place()`**（规则真源裁决），AI 只提供坐标。
- 底部卡宽屏「规则＋对局/胜率」、窄屏 `.bp-swap` 三态；**宽窄两态的 tab 是两个独立 state**
  （共用一个会让"窄屏切到规则→窗口变宽"后第二张卡也显示规则，与固定规则卡重复）。

## 用户交互口径

- 「人机对战这里放一个设置（类似 p2p 中的聊天区，将各种功能都放在这里，避免 ui 显示太挤）」
  → 难度/先后手/悔棋/重开全收进一个设置面板（复用 `.chat-modal` 样式但 `height:auto`——
  它的固定高度是给聊天记录列表用的，照搬会撑出一大片空白），状态行只留一个齿轮入口。
- 图标必须用**图标库**（`lucide-react`，MIT、tree-shakeable：1930 个模块参与分析、
  产物只 +5KB），不许手画 SVG 也不许拿文字顶替。
- 底部三卡**必须等高**（用户拍板）：三个 tab 内容长短不一，不锁高则每次切换都改棋盘
  可用高度——实测 对局 142 / 规则 130 / 胜率 180，切换瞬间棋盘跳 50px。锁到 130px 后
  棋盘反而大了 50px（472→522）。胜率卡内部为此整体压紧：文字与条并成一行、走势图 38px。
- 走势图是**滑动时间窗口**（每手 6px、约 40 手/250px），不是画全部手数；不足两手时
  **不画任何线**（含中线），只留虚线框——滑动窗口下先画一条横贯全宽的直线、下一手再
  缩成一小段，那个「塌掉」的瞬间比留空更扎眼。

## 围棋的核心缺陷：根节点没有先验（2026-09-19 修）

**症状**：9 路空盘的首选在固定种子下随搜索量漂移——(6,2)/(1,5)/(5,2)/(4,4)/(2,4)/
(1,2) 都出现过，好几个在二线，用户看到的是「AI 第一手像乱下」。

**根因**：根节点选择是纯 UCT，**完全没用先验**。围棋分支因子约 250，每个候选只访问
几十次时，随机 playout 分辨不出空盘点位优劣——实测前五名的 q 挤在 0.50~0.53
（标准误约 0.018），`argmax(visits)` 等于在噪声里取最大值。原实现还把未访问节点
一律记 `+inf`，等于按随机展开顺序挑第一个。

**修法**：PUCT，`u = C_PUCT · P(s,a) · √N / (1 + n)`。P 由 `Sampler` 在根局面上按
gamma 分布采样 3000 次统计频率得到（模式化策略本就是「这一手有多像样」的现成度量）。
拉普拉斯平滑不能省——gamma 从不选的点若 u 恒为 0，等于被悄悄删出搜索空间。

**验证靠变异测试**：`C_PUCT=0`（关掉先验）→ 空盘首选落到 (4,2) 二线、新增的
`go9_empty_first_move_is_stable` 立刻变红；恢复 1.6 → 回到中心 3×3。

## 全平台实测（2026-09-19，AI 层）

| 端 | 结果 |
|---|---|
| Web（Chromium） | `ai.js` 9/9；`stress.js` 120 手 8/8（JS 堆 21→21MB 无泄漏、走势图点数=窗口容量 40） |
| 鸿蒙 ArkWeb 6.0 | `ai.js` 9/9 |
| Android WebView | `ai.js` 9/9 |
| Tauri 桌面 | 见下（构建/测试于同日进行） |

**关键结论：Web Worker + wasm 在鸿蒙 ArkWeb 与 Android WebView 里都正常工作**——
这是接入 AI 时最大的未知项（主线程隔离方案能否成立完全取决于它）。

**壳端点测试的两个坑**：
- 模拟器访问不到宿主机的 `localhost:1420`，且各壳资源 origin 不同（Tauri 是
  `http://tauri.localhost`、鸿蒙是 `https://appassets.goptop`）→ `ai.js`/`stress.js`
  对壳端点自动从当前页面推导 origin，不传 base。
- 鸿蒙 `hdc fport` 的 socket 名用**主进程** pid（`webview_devtools_remote_<主进程pid>`），
  不是 render 进程的 pid——实测 render pid 转发后 CDP 端点无响应。

**构建竞态**：Tauri 在编译期嵌入整个 `frontend/dist`。若构建期间并行跑 `npm run build`
重建了前端，旧 hash 文件名被删，Tauri 会报 `failed to read asset at .../dist/assets/xxx.wasm`。
**构建与前端重建不能并行。**

**Why:** 这一轮把「复用开源算法」的天真预期撞碎在许可证与 wasm 运行时两道墙上——两者都不是
读文档能发现的（许可证要查、wasm 时钟问题编译期完全静默）。

**How to apply:** 引入任何第三方引擎前先查许可证与本项目的兼容性；引入任何 Rust 库到 wasm 前，
先 `cargo check --target wasm32-unknown-unknown` **并真的在 wasm 运行时里调用一次**（编译通过
毫无意义）。相关：[[2026-09-19-platform-storage]]、[[2026-09-19-comment-audit-and-doctrine]]。
