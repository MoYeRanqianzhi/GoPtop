# 2026-09-07 · 修复轮审查报告（审查对象：7df4dd5 → 99fbb4c 两轮修复）

> 审查人：主代理（单代理串行，遵守 memory/2026-09-07-no-parallel-agents-single-serial）。
> 审查立场：不信任修复记录的文字宣称，逐项对照代码核实；修复轮新引入的问题单独标 R 级。
> 原始审查：review/2026-09-07-main-agent-code-review.md（A/B/C/D 级编号沿用原报告）。

## 0. 结论

**两轮修复整体质量高，可以接受。** 原报告 8 个 A 级 bug 全部真实修复且方向正确；C 级注释全部对齐实现；D 级除两项标注「可选」外全部落地；B 级断层以「注释实况化」方式诚实消解（Rust protocol.rs/lib.rs 头注释现明说「尚未接线，前端未使用、TS 是唯一真源」，与事实相符）。修复代码普遍带有 WHY 注释与原 bug 引用（如「审查 A7」），后续 agent 可追溯。

修复轮新发现 6 项（R1-R6，无 A 级严重回归；最重两项是死代码复活与一个新竞态窗口），另有 3 项原观察项记录在案。

## 1. 本审查实测验证链

| 检查 | 结果 |
|---|---|
| `npx tsc --noEmit` | 0 错 |
| `npx vitest run` | 30/30 通过（与宣称例数一致，逐项核对覆盖面相符：去重/编码往返/坏前缀/截断/错钥/genPwd 熵/解析全分支/spec=1） |
| `cargo test --workspace` | 26/26 通过 |
| `npm run build` | 成功（258.39 kB） |
| 浏览器 E2E | 本轮未复跑（原窗口已做 Playwright 双窗口回归；本轮以代码级核对为准） |

## 2. 逐项核实（A 级，全部确认修复）

- **A1 ✓** `transport.ts:840-852`：`answered` 移到 `setRemoteDescription` 成功后置位；catch 内 `if (this.answered || this.pc.remoteDescription) return` 做双路径容错。坏回执不再锁死。
- **A2 ✓** `transport.ts:43`（Move 带 `by`，附「不得从本地推断」WHY 注释）+ `useGameSession.tsx:189-221`：接收端 Place/Pass 按 `by` 校验行棋方（`mover !== toMoveRef` 丢弃），Resign 按 `by` 判胜者（`by === "black" ? "white" : "black"`）——观战者、发送方回流均正确。无 `by` 的旧格式消息丢弃（不猜）。发送端 `handlePlace` 带 `by: mover`（:808）。
- **A3 ✓** `useGameSession.tsx:162-176`：SyncState 以 `history.length` 作快照版本，`k.history.length < historyRef.current.length` 丢弃旧快照，注释如实说明「同长视为同版本」。已推演组合场景：SyncState 先到 + Move 后到会被 `mover !== toMove` 校验挡住不重复应用；Move 先到 + 旧空盘 SyncState 后到被本守卫挡住。守卫方向正确（残差见 §5-观察2）。
- **A4 ✓** `useGameSession.tsx:491-511`（applyAnswer 返回错误文案、peer 缺失显式报错）+ `:516-563`（acceptReceipt 先 `await applyAnswer` 成功才推进 phase/pwd/join；失败弹窗保留可重试）。**验证了坏回执旅程**：解码失败 → 弹窗内报错 → phase 仍 waiting、pwd 未失效 → 同弹窗可再贴。
- **A5 ✓** `useGameSession.tsx:409-433`：`setInviteUrl(null)` 起步，offer 成功才出现含 rtc 链接；失败退化为无 rtc 链接并如实提示「跨设备不可用」。App.tsx:197-205 生成中只显示提示+取消，无链接可复制。
- **A6 ✓** App.tsx:209-214：受邀者等待页 `answerBackUrl` 非空时显示「查看回执」重开入口。
- **A7 ✓** `useGameSession.tsx:700-708` `closeAllRtcPeers()`（含 transport.leave + 关 peer + 清弹窗），createInvite/acceptInvite/joinAsSpectator/backHome 四入口统一调用（:394/:440/:677/:711）。
- **A8 ✓** `useGameSession.tsx:281-298`：对局中、受邀者等待中、邀请者等待但钥匙不符——一律 `presence.reject` 回信，挑战方不再死等（其 reject 处理已有 backHome）。

