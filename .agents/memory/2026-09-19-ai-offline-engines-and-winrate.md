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

**Why:** 这一轮把「复用开源算法」的天真预期撞碎在许可证与 wasm 运行时两道墙上——两者都不是
读文档能发现的（许可证要查、wasm 时钟问题编译期完全静默）。

**How to apply:** 引入任何第三方引擎前先查许可证与本项目的兼容性；引入任何 Rust 库到 wasm 前，
先 `cargo check --target wasm32-unknown-unknown` **并真的在 wasm 运行时里调用一次**（编译通过
毫无意义）。相关：[[2026-09-19-platform-storage]]、[[2026-09-19-comment-audit-and-doctrine]]。
