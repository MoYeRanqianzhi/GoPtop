# 2026-09-13/14 审查轮（六维串行）——发现与核实台账

> 规则：审查代理只读并报告；主代理逐条核实后修复。本文是唯一台账，
> 每条 finding 的状态在此推进：`待核实` → `已核实已修(commit)` / `已核实不修(理由)` / `待修`。
> 代理报告原文见各任务输出，关键证据已摘录进对应条目。

## 代理分工与状态

| # | 维度 | 文件范围 | 状态 |
|---|------|----------|------|
| 1 | state/时序/竞态 | state/useGameSession、negotiation、chat | 完成 |
| 2 | 服务器信令 | state/serverSignaling、serverChannel、goptop-server | 完成 |
| 3 | RTC/链接/编解码 | net/rtc、links、stun、identity、protocol、gameChannel | 完成 |
| 4 | UI/React 组件层 | pages/、components/、App.tsx、styles | 完成 |
| 5 | Rust/wasm 边界 | crates/goptop-core、game/rules.ts、src-tauri | 完成 |
| 6 | 文档/测试覆盖 | docs/、.agents/docs/、测试文件 | 待派 |

基线：vitest 32/32（本轮新增 2 条 Chat 去重回归后），E2E 42/42（改动后需复跑）。

## #1 state/时序（P1×1 P2×5 P3×6）

| 级别 | 发现 | 状态 |
|------|------|------|
| P1 | 观战者可批准协商：UndoReq/ResetReq/SwapReq 三 case 只守 phase 不守 role | **已修**：三 case 加 `roleRef.current === "spectator"` 守卫 |
| P2 | Chat 同源双链路（BC+RTC）必重复（DEDUP 缺 Chat，pushChat 无去重） | **已修**：Chat 入 DEDUP + 2 条回归测试（#3 A2 同源） |
| P2 | myAvatarRef 不从 localStorage 回填：刷新后直连建立不发头像 | **已修**：挂载 effect 回填 |
| P2 | serverAdmitChallenger/serverAcceptOffer await 间隙无重入守卫 | 待修（触发窗口窄，与 R2 守卫同模式可套） |
| P2 | 服务器模式头像键位漂移：Avatar 只走 P2P 键 u-，聊天行（服务器 relay）用 s- 查 | 待修（需 Avatar 走 relay 的设计取舍：20KB dataURL 经服务器） |
| P2 | resetBoardFor 不 bump sv | **核实不修**：调用点均在 closeAllRtcPeers 之后（换局/开局），旧同步无碰撞面；空局 (sv,0) 与重置后状态相同 |
| P3 | invitee 取消后 inviter 卡 waiting（无撤销通知） | 待修 |
| P3 | closeAllRtcPeers 不清 specRequests/spectators/myHostRef | 待核实（配对字段是 server 域，跨域清理需防误伤） |
| P3 | createInvite 服务器连通性检查在状态写入之后 | **已修**：检查前移至任何写入前 |
| P3 | createInvite 不清已开的 invite modal | 待修（小） |
| P3 | resolvePeerName 闭包冻结空 chatLog | 待核实 |
| P3 | Pass 落子不查 winner（终局后仍可 Pass 改手数） | 待修（小） |

## #2 服务器信令（P1×1 P2×5 P3×8）

| 级别 | 发现 | 状态 |
|------|------|------|
| P1 | 同 ID 顶替：旧连接断开清理无条件 `peers.remove(user_id)`，删掉新连接注册 → 在线却不可达 | **已修**：清理加 `p.tx.same_channel(&tx)` 判定；顶替 error 加 `code:"taken-over"`，客户端收到后主动断开（manualClose 防乒乓），onclose 不覆盖 error 态 |
| P2 | spec-chat/spec-chat-req 用 `find(id!==from)` 找转发目标而非 opponentRef | 待修 |
| P2 | spec-sync 的 enabled 只写 state 不写 ref：另一 host 的「关闭观战」被本端下次 pushSpecSync 覆盖回去 | **已修**：ref+state 双写 |
| P2 | join 无互斥：两受邀者同 join，第二个卡死 | 待修（accept 应拒绝已有对手的 join） |
| P2 | host 退出（backHome）不散场观战者：只断 RTC，观战者停留在观战中 | **已修**：backHome 对 host===myServerId 的观战者先发 spec-kicked（role 守卫防观战者自踢） |
| P2 | 服务器断连期间仍可 relay 落子（handlePlace/linkLamp/boardDisabled 不查 serverChannel.connected） | 待评估：直连可用时强制查 connected 反而错误——应查「direct 或 relay 至少一路可用」 |
| P3 | 双方互发挑战（challenge 对撞）双重 admit 死锁，需确定性 tiebreak | 待修 |
| P3 | 第二条 hello `unreachable!()` panic（main.rs） | 待修（小） |
| P3 | muted 字段无人强制（禁言纯摆设） | 待修 |
| P3 | 128/64 字段限制只数顶层键，无字节上限 | 待修（小） |
| P3 | confirmReq 单槽：未决时新协商覆盖旧的 | 已知限制（TODO「协商弹窗队列」候选），不改 |
| P3 | spec-sync 不转发给本端自己的观战者；meKicked 死防御 | 待修/删 |
| P3 | 注释失真：s- 短 ID 注释（hello 实际总带 userId）、pushSpecSync「全部观战者」等 | 待修 |

