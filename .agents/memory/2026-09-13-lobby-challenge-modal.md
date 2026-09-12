# 2026-09-13 · 大厅挑战修复与邀请弹窗拍板

## 用户报障与拍板
1. Bug：对战中/主页从用户列表挑战，对方同意后双方都不自动进对局页。
2. 拍板：对局邀请改成**居中弹窗**（原 play-stack 顶部横幅不够明显）。

## 根因（两处，均为代码 bug）
- **challenge-accepted 守卫错位**：挑战者发 challenge 时处于 home（serverChallengePeer
  仅主页可发起且不改本端状态），同意信到达时仍是 home；旧守卫要求 waiting+inviter——
  该状态在本流程不可达 → serverAdmitChallenger（建局+送 offer）被静默跳过，双方停在原地。
- **/users 页挑战走错信令**：onAction 恒用 acceptInvite（presence.challenge，仅同源
  BroadcastChannel 有效），服务器模式下跨设备永远发不到；改 serverMode ? serverChallengePeer。

## 修复要点
- 守卫改为 phase=home，保留 waiting+inviter 分支（「先挑战又点开启对战」的边缘顺序下
  被接受的挑战优先成局，兼容旧守卫意图）。
- 邀请弹窗：serverIncoming（服务器挑战）与 incoming（同源 presence 挑战）统一一档
  inviteReq，fixed 覆盖层 z1000；**不设背景点击关闭**——静默忽略会让挑战方停在
  「等待对方同意」。只在 home 出现，不会打断对局中用户（对局中收挑战由服务端自动拒绝）。
- E2E 扩到 39 断言全过：11（/p2p 列表挑战全链：弹窗→同意→双方进对局→看到棋盘）
  + 12（/users 页挑战全链）；既有 1-10 无回归。

## E2E 脚本教训（%TEMP%/goptop-e2e/run.js）
- 列表行定位必须取含目标名字的「最小 div」；从按钮向上爬祖先会越过行边界撞进整张
  卡片，误点第一行可点挑战按钮（点了别人）。
- /users 页没有「服务器已连接」文案（只在 /p2p 主页显示），等名册里对方名字出现才算就绪。
- 本机 shell 里 node 必须用完整路径 node.exe（裸 node 报 stdin is not a tty）。

## 范围外（已知未修，待用户指令）
- 用户主页 /<userId> 的无 pwd「挑战」按钮仍走 acceptInvite（服务器模式跨设备不可达）；
  pwd 链接的自动 join 流不受影响。

## 跨设备真实对战实测（同日晚，本机 ↔ ssh remote=RackNerd 美国，官服信令）
用户拍板：测试必须每步截图人工核验、完全模拟人类操作（真实鼠标事件，非合成事件）。
结果 **全链路通过**：列表挑战（真实点击）→ 弹窗同意 → 双方自动进对局页 →
9 手真实落子逐手双侧同步 → 黑五连胜判定一致。双方状态均为「已直连」——
**本次 WebRTC 直连成功**（与 2026-09-10「移动丢国际来向 UDP」结论不同，
ICE 打通与否存在网络路径随机性；服务器 relay 兜底仍是必要保险）。

### 测试基建（challenge-server.js 拟人化控制服务，本机 %TEMP%/goptop-e2e 与 remote ~/pw 各一份）
- 常驻 headless 浏览器 + HTTP 端点：/open /status /shot(截图) /challenge /accept /place(/eval 调试)；
  外部 curl 逐步驱动，每步 /shot 截图人工回看；remote 截图经 `ssh remote "curl .../shot"` 取回。
- 落子坐标由 SVG viewBox 反推（pad=30，cell=(vb-60)/(size-1)），mouse.move+mouse.click 真实点击。
- 环境坑：remote node 在 /root/node/bin（非交互 ssh 不加载 .bashrc PATH）；本机 playwright
  必须显式 executablePath=chromium-1228；**git-bash curl -d 的中文按 GBK 发出**（JSON body
  乱码），必须 Write 工具写 UTF-8 文件再 `--data-binary @file`；remote 清进程用 ss 取 PID
  再 kill（pkill -f 模式会匹配自身 ssh 命令行导致 ssh 255）。
- remote 遗留：Sep10 起的旧 node bot.js 进程仍在跑（旧无服务器流程 bot，未动）。
