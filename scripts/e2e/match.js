/**
 * match.js — 跨端实机对局驱动（全端两两对战、真实点击、下到分出胜负）。
 *
 * 端规格（argv 里的 A/B/C 位置）：
 *   web              本机 chromium 桌面视口（http://localhost:5173）
 *   web:<url>        本机 chromium 桌面视口（指定地址）
 *   mob / mob:<url>  本机 chromium 手机仿真（Pixel 5，触摸）
 *   cdp:<url>        接管已运行 WebView（桌面壳 9222 / 安卓 adb forward / 鸿蒙 hdc fport）
 *
 * 用法：
 *   node match.js game  web cdp:http://127.0.0.1:9222            # 一盘下到分出胜负
 *   node match.js chat  web cdp:http://127.0.0.1:9222            # 聊天 + 三种协商全流程
 *   node match.js watch web cdp:http://127.0.0.1:9222 mob        # 含第三方观战
 *
 * 环境变量：
 *   XDEV_SERVER=none   用无服务器模式（默认走官方服务器，与产品默认一致）
 *   XDEV_HEADFUL=1     浏览器端点有头运行（人工旁看）
 */
const { Endpoint, playOneMove } = require("./device.js");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const HOME = "http://localhost:5173/";

/** 解析端规格 → Endpoint。 */
async function open(spec, name) {
  if (spec === "android" || spec.startsWith("android:")) {
    const serial = spec.includes(":") ? spec.slice(8) : null;
    const ep = await Endpoint.android(name, { serial });
    await prepShell(ep, name);
    return ep;
  }
  if (spec.startsWith("cdp:")) {
    const ep = await Endpoint.cdp(name, spec.slice(4));
    await prepShell(ep, name);
    return ep;
  }
  if (spec === "mob" || spec.startsWith("mob:")) {
    const url = spec.includes(":") ? spec.slice(4) : HOME;
    const ep = await Endpoint.browser(name, url, { mobile: true });
    await prep(ep);
    return ep;
  }
  if (spec === "web" || spec.startsWith("web:")) {
    const url = spec.includes(":") ? spec.slice(4) : HOME;
    const ep = await Endpoint.browser(name, url);
    await prep(ep);
    return ep;
  }
  throw new Error(`未知端规格: ${spec}`);
}

/** 壳端点（桌面/安卓/鸿蒙）：固定昵称与服务模式后重载挂载（等价用户先设好配置）。 */
async function prepShell(ep, name) {
  await ep.page.evaluate((mode) => {
    localStorage.setItem("goptop:name", mode.name);
    if (mode.server === "none") localStorage.setItem("goptop:server-sel", "none");
    else localStorage.removeItem("goptop:server-sel");
  }, { name, server: process.env.XDEV_SERVER === "none" ? "none" : "official" });
  await ep.page.reload({ waitUntil: "domcontentloaded" });
  await ep.ready(40000);
}

/** 浏览器端点首次进入：固定昵称与服务模式，再进 P2P 页（模拟用户先设好配置）。 */
async function prep(ep) {
  await ep.page.evaluate((mode) => {
    localStorage.setItem("goptop:name", mode.name);
    if (mode.server === "none") localStorage.setItem("goptop:server-sel", "none");
    else localStorage.removeItem("goptop:server-sel");
  }, { name: ep.name, server: process.env.XDEV_SERVER === "none" ? "none" : "official" });
  await ep.goto(HOME + "p2p");
  await ep.ready(40000);
}

/** 两端等到「同一手数」，确保上一步同步完成。 */
async function waitSync(A, B, timeout = 30000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    const [a, b] = [await A.snap(), await B.snap()];
    if (a.moveCount === b.moveCount && a.toMove === b.toMove && a.winner === b.winner) return a;
    await sleep(250);
  }
  throw new Error("两端手数未同步（数据面可能中断）");
}

/** 排队等两端都进入对局。 */
async function waitPlaying(A, B, timeout = 90000) {
  await A.waitSnap((s) => s.phase === "playing", timeout, `${A.name} 进入对局`);
  await B.waitSnap((s) => s.phase === "playing", timeout, `${B.name} 进入对局`);
}

