# .agents/MEMORY.md — 共享关键记忆索引

- [2026-09-18 全维度实机测试轮](memory/2026-09-18-multi-device-real-test-round.md) — 五端两两对战全通；修 7 个产品缺陷（鸿蒙链接/paste 走错通道/观战链接缺失/spec 丢失/乱序丢手/围棋终局 UI 缺失/认输判错）；测试环境搭建要点（安卓禁 connectOverCDP、桌面壳须带 custom-protocol）；布局正反馈塌陷与窄屏横幅遮挡两个陷阱
- **[归档记忆]** 2026-09-07 全量代码审查 → 两轮修复 → 修复轮审查 → R 级修复 → 复审，全循环闭环（A1-A8/B/C/D + R1-R6 落地；残留 R2a/R2b 低危守卫补漏跟踪于 TODO.md）。审查报告已移至 `review/archive/`（2026-09-07-main-agent-code-review.md、2026-09-07-fix-round-review.md），文件头有归档标注，仅作历史记录勿回填。
- [2026-09-07 P2P 对等术语拍板](memory/2026-09-07-p2p-peer-terminology-inviter-invitee.md) — 房主/客人全面废弃，改邀请者/受邀者（inviter/invitee）；ICE host 候选是协议词汇除外
- [2026-09-07 P2P 方向拍板](memory/2026-09-07-p2p-direction-a-no-server-auto-share-origin.md) — A 路线纯无服务器（回执保留）；分享域名必须全自动（代码常量），禁止做成用户设置项
- [2026-09-07 回执与观战设计](memory/2026-09-07-receipt-ui-and-spectator-design.md) — 同浏览器双窗口是极端情况不考虑；回执按钮常驻、确认文案去「开局」、回执类型按链接属性自动识别；观战者只与邀请者直连；此前开发无记忆是违规，今后拍板必落记忆
- [2026-09-07 禁多代理并发](memory/2026-09-07-no-parallel-agents-single-serial.md) — 并发 Workflow 打爆 API 中断；审查/修复一律单代理串行，禁 parallel() 扇出
- [2026-09-10 P2P 跨网实测](memory/2026-09-10-p2p-crossnet-stun-turn.md) — 代码无 bug；失败根因=移动丢国际来向 UDP+阿里云安全组；STUN-only 天花板已实测，TURN 兜底方向待拍板；三台测试机部署位置与网络特性
- [2026-09-12 userId 链接+协商+聊天+观战房间](memory/2026-09-12-userid-link-negotiation-chat-spectator.md) — 链接回归 /userId?pwd=；服务器纯转发不落地；协商消息必须三链路去重；名册 ID=客户端持久 userId
- [2026-09-15 多代理禁令恢复](memory/2026-09-07-no-parallel-agents-single-serial.md) — 用户拍板禁令继续，单代理串行为现行规则
- [2026-09-15 传输层 Rust 化方向](memory/2026-09-15-rustification-direction-wasm-websys.md) — wasm-bindgen+web-sys 否决原生桥接（四端全 WebView）；goptop-net 纯逻辑 + goptop-transport wasm；TS 只剩 UI
- [2026-09-15 无服务器跨设备观战设计](memory/2026-09-15-serverless-spectator-receipt.md) — specrtc 链接 + 观战回执 + 受理后自动换新链接；offer 单次消费语义
- [2026-09-15 Rust 化第二阶段落地](memory/2026-09-15-rustification-phase2-landed.md) — 五项遗留全清；联调期六大深层 bug（FeedOffer/RenamePeer/negotiated DC/观战镜像转发/通知顺序/mDNS 环境）；验证基线全绿
- [2026-09-13 大厅挑战修复+邀请弹窗](memory/2026-09-13-lobby-challenge-modal.md) — challenge-accepted 守卫错位+/users 走错信令两根因；邀请统一居中弹窗不设背景关闭；E2E 39 断言；跨设备实测（本机↔美国官服）挑战→弹窗→对局→黑胜全通且 WebRTC 直连成功
- [2026-09-13 整理轮：三层拆分+规则下沉 wasm](memory/2026-09-13-cleanup-refactor-round.md) — 架构要求重申（Rust 承接一切，TS 只 UI）；wasm 规则接线（pickKind 中间帧 trap 坑）；B1-B4 根因；E2E 断言依赖 UI 文案面的教训；多代理禁令解除
- [2026-09-14 四端实机验证+真 Windows 标题栏+六维审查](memory/2026-09-14-four-platform-testbar-review-round.md) — HTMAXBUTTON 覆盖层真 Snap Layouts（点击/悬停必须回传页面；恒返回非客户区命中码 ⇒ 只收 WM_NCMOUSEMOVE/NCMOUSELEAVE，须配 TME_NONCLIENT，2026-09-19 补修）；Android `__TAURI_INTERNALS__` 非 document-start 注入坑；HarmonyOS ArkWeb 壳（未签名 HAP 可装模拟器+CDP 驱动）；SyncState.history 必须可表达 Pass；E2E 入库 scripts/e2e/；官服部署程序
- [2026-09-19 聊天停靠栏三档布局](memory/2026-09-19-chat-dock-three-tier-layout.md) — 等宽(棋盘列宽) → 压缩聊天 → 弹窗；形态全实测（删 1080px 断点）；`.play-stack` 必须 `flex: 0 0 auto`，否则「停靠栏挤窄整组→实测把挤窄值当基准」锁死棋盘；run.js 49 断言
