/**
 * stress.js — 长时间对局与压力验证（人机对战 + 实时胜率）。
 *
 * 验的是「跑久了会不会坏」，不是功能能不能用（那由 ai.js 覆盖）：
 *   1. 长时间对局后界面是否仍可交互、胜率是否仍在更新；
 *   2. 走势图的滑动窗口是否始终守住窗口大小（不会随手数无限增长）；
 *   3. JS 堆是否失控（分析每次都要跨线程传局面 JSON，漏水会累积）；
 *   4. 快速连续落子（比分析快得多）时是否崩溃或卡死。
 *
 * 用法：
 *   node stress.js [端] [URL] [手数]
 *   端：web（默认）/ mob / android / cdp:<url>（鸿蒙 9444、桌面壳 9222）
 *   手数默认 120（围棋 9 路，够触发多轮窗口滑动）
 *
 * 注意：本脚本走**本地对战**（`/local`）而不是人机对战——压的是胜率分析与渲染
 * 这条链路，人机对战每手还要等 AI 思考，跑 120 手要十几分钟且引入无关变量。
 */
const { Endpoint } = require("./device.js");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let passed = 0;
let failed = 0;

function check(name, cond, extra = "") {
  if (cond) { passed++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { failed++; console.log(`FAIL ${name}${extra ? " — " + extra : ""}`); }
}

/** 用真实鼠标点棋盘上的 (gx, gy)。几何与 BoardSvg 一致：viewBox = 60 + (size-1)*36。 */
async function clickPoint(ep, gx, gy, size) {
  const geo = await ep.page.evaluate(() => {
    const b = document.querySelector(".board-wrap svg").getBoundingClientRect();
    return { x: b.x, y: b.y, w: b.width };
  });
  const PAD = 30, CELL = 36;
  const vb = PAD * 2 + (size - 1) * CELL;
  const s = geo.w / vb;
  await ep.page.mouse.click(
    Math.round(geo.x + (PAD + gx * CELL) * s),
    Math.round(geo.y + (PAD + gy * CELL) * s),
  );
}

/** 读对局卡上的手数。
 *  底部卡可能停在「胜率」上（手数只在「对局」卡里），所以先切回去再读。 */
async function moveCount(ep) {
  await backToGame(ep);
  return ep.page.evaluate(() => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });
}

/** 把底部切回「对局」卡（宽窄两套卡互斥显示，按可见性选）。 */
async function backToGame(ep) {
  const narrow = ep.page.locator(".bp-swap button");
  if (await narrow.isVisible().catch(() => false)) {
    for (let i = 0; i < 4; i++) {
      const label = (await ep.page.locator(".bp-swap .brutal-label").first().innerText().catch(() => "")).trim();
      if (label === "对局") return;
      await narrow.click();
      await sleep(400);
    }
  } else {
    const wide = ep.page.locator('button[title*="对局 / 胜率"]');
    if (await wide.isVisible().catch(() => false)) {
      const labels = await ep.page.locator(".bp-wide .brutal-label").allInnerTexts().catch(() => []);
      if (labels.some((t) => t.trim() === "对局")) return;
      await wide.click();
      await sleep(400);
    }
  }
}

/** 给 evaluate 兜超时：渲染主线程被占死时 evaluate 永不返回（Playwright 的 evaluate
 *  没有超时参数），而快速连点阶段的设计目的恰恰是诱发卡死——不与定时器赛跑，
 *  页面真卡死时脚本会挂死而非判负（node 不退出、无 FAIL、退出码语义丢失）。
 *  输了赛跑一方的 rejection 必须先接住，否则健康路径上会炸 unhandled rejection。 */
function withTimeout(p, ms, label) {
  let timer;
  const boom = new Promise((_, rej) => { timer = setTimeout(() => rej(new Error(`${label}（${ms / 1000}s 无响应）`)), ms); });
  boom.catch(() => {});
  return Promise.race([p, boom]).finally(() => clearTimeout(timer));
}