/** A 开局、B 加入（浏览器走「打开链接」，壳走「粘贴链接」）。返回 A 的邀请链接。 */
async function pairUp(A, B, { serverMode = process.env.XDEV_SERVER !== "none" } = {}) {
  // 两端都进大厅并等信令就绪（开局有连通性前置检查）。
  for (const ep of [A, B]) { await ep.enterP2P(); await ep.waitServerReady(); }
  const link = await A.createInvite();
  if (!link) throw new Error(`${A.name} 未生成邀请链接`);
  console.log(`[配对] ${A.name} 邀请链接: ${link.slice(0, 90)}${link.length > 90 ? "…" : ""}`);
  if (B._browser && !B.page.url().startsWith("http://tauri") && !B.page.url().startsWith("https://appassets")) {
    // 浏览器端点：走「打开链接」这条真实用户路径。
    await B.goto(link);
  } else {
    // 壳端点：粘贴链接（产品内的正规入口）。
    await B.joinByPaste(link);
  }
  await waitPlaying(A, B);
  return link;
}

/**
 * 下到分出胜负：每手由「当前行棋方」用真实点击落子，直到 winner 出现。
 * 返胜者。maxMoves 兜底防死循环。
 */
/**
 * 步进 n 手真实落子（每手等两端同步）；中途出现胜者立即返回。
 * 返回 winner 或 null（走满 n 手仍未见胜负）。
 */
async function playMoves(A, B, n, { sync = true } = {}) {
  let played = 0;
  let guard = 0;
  while (played < n && guard++ < n * 40 + 80) {
    const s = await A.snap();
    if (s.winner) return s.winner;
    const aMoves = s.toMove === s.myColor;
    const mover = aMoves ? A : B;
    // 棋力按**行棋方的执色**分配（黑 sharp / 白 casual），不按端点：换棋会互换执色，
    // 按端点分配会让两边棋力随换棋漂移。该组合已离线模拟验证：黑 37 手取胜。
    // 注意 s 是 A 的快照，走 B 时要取反色，否则把 A 的档位套到 B 头上。
    const moverColor = aMoves ? s.myColor : (s.myColor === "black" ? "white" : "black");
    const level = moverColor === "black" ? "sharp" : "casual";
    const mv = await playOneMove(mover, level);
    if (!mv) { await sleep(250); continue; }
    if (process.env.MATCH_DEBUG === "1" && played % 10 === 0) {
      console.log(`  [dbg] 第${played + 1}手 ${mover.name} 执${moverColor} 档=${level} 盘面=${mv.x},${mv.y}`);
    }
    played++;
    if (sync) await waitSync(A, B, 40000);
  }
  return null;
}

/** 下到分出胜负。 */
async function playToVictory(A, B, opts = {}) {
  const { maxMoves = 260 } = opts;
  const w = await playMoves(A, B, maxMoves, opts);
  if (!w) throw new Error("达到最大手数仍未分出胜负");
  console.log(`[对局] 第 ${(await A.snap()).moveCount} 手分出胜负：${w} 胜`);
  return w;
}

/* ----------------------------- 场景 ----------------------------- */

/** 场景 1：一盘完整对局，真实点击下到胜利。 */
async function scenarioGame(A, B) {
  await pairUp(A, B);
  const w = await playToVictory(A, B);
  const [sa, sb] = [await A.snap(), await B.snap()];
  if (sa.winner !== sb.winner) throw new Error(`两端胜负不一致: ${sa.winner} vs ${sb.winner}`);
  console.log(`[结果] ${A.name} winner=${sa.winner} moves=${sa.moveCount} / ${B.name} winner=${sb.winner} moves=${sb.moveCount}`);
  return w;
}