## 3. 逐项核实（B/C/D 级，全部确认落地）

- **B1/B2/B3 ✓** protocol.rs 头注释改为「参考实现（尚未接线，前端未使用）+ TS 是现行协议唯一真源 + DTLS 非 iroh」；transport lib.rs 明示「当前无任何调用者」并引无服务器红线；p2p.rs 同步。注释与事实完全相符。
- **C1-C8 ✓** 跨设备观战注释改「尚未实现」（transport.ts:12-14）；CompressionStream 兼容面写实并注明无降级（:616-624）；BoardSvg「悬停」；C4 枚举化（go.rs 返回 `RuleError`，game.rs 删除字符串匹配，`?` 直传）；C6 WHY 注释随 A2 重写；C7/C2 死代码删除（Stone.tsx、game/rules.ts，git 证实）；C8「确认回执」（App.tsx:530）+ header 常驻「输入回执」（App.tsx:100）——用户拍板三条全部落地。
- **D3 ✓** `transport.ts:93-97` CSPRNG 固定 6 位 base36（注释自认取模偏差；实测偏差 ~1.4%，注释写 ~2.7%，方向保守无害，见 R6）。
- **D4 ✓** `transport.ts:321-327` 单段路径须 u- 前缀或 pwd/rtc 参数，测试覆盖裸中文/IP:port 拒识。
- **D5 ✓** `transport.ts:756-761` 仅 `failed` 判死，disconnected 注释说明可自愈。
- **D6 ✓** `transport.ts:398-400/420/428` 实例字段箭头函数，add/remove 严格配对。
- **D7 ✓** `useGameSession.tsx:765-768` closed/error 出列（peer.close() 触发 setState("closed") → onState → 出列，手动关闭也被覆盖）。
- **D8 ✓（有残留宣称偏差，见 R3）** showNotice 单入口 + timer 管理，主体生效。
- **D9 ✓** `useGameSession.tsx:321-326` 观战者上报 idle。
- **D10 ✓** `game.rs:97-103` 尺寸不变量 assert（Gomoku=15 / Go∈{9,13,19}），注释说明「内部保证、外部输入在入口校验」的分层。
- **D11 ✓** vitest 30 例 + `npm test` script；顺修 deflate/inflate writer promise 未 catch（transport.ts:634-645）。
- **D1 ✓** 三刀拆分：App.tsx 544 行纯壳、useGameSession.tsx 902 行状态机、pages/components.tsx、pages/LocalPage.tsx、game/board.ts 规则唯一实现。逐段对照确认「原样搬出」承诺成立（未见行为漂移）。
- **C4 ✓** 见上。**术语轮 ✓（有残留，见 R5）**：inviter/invitee 全套标识符与文案替换，ICE host 候选按拍板保留。

## 4. R 级：修复轮新发现（按严重度排序）

### R1（中）gameStore.ts 死代码复活 + 三处注释互相矛盾
- `frontend/src/state/gameStore.ts` 仍含完整 `checkFive` 实现（:16-33），全库零引用（grep 证实，仅 game/board.ts 注释提及它）。
- 三处宣称冲突：
  - `game/board.ts:2` 自称「checkFive 与 emptyBoard 的**唯一实现**」；
  - `architecture.md:93` 称 gameStore.ts「瘦身为**注释态**」——实际是活代码；
  - `gameStore.ts:4-6` 自称与「App 内联副本」重复——App 内联副本在 D1 拆分中已消失，注释描述的镜像关系已不存在。
- 恰是原始审查 B4（重复实现被静默漂移）想杜绝的形态：下一次有人改五连判定，很可能只改其中一份。**修：删除 gameStore.ts（CLAUDE.md 未用即删），并同步 architecture.md。**