/** JS 堆占用（MB）；非 Chromium 返回 null。读堆同样走 evaluate，同样要兜卡死。 */
async function heapMB(ep) {
  return withTimeout(ep.page.evaluate(() => {
    const m = performance.memory;
    return m ? Math.round(m.usedJSHeapSize / 1048576) : null;
  }), 15000, "页面疑似卡死——读堆占用");
}

/** 探活：棋盘还渲染着吗。页面真卡死时由超时判负（异常落进 main().catch 非零退出）。 */
function probeAlive(ep, timeout = 15000) {
  return withTimeout(ep.page.evaluate(() => !!document.querySelector(".board-wrap svg")), timeout, "页面疑似卡死——探活");
}

/** 打开胜率卡（宽窄两套卡的可见性互斥，按可见性选）。 */
async function openOdds(ep) {
  const narrow = ep.page.locator(".bp-swap button");
  if (await narrow.isVisible().catch(() => false)) {
    for (let i = 0; i < 2; i++) { await narrow.click(); await sleep(600); }
  } else {
    const wide = ep.page.locator('button[title*="对局 / 胜率"]');
    if (await wide.isVisible().catch(() => false)) await wide.click();
    await sleep(600);
  }
}

async function main() {
  // 全局看门狗：兜住探活点之外的挂死（120 手循环里 clickPoint/moveCount 的 evaluate
  // 同样可能在主线程被占死后永不返回）。到点强制非零退出，保住退出码语义。
  const watchdog = setTimeout(() => { console.error("stress 全局超时（30 分钟无进展），强制退出"); process.exit(3); }, 30 * 60 * 1000);
  const spec = process.argv[2] ?? "web";
  // 壳端点不需要 base（从当前页面推导 origin），于是 `node stress.js cdp:… 120`
  // 的第二个参数直接就是手数——按位置死抠会让 120 被当成 base，拼出 "120/local"
  // 这种地址，导航静默失败后脚本仍在**上一个页面**上跑（实测：在 /ai 上跑出
  // 171 手、横坐标 -37 的假失败）。
  const numeric = /^\d+$/.test(process.argv[3] ?? "");
  const base = numeric ? "" : (process.argv[3] ?? "http://localhost:1420/");
  const total = Number(numeric ? process.argv[3] : (process.argv[4] ?? 120));
  const url = base.replace(/\/$/, "") + "/local";

  // 壳端点（android / 鸿蒙 cdp / 桌面壳 cdp）连的是**已经在跑的**应用，启动时停在
  // 菜单页，所以要导航到 /local；且各壳的资源 origin 不同（Tauri 是
  // http://tauri.localhost、鸿蒙是 appassets.goptop），没显式给 base 时从当前页面推导。
  let ep;
  let target = url;
  if (spec === "android" || spec.startsWith("android:")) {
    ep = await Endpoint.android("stress", { serial: spec.includes(":") ? spec.slice(8) : null });
    target = new URL(ep.page.url()).origin + "/local";
  } else if (spec.startsWith("cdp:")) {
    ep = await Endpoint.cdp("stress", spec.slice(4));
    if (!base) target = new URL(ep.page.url()).origin + "/local";
  } else if (spec === "mob") {
    ep = await Endpoint.browser("stress", url, { mobile: true });
  } else {
    ep = await Endpoint.browser("stress", url);
  }
  await ep.goto(target).catch(() => { /* 浏览器端点构造时已导航过，重复导航失败可忽略 */ });
  // 被吞的导航错误必须用落地位置兜住：壳端点（android/cdp）只接管不导航，这一条
  // 是进 /local 的**唯一**导航——静默失败会让 120 手全落在旧页面上跑（实测在 /ai
  // 上跑出 171 手、横坐标 -37 的假失败，报错远离真实根因）。
  const landed = ep.page.url();
  if (landed !== target) throw new Error(`[stress] 导航后停在 ${landed}，未进入 ${target}——导航失败被忽略`);
  await ep.ready(40000);

  // 切到 19 路：361 个交叉点，走 120 手不会把坐标用重复（9 路只有 81 点，第一版
  // 用 `(i%9, floor(i/9)%9)` 从第 82 手起就与前面撞点，落子被正确拒绝，却被
  // 断言记成了「落子没生效」——是脚本的错，不是产品的）。
  await ep.page.locator('button:has-text("围棋")').click();
  await sleep(900);
  const s19 = ep.page.locator('header button:has-text("19×19")');
  if (await s19.isVisible().catch(() => false)) { await s19.click(); await sleep(900); }
  const size = 19;

  const heap0 = await heapMB(ep);
  console.log(`起始堆占用: ${heap0 ?? "N/A"} MB`);

  // —— 落子：每手留 1.1s 给分析（500ms 预算），别把请求全挤成取消 ——
  const t0 = Date.now();
  let placed = 0;
  for (let i = 0; i < total; i++) {
    const gx = i % size, gy = Math.floor(i / size);
    await clickPoint(ep, gx, gy, size);
    await sleep(1100);
    placed++;
  }
  const elapsed = (Date.now() - t0) / 1000;
  await sleep(6000); // 等最后的分析收尾

  const moves = await moveCount(ep);
  check("长时间对局落子全部生效", moves === placed, `手数=${moves} / 点击=${placed}`);

  await openOdds(ep);
  await sleep(4000);
  const odds = await ep.page.evaluate(() => {
    const pl = document.querySelector(".wr-chart polyline");
    const pts = (pl?.getAttribute("points") ?? "").split(" ").filter(Boolean);
    const xs = pts.map((p) => parseFloat(p.split(",")[0]));
    return {
      ptCount: xs.length,
      minX: xs.length ? Math.round(Math.min(...xs)) : null,
      maxX: xs.length ? Math.round(Math.max(...xs)) : null,
      aria: document.querySelector(".wr-chart")?.getAttribute("aria-label") ?? "",
      boxW: Math.round(document.querySelector(".wr-chart-box")?.getBoundingClientRect().width ?? 0),
      barW: document.querySelector(".wr-fill--mine")?.style.width ?? null,
      text: document.querySelector(".wr-bar")?.innerText.replace(/\n/g, " ") ?? "",
    };
  });
  const windowSize = odds.boxW > 0 ? Math.max(6, Math.floor(odds.boxW / 6)) : 40;

  check("走势图仍在渲染", odds.ptCount > 0, odds.aria);
  // 滑动窗口的核心断言：点数不得超过窗口容量，否则就是「越画越密」又回来了
  check(
    "走势图点数不超过窗口容量",
    odds.ptCount <= windowSize,
    `点数=${odds.ptCount} 窗口=${windowSize}`,
  );
  check("曲线横坐标恒在框内", odds.minX !== null && odds.minX >= 0 && odds.maxX <= 100, `x∈[${odds.minX}, ${odds.maxX}]`);
  check("胜率条仍在更新", odds.barW !== null, odds.text);

  // —— 界面仍可交互：再落一手并确认手数增长 ——
  const before = await moveCount(ep);
  await clickPoint(ep, 9, 9, size);
  await sleep(2500);
  const after = await moveCount(ep);
  check("长跑后界面仍可交互", after === before + 1, `${before} → ${after}`);

  // —— 快速连续落子：比分析快得多，考验取消与队列 ——
  for (let i = 0; i < 12; i++) {
    const gx = (i * 3) % size, gy = (i * 7) % size;
    await clickPoint(ep, gx, gy, size);
    await sleep(40);
  }
  await sleep(12000);
  // 探活必须带超时：裸 evaluate 在主线程被占死时永不返回，而诱发卡死正是这一步的目的
  const alive = await probeAlive(ep);
  check("快速连点后页面未崩溃", alive);

  const heap1 = await heapMB(ep);
  if (heap0 !== null && heap1 !== null) {
    check("JS 堆未失控（< 200MB）", heap1 < 200, `${heap0} → ${heap1} MB`);
  } else {
    console.log("SKIP JS 堆检查（该环境不暴露 performance.memory）");
  }

  console.log(`\n${placed} 手用时 ${elapsed.toFixed(1)}s\n${passed} passed, ${failed} failed`);
  await ep.close();
  clearTimeout(watchdog);
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