/** 场景 2：聊天 + 三种协商（悔棋 / 换棋 / 重开）全流程。 */
async function scenarioChat(A, B) {
  await pairUp(A, B);
  // 先落两子（协商需要非空历史）。
  await playMoves(A, B, 2);

  // 聊天双向
  await A.sendChat("你好，我是" + A.name);
  await B.waitSnap((s) => s.chatLog.some((m) => m.text.includes(A.name)), 15000, "B 收到 A 的聊天");
  await B.sendChat("收到，我是" + B.name);
  await A.waitSnap((s) => s.chatLog.some((m) => m.text.includes(B.name)), 15000, "A 收到 B 的聊天");
  console.log("[聊天] 双向消息互通 ✓");

  // 悔棋：B 请求 → A 同意
  await B.negotiate("undo");
  await A.waitSnap((s) => s.confirmReq?.kind === "undo", 15000, "A 弹出悔棋确认");
  await A.approve();
  await waitSync(A, B);
  const undoTarget = (await B.snap()).moveCount;
  console.log(`[悔棋] 同意后手数 = ${undoTarget} ✓`);

  // 换棋：A 请求 → B 同意（黑白互换）
  const colorBefore = (await B.snap()).myColor;
  await A.negotiate("swap");
  await B.waitSnap((s) => s.confirmReq?.kind === "swap", 15000, "B 弹出换棋确认");
  await B.approve();
  await B.waitSnap((s) => s.myColor !== colorBefore, 20000, "换棋后颜色互换");
  console.log(`[换棋] ${B.name} ${colorBefore} → ${(await B.snap()).myColor} ✓`);
  await waitSync(A, B);

  // 重开：B 请求 → A 同意
  await B.negotiate("reset");
  await A.waitSnap((s) => s.confirmReq?.kind === "reset", 15000, "A 弹出重开确认");
  await A.approve();
  await waitSync(A, B);
  const after = await A.snap();
  if (after.moveCount !== 0) throw new Error(`重开后手数应为 0，实为 ${after.moveCount}`);
  console.log("[重开] 双方手数归零 ✓");

  // 重开后继续下到分出胜负（协商不破坏对局）
  const w = await playToVictory(A, B);
  console.log(`[收尾] 协商后仍能正常完局，${w} 胜`);
  return w;
}

/** 场景 3：第三方观战（房主观战链接 → 观察者接入 → 看到实时落子）。 */
async function scenarioWatch(A, B, C) {
  await pairUp(A, B);
  const spec = await A.inviteSpectate();
  console.log(`[观战] 链接: ${spec.slice(0, 90)}${spec.length > 90 ? "…" : ""}`);
  if (C._browser && !C.page.url().startsWith("http://tauri") && !C.page.url().startsWith("https://appassets")) {
    await C.goto(spec);
  } else {
    await C.enterP2P();
    await C.waitServerReady();
    await C.joinByPaste(spec);
  }
  await C.waitSnap((s) => s.phase === "playing", 90000, `${C.name} 进入观战`);
  await C.waitSnap((s) => s.peerConnected, 60000, `${C.name} 直连建立`);
  console.log(`[观战] ${C.name} role=${(await C.snap()).role}`);
  // A 落一手，观战者应看到
  // 逐手落子并观察观战者同步（便于定位镜像丢失在那一手）。
  for (let i = 0; i < 6; i++) {
    const s = await A.snap();
    const mover = s.toMove === s.myColor ? A : B;
    const mv = await playOneMove(mover);
    if (!mv) { await sleep(250); i--; continue; }
    await waitSync(A, B, 40000);
    const cs = await C.snap();
    console.log(`  [观战] 第${i + 1}手 ${mover.name}(${mv.x},${mv.y}) → C 手数=${cs.moveCount}`);
  }
  await C.waitSnap((s) => s.moveCount >= 6, 30000, "观战者看到落子");
  console.log(`[观战] 观战者同步到手数 ${(await C.snap()).moveCount} ✓`);
  const w = await playToVictory(A, B);
  console.log(`[收尾] 观战在场下完局，${w} 胜`);
  return w;
}

/* ----------------------------- 入口 ----------------------------- */

(async () => {
  const [scenario, specA, specB, specC] = process.argv.slice(2);
  const eps = [];
  try {
    const A = await open(specA, "A");
    eps.push(A);
    const B = await open(specB, "B");
    eps.push(B);
    let C = null;
    if (specC) { C = await open(specC, "C"); eps.push(C); }
    console.log(`[就绪] A=${A.name} B=${B.name}${C ? " C=" + C.name : ""}`);
    console.log(`[就绪] ${await A.brief()}`);
    console.log(`[就绪] ${await B.brief()}`);

    let winner;
    if (scenario === "game") winner = await scenarioGame(A, B);
    else if (scenario === "chat") winner = await scenarioChat(A, B);
    else if (scenario === "watch") winner = await scenarioWatch(A, B, C);
    else throw new Error(`未知场景: ${scenario}`);

    console.log(`===== MATCH OK: ${A.name} vs ${B.name}${C ? " + " + C.name : ""} → winner=${winner} =====`);
    process.exit(0);
  } catch (e) {
    console.error(`===== MATCH FAIL: ${e.message} =====`);
    for (const ep of eps) {
      try { console.error(`  [${ep.name}] ${await ep.brief()}`); } catch { /* ignore */ }
      if (ep.errors.length) console.error(`  [${ep.name}] pageerror: ${ep.errors.slice(0, 3).join(" | ")}`);
    }
    process.exit(1);
  } finally {
    for (const ep of eps) await ep.close();
  }
})();
