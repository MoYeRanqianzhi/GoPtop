# 2026-09-10 P2P 跨网实测：STUN-only 直连失败根因与 TURN 方向

## 实测矩阵（本机=移动宽带 cone NAT 117.144.78.29 / router=阿里云杭州 121.41.236.136 / remote=RackNerd 美国 198.12.108.71）

| 测试 | 结果 |
|---|---|
| STUN 三条线路（miwifi/bilibili/google）三地可达性 | 全部存活，映射正常 |
| 本机 NAT 类型 | cone（三 STUN 同 socket 映射端口一致 4820，稳定） |
| 同机跨源全流程（localhost:5173 vs 127.0.0.1:5174） | ✓ 全通：offer/回执信令、ICE、DataChannel、落子同步 |
| T2 router↔remote（跨洲公网↔公网） | ICE check 双向通（98 包）、pair succeeded，**DTLS 卡 connecting**（疑阿里云安全组丢 UDP 分片：STUN 小包活、DTLS 证书大包死） |
| T1 本机↔remote（NAT↔公网） | ICE failed。remote 网卡证实 Out 包发出但本机收不到任何回包 → **中国移动丢弃国际来向 UDP**（同机 TCP/SSH 双向正常） |

## 代码层结论
- transport.ts DirectRtcPeer + useGameSession 编排无 bug：信令编解码、URL 候选传输（decode 实测 offer 含 2×mDNS host + 1×srflx）、waitGathering→localDescription 候选齐全。
- 同机跨源失败复现不了、跨网失败的根因全在网络层。之前「无法连接」= 真实网络下 STUN-only 直连成功率问题，不是代码缺陷。

## 关键网络事实（测试环境，复测时需知）
- remote（RackNerd 美国）→ 移动宽带的 UDP 出站：网卡发出但对端收不到（移动国际来向清洗）；→ 阿里云方向正常。
- router（阿里云杭州）：安全组未放 UDP 入站（实例外，CLI 改不了）；本机 iptables 全开；80/443 TCP 放行、nginx 跑着 a.meowoo.org（勿动）。
- remote 本地 iptables：PUBLIC-INGRESS-GUARD 链，测试时已插入 TCP 8000/8001 放行（测试后可移除）；UDP 入站本来全放。
- 测试部署：两台服务器 ~/web（dist 静态 + spa.py SPA fallback，端口 8000；remote 另有 8001 作第二源）；~/pw/bot.js（playwright chromium + HTTP 控制 9310：/invite /accept /receipt /status /ice）。
- 本机系统代理会把美国 IP 的 HTTP 交给代理节点导致 ERR_HTTP_RESPONSE_CODE_FAILURE——本机浏览器测试时用 localhost 源绕开。

## TURN 方向 → 最终拍板（2026-09-11）：自建信令服务器模式（已落地）
用户拍板弃 Cloudflare TURN 路线，改为 RustDesk 式自建信令服务器（更完整）：
- 新增 crates/goptop-server（axum+WS）：名册/短码邀请/短码观战/SDP 转发/relay 兜底；
  官服 wss://goptopserver.meowoo.org/ws → free28（192.228.105.26，1Panel openresty 反代，
  acme.sh 自动续期证书，systemd 常驻 127.0.0.1:9527）
- 前端 serverChannel.ts + 双模式编排：无服务器（原样保留）/服务器（单选切换，默认官服）
- STUN 三线路改九条免费池；设置页两区块；/j /s 短码链接；免回执
- 关键坑：服务器空闲超时必须远大于客户端心跳（曾 10s<25s 导致连接被误杀、短码失效）
- 部署细节：free28 的 DNS 由用户在 Cloudflare 加（橙色云代理）；openresty conf 在
  /opt/1panel/www/conf.d/；acme.sh webroot=/opt/1panel/www/sites/goptopserver.meowoo.org/acme
