# 2026-10-07 探查 bug 大轮：九维审查 36 实证 → 八组并行修复 → 全基线回归

用户指令「加强测试和优化，探查bug」。审查面 = 上一轮 39 提交（约 7800 行原生传输接线）。

## 数字

- 9 维并行审查（53 agent）→ 44 条发现 → **对抗验证确认 36 / 驳回 8**。
- 8 组修复 agent（互斥文件范围，各自「修→回归测试→全绿→提交」）→ 8 提交全落位。
- 回归：cargo workspace 全绿（goptop-net 56、goptop-ai 41）、vitest 70/70、tsc 干净；
  Web 基线 run.js 58/58、go-capture 7/7、ai.js 9/9、noserver-pair 9/9、stress 120 手。

## 四条 high（修复均在 128502f / 81d8fb2）

1. **同源挑战永久砖死**：accept_challenge_with 采纳受邀者 gameId 却不发 JoinChannel
   ——Rust 迁移自 TS 时漏掉 transport.join（对照 accept_receipt 平行路径即可发现此类遗漏）。
2. **远端 SyncState ragged 棋盘越界 abort**：先入库后校验。修法=先校验后落账 +
   apply_move/can_place 索引 .get 双保险。
3. **线格式 Reset{size:65535} → 86GB 分配 OOM**：修法=(kind,size) 统一经
   make_engine_kind 归一化，归一化结果是唯一尺寸来源（不要另写白名单）。
4. **前端从不调 session_drop**：每次 WebView 重载漏一个原生会话（20Hz 泵+presence+WS）。
   修法=GameSession.dispose + adopt-guard（StrictMode 双挂载只释放自己建的会话）。

## 主题级教训

- **「迁移遗漏」类 bug 的检测法**：Rust 迁移自 TS 的逻辑，找 TS 时代的同位置对照
  （accept_receipt 有 JoinChannel 而 accept_challenge_without 没有，就是信号）。
- **「信任边界」类 bug 成串出现**：Reset size、ragged SyncState、deflate 炸弹、
  gomoku history 坐标、AnalyzeRequest kind——全是「线上输入未校验直进引擎/分配」。
  一处确认后应同型扫一遍（本轮就是这么成串抓出来的）。上限类修法用流式 take(MAX+1)，
  不要先解完再比长度。
- **会话生命周期要两端闭环**：宿主有 session_drop 命令 ≠ 前端会调；泄漏在
  「重载/StrictMode 双挂载」时发生，正常路径测不出来。
- 对抗验证驳回了 8 条（如「锁中毒连环 panic」前提事实错误），**报告级发现必须实证**。

## 环境/工具坑（本轮新踩）

- **Git Bash 里 node 被 profile 别名成 `winpty node.exe`**：后台任务（非 TTY）里
  `node` 直接死（"stdin is not a tty" 退出码 1，零输出冒充测试失败）。后台跑 node
  脚本一律用 `node.exe` 直调或 `exec node`。
- **rustup 工具链更新会丢已装 target**：wasm32-unknown-unknown 与 OHOS 双 target
  同时消失，报 E0463 "can't find crate for core"。`rustup target list --installed` 先查。
- **管道 tail 吞退出码**：`bash build.sh | tail -2 && …` 的 && 链看 tail 的 0，
  构建失败照样往下走。要 `rc=$?` 显式接。
- **serve.js 之类常驻进程**：Bash 工具的 `&` 后台随 shell 退出被杀，必须 run_in_background。

## 遗留（记录在案，未获指令不动工）

1. **transport accept_answer 的 stale-answer 子态**：[3]（坏回执重试）的主修复已落地
   （状态机受理重试、钥匙进对局才消费），但「answer 有效且已应用、连接未成」时第二次
   answer 在 stable 态会被静默吞掉。rollback 在 stable 态不可用（WebRTC 只允许
   have-local-offer 回滚）；正解是「stable ⇒ 用会话保存的原 offer 重建 peer」或
   「失败 surface 成 Event→Notice」，属真设计活，留专门一轮。
2. **AI 执黑时点「重开」不接第一手**（F 组顺带发现的相邻既有缺陷）：resetBoard 后
   toMove 仍 black 且 effect 依赖不变，AI 不重跑——此前被幽灵落子掩盖成死局。
   需产品层拍板是否 resetBoard 后强制重跑 AI effect。
3. E2E 壳甲类覆盖（shell-pair/ohos-native-probe）随本轮产物已重建，实机矩阵未重跑
   （桌面壳 debug 构建后 shell-pair 已跑；安卓/鸿蒙模拟器矩阵留待下轮实测轮）。