### R2（中低）acceptReceipt 的 await 间隙竞态（A4 修复引入）
- `useGameSession.tsx:532` `const err = await applyAnswer(r.rtcAns, r.pwd)`：await 期间用户可点「取消等待」（backHome：leave channel、清 refs、nav("/")）；applyAnswer resolve 后**续体无条件执行** `transport.join(r.gameId)` + `setPhase("playing")`——用户已回主页却被拉进对局状态。
- 窗口 = decode + setRemoteDescription 时长（快则毫秒级，坏网/大 SDP 可到秒级）。旧实现同步推进无此窗口，属修复引入。
- **修：await 后复查 `phaseRef.current === "waiting" && roleRef.current === "inviter"`，不满足则放弃续体**（一两行）。

### R3（低）D8 宣称「接管全部调用点」，实际残留 2 处直调
- `useGameSession.tsx:424`（createInvite 成功路径）与 `:726`（backHome）仍直调 `setNotice(null)`，绕过 showNotice 的 timer 管理。
- 实际效果无害（均为清空操作，且 showNotice 设新值前先清旧 timer），但 TODO.md「接管全部调用点」的记录与代码不符——下次审计又会踩「宣称 vs 实现」。**修：两处改 `showNotice(null)`，或修正记录措辞。**

### R4（低）components.tsx 头注释停留在拆分第一刀状态
- `pages/components.tsx:6`：「对局状态机与信令编排**仍留在 App.tsx**（拆走反造间接层）」——第二刀已把状态机挪进 useGameSession.tsx，该注释现在误导。App.tsx:2 的「useGameSession.**ts**」也少写 x（文件实为 .tsx）。**修：两处注释对齐。**

### R5（低）术语轮残留 4 处
- `transport.ts:219` 注释 `<分享域名>/<host>?pwd=`（应为 inviterId 语义）；`.agents/docs/p2p-protocol.md:9-10` URL 表列名仍 `<hostId>`；`transport.test.ts:214,218` 夹具名 `u-host01`。功能无影响，但与「全套改 inviter/invitee」的完成宣称不符。

### R6（极低，可不改）genPwd 取模偏差注释数字失实
- `transport.ts:96` 注释「取模偏差 ~2.7%」——实测 36^6 与 2^32 的偏差为 ~1.36%。宣称比实际更差（保守方向），仅数字不准。

## 5. 观察项（非本轮引入，记录备查，不要求本轮修）

1. **createInvite 异步完成于取消后**：取消等待后 offer 异步链仍会写 `setInviteUrl/setDirectState/inviterRtcRef`（:420-432）。多数视图不显示这些 state，下一换局入口会清理；与 R2 同属「异步续体不复查 phase」模式，修 R2 时可顺手一并加守卫。
2. **A3 守卫残差**：同长快照无条件应用（已注释声明）；Reset 消息无版本号——理论上 Reset 在途时旧 SyncState（history 更长）会复活旧局，需「连接建立后 200ms 内重开」的极端时序，alpha 可接受。
3. **A5 失败路径文案**：offer 生成失败后 RtcStatusLine 仍显示「直连失败（可检查设置页线路）」——若失败源于 CompressionStream 缺失则与线路无关（C2 注释已写实，此 UI 文案未同步细分）。低优先。

## 6. 建议处理

R1/R2 建议下一轮立即修（各 ~10 分钟改动）；R3-R5 属注释/文案对齐，可攒一批处理；R6/观察项可不动。修完回填本文件「修复记录」。

## 修复记录

**2026-09-07 R 级修复轮（原窗口主代理执行）**——R1-R6 全部核实属实并修复：