## #3 RTC/链接/编解码（P2×3 P3×4 + 结构 B1-B8）

| 级别 | 发现 | 状态 |
|------|------|------|
| A1 P2 | 服务器模式 join 分支对同一 peer 二次 prepareOffer：close() 的 "closed" 把 peer 永久出列，先到受邀者的 answer 永久丢失 | 待修（join 时若 offer 已存在则复用现成描述重发，或拒绝第二个 join） |
| A2 P2 | 同源对局聊天必重复 | **已修**（同 #1，Chat 入 DEDUP） |
| A3 P2 | waitGathering 8s 超时无用户可见失败路径：候选不全时两端静默挂死 | 待修（返回是否按时完成；open 前 30s 显式失败提示） |
| A4 P3 | parsePastedLink 对 `?u=` 双重 decodeURIComponent，ID 含 % 抛 URIError 被吞成「无法识别」 | **已修**：删 :179 二次 decode + 回归测试（links.test.ts） |
| A5 P3 | 受邀者侧坏 offer 报错文案误导为线路问题 | 待修（decode 失败单独文案） |
| A6 P3 | 自定义 STUN 行无格式校验（待验证 RTCPeerConnection 构造是否抛） | 待验证后修 |
| A7 P3 | loadStunLines「空数组回默认」+「内置项无条件补回」：删除/关闭语义不可持久化 | 记录（UI 不可达，纯潜在） |
| B1 | DirectRtcPeer.all 只写不读 | 待修（整体删除） |
| B2 | GameMsg Ping/Pong 死协议 | 待修（删除或注明保留原因） |
| B3 | homeUrl 死导出（且 origin 口径与 shareOrigin 不一致） | 待修（删） |
| B4 | rtc.ts 头注释「绝不重复应用」失真 | **已修**（随 Chat 入 DEDUP 一并改写注释） |
| B5 | links 解析逻辑三份拷贝（parseUrl/parsePastedLink/parsePastedAnswer），漂移已发生（A4） | 记录（重构候选，收敛为单一解析核心） |
| B6 | rtc.ts:50 「fnv1a 双散列」实际 h2 是 murmur 常数 | 待修（注释） |
| B7 | stun.ts:61「全关则纯 host 候选」口径不含「删除」语义 | 待修（注释） |
| B8 | attachPeer「每 peer 恰 attach 一次」约定无固化 | 待修（注释） |

## #4 UI/React 组件层（P1×2 P2×3 P3×14）

