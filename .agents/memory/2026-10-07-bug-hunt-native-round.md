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

1. **transport accept_answer 的 stale-answer 子态——B 方案已落地（7348822 + c9f5010）**：
   set_remote 失败经 `Event::RtcApplyFailed` 进状态机，邀请者等待态弹可行动提示；
   非等待态维持静默。**端到端 E2E（receipt-retry.js 6/6）又抓出两处单测盖不住的真路径缺口**：
   守卫按槽位在册过滤会在「ICE 升级 failed → gone 移除槽位」时间线永远静默（改只看
   phase/role）；**UI 在 accept_receipt 清掉 invite_url 后连回执按钮都没了**——重试窗口
   只在状态机里存在（P2pPage inviter 等待卡补了入口）。关键界面已截图目检（等待卡新
   分支、提示折行渲染、AI 重开三态）。**教训：文本断言在 DOM 里 ≠ UI 正确，改动点必须
   截图亲眼看；E2E 前提假设（如「ICE 必升 failed」）要被诊断输出证伪就当场改**。
   **A 方案仍未做**：stable 态用会话保存的原 offer（`PeerSlot.offer_plain`）重建 peer，
   让补发回执真正连上（当前提示引导走「重发邀请」的重路径）——按实际发生率再决定。
2. **AI 执黑思考中重开的死局——已修（7348822）**：genTick 渲染代次进 AI effect 依赖；
   ai.js 的死局回归检查做过红验证（无修复时手数=0 死局，修复后手数=1）。
3. **壳测未随本轮修复重跑**：壳 release 构建在链接前被系统内存回收（当前
   target/release/goptop.exe 仍是 9月20日 旧二进制），shell-pair/ohos-native-probe
   与安卓/鸿蒙模拟器矩阵留待内存宽裕时按 build→起壳/模拟器 串行重跑。