- **R1** `state/gameStore.ts` 删除（git rm；零引用活代码）。architecture.md 死代码节同步。
- **R2** `acceptReceipt` 的 `await applyAnswer` 后新增守卫：`phaseRef !== "waiting" || roleRef !== "inviter"` 即放弃续体（useGameSession.tsx）。观察项 1 同模式顺手修：createInvite 异步续体加 `pwdRef.current !== p` 复查，取消后不再写本局 state。浏览器验证：跨设备回执正常路径双端进局、落子同步（手数 1 双端一致）。
- **R3** createInvite 成功路径与 backHome 两处 `setNotice(null)` 改 `showNotice(null)`——现在全部调用点均经单入口（showNotice 定义体内 2 处除外）。
- **R4** components.tsx 头注释改「拆分两刀完成后」实况；App.tsx 头注释 useGameSession.ts 少写的 x 补上。
- **R5** transport.ts:219 `<host>`→`<inviterId>`；p2p-protocol.md URL 表两处 `<hostId>`→`<inviterId>`；transport.test.ts 夹具 u-host01→u-inviter01。
- **R6** genPwd 注释改为实测数字：36^6 与 2^32 偏差桶 2118184960/2^32（单值概率绝对差 ~2.3e-10）。审查报告给的 ~1.4% 与原注释 ~2.7% 均不准——正确口径是「低段值出现 2 次、其余 1 次」的分布差，对 6 位钥匙无实际影响。
- 验证链：tsc 0 错、vitest 30/30、vite build 成功、Playwright 双窗口跨设备回执全流程（presence 屏蔽模拟跨设备）+ 落子同步。
- R3 修复时发现 showNotice 内部 setTimeout 回调曾被批量替换成递归 showNotice(null)——行为等价但已改回直调 setNotice(null)（定义体内不受单入口约束）。

## 修复记录审查（2026-09-07，审查人：主代理，commit 1c37671）

### 逐项核实（不信任宣称，对照 diff 与代码）

- **R1 ✓** `state/gameStore.ts` 已删（`frontend/src/state/` 仅剩 useGameSession.tsx）；architecture.md 死代码节改为「gameStore.ts 已删除」；game/board.ts 保留的历史教训注释（提及 gameStore 属历史引用，合理）。三方矛盾消除。
- **R2 ✓（守卫已加，但模式不彻底，残留 R2a/R2b，见下）**
- **R3 ✓** 两处 `setNotice(null)` → `showNotice(null)`；showNotice 定义体内 setTimeout 回调为直调 `setNotice(null)`——正确（定义体内清除是 timer 管理自身的一部分，若递归 showNotice(null) 反而多一次 clearTimeout）。全库 now 无绕过单入口的调用点。
- **R4 ✓** components.tsx 头注释改「两刀完成后…已抽到 state/useGameSession.tsx」；App.tsx 头注释补 `.tsx` 后缀。
- **R5 ✓** transport.ts:219 `<inviterId>`；p2p-protocol.md 邀请/回执两行 `<inviterId>`；test.ts 夹具 `u-inviter01`。grep 复核无残留（ICE host 候选按拍板保留）。
- **R6 ✓** 注释改「低段 2118184960 个值出现 2 次、其余 1 次，单值概率绝对差 ~2.3e-10」。复核数字：2118184960 = 2^32 − 36^6 ✓；1/2^32 ≈ 2.33e-10 ✓。比我报告的 ~1.36% 口径（受影响值占比）更精确，正确。

### R2 残留（同类竞态守卫不彻底，均为极窄窗口，低严重度）

- **R2a（低）** `useGameSession.tsx:539` 守卫只查 `phase/role`：`if (phaseRef.current !== "waiting" || roleRef.current !== "inviter") return null;`。若 await 期间用户**取消并在续体执行前开了新局**（新局同样 waiting+inviter），守卫被新局骗过——旧回执续体将 join 旧 gameId、置 playing、清掉新局 pwd。createInvite 守卫（:422-425）已用 `pwdRef.current !== p` 三重校验，acceptReceipt 缺对称的 `pwdRef.current !== r.pwd`。**修：一行补 pwd 对比即可**（入口已校验 r.pwd==pwdRef，await 后它只可能被 backHome 清 null 或换新局，两种都该拒）。
- **R2b（低）** createInvite 的 **catch 分支无守卫**：offer 生成失败（典型：CompressionStream 缺失 + waitGathering 走满 8s 超时）时，await 间隙取消后 catch 仍会写 `setInviteUrl(无rtc)` + `setDirectState("error")` + showNotice(「已生成同源链接…」)——写到主页/新局上的脏 state 与误导 notice。成功路径已有守卫，catch 路径漏对称处理。**修：catch 内加同款三重校验（含 pwdRef.current !== p），不满足则静默放弃。**

两处合计约 6 行改动，建议随下一轮顺手修；不阻塞当前状态。

### 验证链（本审查实测）

tsc 0 错、vitest 30/30、cargo test 26/26、vite build 成功（258.54 kB）。浏览器 E2E 未复跑（原窗口已验证 R2 跨设备回执全流程）。