| 级别 | 发现 | 状态 |
|------|------|------|
| P1-1 | IME composition 未隔离：中文输入法按 Enter 确认候选词会把未上屏文本当消息发出（ChatPanel components.tsx:189、PasteModal.tsx:65，全仓无 isComposing 处理） | **已修**：两处 onKeyDown 加 `!e.nativeEvent.isComposing` 守卫 |
| P1-2 | 上传/清除头像后预览不刷新：UserPage 读 localStorage 渲染，saveMyAvatar 无 setState，点保存/清除看似无效 | **已修**：UserPage 预览改 useState 驱动，onSave 回写 |
| P2-1 | UserPage 的重开按钮在非服务器 P2P 绕过对方同意制（reset() 注释称 P2P 已同意制但实现只覆盖服务器模式） | 待修（与 #1「unilateral Reset」同源：协议 Reset 消息仍是活路径） |
| P2-2 | chatLog 跨局残留 + chatOpen 跨局自动弹开（chat.ts 只追加不清，App 无复位） | 待修 |
| P2-3 | 双 ChatPanel 实例并存（dock+modal 同时 mount，草稿/滚动独立；App.tsx:127 死条件） | 待修（按断点只渲染其一） |
| P3-1 | BoardPanel history prop 死属性，4 页面白传 | 待修（删） |
| P3-2 | 观战者显示「执白」（myColor 占位值泄漏到文案） | 待修（观战显示「观战」） |
| P3-3 | 胜利横幅「点击重开」与 P2P/观战页无重开按钮脱节 | 待修（文案按模式） |
| P3-4 | NoticeLine 一律绿色：错误文案也走成功色 | 待修（语义分级或记录） |
| P3-5 | 观战申请文案「钥匙不正确」硬编码失真 | 待修 |
| P3-6 | InviteModal/PasteModal 同 zIndex 盖序；ConfirmBanner(50) 被弹窗盖 | 待修 |
| P3-7 | ConfirmBanner 出现挤压 play-stack 致棋盘宽度跳变（flex 可收缩） | 待修 |
| P3-8 | LocalPage/WatchPage 内层重复 .play-stack（D1 拆分漂移） | 待修 |
| P3-9 | loadDefaults 不校验 kind/size 组合（残留 gomoku+19 时星位失真） | 待修（原子修正同 pickKind） |
| P3-10 | UsersPage 非服务器模式挑战按钮可点无反馈 | 待修 |
| P3-11 | UserPage waiting 态漏分支：invitee 等待/offer 生成中显示「开启对战」可静默放弃当前等待 | 待修 |
| P3-12 | 悬停橙圈在 boardDisabled 期不清除（onPointerMove 早退不清 hover） | 待修 |
| P3-13 | 可访问性集合：BoardSvg role=grid、弹窗无 dialog/aria-modal/Esc、ConfirmBanner 无 aria-live、KindSizePicker locked 可开面板 | 待修（部分） |
| P3-14 | 横屏左右 safe-area 缺失；favicon 仍 Vite 默认 | 待修（小） |
| 跨界注记 | submitModal paste-invite 分支忽略 it.spec（观战链接会按挑战发出）；boardDisabled useMemo 依赖 ref 的隐性耦合 | 供 state 侧修复轮参考 |

查证不成立：LocalPage undo 索引、App --stack-max effect、切尺寸越界防护、PosterStrip StrictMode、boardDisabled 消费面、chat key、P2pPage specUrl 回退。

## 测试缺口（#3 提出，#4/#6 可能补充）

## #5 Rust/wasm 边界（P1×2 条件触发 P2×3 P3×10；titlebar/契约核对全过）

| 级别 | 发现 | 状态 |
|------|------|------|
| P1-1 | Pass 消息与 wasm 引擎脱钩：无 pass() 绑定，收到 Move{Pass} 只翻 TS toMove，引擎 to_move 陈旧 → 有对端发 Pass 后落子颜色/轮转错乱（现仓库无发送入口，条件触发） | **已修**：wasm.rs 加 pass() 绑定，useGameSession Pass 分支走引擎并记入 history |
| P1-2 | adopt/undo 历史契约不含 Pass：SyncState.history: Coord[] 结构上不可表达 Pass，含 Pass 对局 adopt 后 undo 必然轮转漂移（#1 假设证实） | **已修**：protocol.ts history 升级 (Coord\|"pass")[]，adopt 按 untagged 枚举重建 Move |
| P2-1 | rules.ts adopt 忽略 boolean 返回值：远端 SyncState 非法（维度/toMove=empty）时引擎拒绝而 TS 照收，两边分叉无提示 | **已修**：adopt 检查返回值并 console.warn 留痕 |
| P2-2 | undo_last 重放 `let _ = fresh.try_play(mv)` 静默丢弃错误：adopt 未校验坐标边界，越界重放带病回退 | **已修**：adopt 校验坐标范围；undo 改为重建成功才替换状态，失败报 replay_failed |
| P2-3 | GameState::new 的 assert 是唯一 trap 防线且 panic=abort 直接白屏，TS 守卫与 trap 间只隔远端可控 size | **已修**：GameKind::is_valid() 提取，wasm new_game 预验证返回 None |
| P3-1 | try_place u8 参数越界静默截断（现调用方已钳制，防御注记） | 记录 |
| P3-2 | adopt 接受 winner:"empty"（to_move 有守卫 winner 没有） | **已修**：winner 白名单 black/white/null |
| P3-3 | captures 不可观测且 adopt 清零（UI 未显示提子，未来地雷） | 记录 |
| P3-4 | WasmGame 无显式 free()，靠 FinalizationRegistry 兜底 | 记录 |
| P3-5 | titlebar.rs 五项专项核对（类注册/HWND 残留/TrackMouseEvent/scale/单位与事件名）——**全部通过** | 无需行动 |
| P3-6 | capabilities/命令/事件与前端完全一致 | 无需行动 |
| P3-7 | ko 与双 Pass 终局缺失（go.rs 已声明；Pass 现仅翻轮手） | 已知限制，随 P1-1 修复考虑 |
| P3-8 | protocol.rs:3 注释指向不存在的 transport.ts（真源 net/protocol.ts） | **已修**：注释改为 protocol.ts |
| P3-9 | tauri.conf.json CSP null，建议收紧 | 记录（桌面建议） |
| P3-10 | board_json 每手 361 次 format!、BFS 每手建 HashSet——非病理性，无需行动 | 无需行动 |
| 契约核对 | 棋盘 JSON/ok_reply/Coord/kind 标签/.d.ts 签名与 TS 侧**全部一致**；Rust protocol.rs 未接线不构成漂移 | 无需行动 |

