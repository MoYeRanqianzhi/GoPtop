# 2026-09-15 · 传输层 Rust 化第二阶段落地 + 五项遗留全清

## 落地内容（用户指令「完整进行 Rust 化，以上全部需要完成」）

1. **围棋劫争+终局数目**（T1，commit 13 files）：简单劫（ko_point 随重放恢复）、
   双 Pass 终局计分态、中国规则区域法（`score_area` 贴 7.5）、死子标记+双确认计分；
   顺带修复 **9/13 路物理盘外假气 bug**（BFS 气计算/计分空域限定逻辑区域）；
   adopt 改为历史全量重放重建（captures/ko/scoring 权威恢复，矛盾整体拒绝）。
2. **服务器 P3 快修**（T2）：循环内 hello 不再 unreachable panic（release abort 全服崩）、
   payload 改序列化字节上限（signal 64KB/relay 16KB）、注释对齐。
3. **goptop-net 纯逻辑 crate**（T3）：protocol（serde 对齐 TS 线格式）/dedup/identity
   （随机源注入）/codec（G1 与浏览器互通向量锁定）/links（手写 URL 切分——url crate
   的 RFC 归一化破坏 %2F 往返）/session 状态机（Elm 风格 reduce，33 单测）。
   **协商弹窗队列**（数据化 ConfirmAction，未决不再覆盖）、**无服务器跨设备观战回执**
   （specrtc 链接 → 观战回执 → 受理后 spec-pending 改名 live 并自动换新链接）都在状态机内。
4. **goptop-transport wasm crate**（T4）：web-sys 封装 WS/BC/RTC/storage/crypto，
   通道泵模型（IO 回调只入队，50ms 泵 drain → reduce → 执行 Effect）。
5. **前端接线**（T5）：useGameSession 重写为快照驱动壳（返回形状不变，pages 零改动）；
   TS 传输逻辑层删除（rtc/gameChannel/serverChannel/presence/chat/negotiation/serverSignaling）。

## 联调期抓出的深层 bug（E2E 全链跑通才暴露）

- **FeedOffer 缺失**：server_accept_offer 只建 PC 没喂 offer——B 永不生成 answer。
- **RenamePeer 缺失**：join 受理后状态机槽位改名 main→对端 ID，但 bridge 句柄键没同步
  → A 端 AcceptAnswer 找不到连接 → remote 候选恒空 → ICE checking 卡死。
- **ondatachannel 偶发丢失**：B 端 SRD 后事件派发竞争 → 改**双端 negotiated DC（id=0）**
  （经典游戏联机解，彻底消除事件依赖）。
- **观战数据面断裂**：观战者与受理方直连、与对手无链路——对手落子经受理方 relay 镜像
  转发给名下观战者（去重表兜底二次应用）。
- **pending_specs 匹配被 spec- 前缀分支遮蔽**：服务器模式观战 tag=对端 ID，spec-offer
  永不发出。
- **spec-kicked 通知被吞**：Notice 先发、do_back_home 的 Notice(None) 后发覆盖——
  通知类 effect 必须放在状态清理之后。
- **headless E2E 的 mDNS 候选**：跨 context .local 解析慢导致直连 20s 内不稳——E2E
  窗口放大到 45s；这是环境特性不是代码缺陷（历史版本同环境同样慢）。

## 验证基线（全部全绿）

- cargo test：goptop-net 33 + goptop-core 49（wasm feature）+ goptop-server 4
- tsc / vitest 34 / build / **E2E 42/42**（run.js 服务器模式全链：直连/落子/聊天/
  悔棋/换棋/重开协商/观战/发言批准/踢出/大厅挑战×2）

## 跨设备真机测试（2026-09-15，router/remote/free28 + 本机）

- **重大 bug 修复**：codec 手动缓冲循环在「解压输出恰好写满缓冲」边界对
  ≥243 字符 offer 的 token 全部 Inflate 失败（**邀请/观战链接跨设备全挂的根因**，
  本机同源 E2E 因候选交换路径不同未暴露）——改 flate2 读写封装彻底修复。
- **无服务器观战双密钥 bug**：内层 offer 用对局 pwd 编码、观众只有 specPwd——
  砍掉内层加密改**单密钥设计**（specrtc 直载明文 offer JSON，外层 specPwd 唯一保护）。
- 测试结果（本机 ↔ remote 美国 RackNerd，两端 headless chromium）：
  1. 服务器模式对局 ✓（官服 WSS 信令、跨设备加入、落子同步；直连因跨国 UDP 断开、
     relay 兜底工作——A2 已知限制复现）
  2. 跨设备观战 ✓（remote C 以 spectator 接入本机对局）
  3. 无服务器回执闭环 ✓（链接→回执生成→受理→连接建立全流程，直连成败取决于运营商）
- 部署：dist tar → router ~/web:8000（阿里云安全组未开 8000 入站，仅内网用）/
  remote /root/goptop/frontend/dist（python SPA :5175，node=/root/node/bin，
  playwright 依赖=/root/pw）；free28 官服 goptop-server active、health ok。
- 测试脚本入库：scripts/e2e/xdev-a.js（本机 A）+ xdev-b.js（remote B/C）。

## 遗留

- 本地对局仍走 rules.ts（goptop-core wasm 绑定薄壳）——统一走 WasmSession 后删除。
- 官服部署未做（本次服务器协议零改动，旧版服务器兼容新版客户端——纯转发）。
- 协商队列 E2E 未单独覆盖（单测覆盖队列行为）。

## 关联

- [[2026-09-15-rustification-direction-wasm-websys]]
- [[2026-09-15-serverless-spectator-receipt]]
- [[2026-09-12-userid-link-negotiation-chat-spectator]]