## #6 文档/测试覆盖（文档失真 15 + 缺失 6 + 测试缺口 12）

| 级别 | 发现 | 状态 |
|------|------|------|
| A1 | docs/使用指南.md:100「跨设备观战尚未实现」与已知限制 A1 矛盾——服务器观战已全链可用 | 待修 |
| A2-A6 | 使用指南：P2P 描述漏信令服务器可选；STUN 表 3 行旧名（实际 9 条内置）；4.4 编号错位；/users 跨设备说明；设置表缺信令服务器/头像行 | 待修 |
| A7 | docs/部署指南.md:57 引用已删除的 transport.ts（真源 links.ts:154） | 待修 |
| A8 | 部署指南「不需要任何后端」与可选官服矛盾，缺自建服务器部署章节 | 待修 |
| A9 | p2p-protocol.md:58 DEDUP 列表含不存在的 Aware，实际集合见 gameChannel.ts:83 | 待修 |
| A10-A12 | architecture.md：vitest 30→33、行数标注过期、components 列表缺 BrutalCard/PosterStrip | 待修 |
| A13 | p2p-protocol.md:113 S2C error 漏 `code:"taken-over"` 语义 | 待修 |
| A14-A15 | DOCS.md 索引未覆盖根 review/；cleanup-audit.md 指向空目录 .agents/plan/ | 待修 |
| 注释 | src-tauri/lib.rs p2p/iroh 注释与 p2p.rs「空模块」矛盾；goptop-server main.rs:10 kind 列表含已删的 ice | 待修 |
| B1 | **E2E/浏览器验证脚本未版本化**（run.js 42 断言/go-capture/ui-audit 只在本机 %TEMP%），新 clone 无法复现回归基线 | 待修（入 scripts/e2e/ + 说明） |
| B2-B6 | 自建信令服务器部署文档、服务器模式/聊天/换棋/头像人类文档、STUN 迁移记录、版本口径 | 待修 |
| C1-C9 | TS 测试缺口：links.parseUrl+5 构造函数、(sv,len) 双键守卫、DirectRtcPeer 状态机、rtcCodec 异常、协商类去重、stun/servers/identity 零测试 | 待补 |
| C10 | wasm.rs 零测试：10 条用例清单（undo 提子还原/空历史/adopt 维度/颜色/错误码契约/game_over/Resign/多子块提） | 待补 |
| C11 | goptop-server 零测试：is_valid_peer_id/sanitize_name/限速窗口 | 待补 |
| C12 | state 层可下沉纯函数：loadDefaults/resolvePeerName/serverSignaling 假通道驱动 | 记录 |
| D | 一致项：README 命令面、STUN 9 条、已知限制 A2-A4/B1/C2、architecture 时序数字、p2p-protocol URL 六格式/G1 编码链/限速参数、build-wasm.sh、DOCS.md memory 索引 | 无需行动 |


- rtcCodec：空 pwd 往返、atob 异常路径、`length%4==1` 截断、非 ASCII、缺 s/t 字段。
- links：parseUrl 完全无测试（/watch 单段、多段、畸形 %）；五个 URL 构造函数零测试；`?u=` 双解码回归。
- gameChannel：Avatar 去重、join/leave 重建 BC。
- DirectRtcPeer 状态机零测试（close→closed、answered 幂等、re-prepare 语义）。

## 本轮已落地修复的验证

- `npx tsc --noEmit` 通过；vitest 32/32（含新增 Chat 去重 2 条）。
- `cargo check -p goptop-server` 通过。
- 服务器改动（same_channel/taken-over）需 E2E 复跑 + 部署官服后回归。
