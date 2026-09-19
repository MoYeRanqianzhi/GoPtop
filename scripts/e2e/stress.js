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
 *   端：web（默认）/ mob / android / cdp:<url>
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

/** JS 堆占用（MB）；非 Chromium 返回 null。 */
async function heapMB(ep) {
  return ep.page.evaluate(() => {
    const m = performance.memory;
    return m ? Math.round(m.usedJSHeapSize / 1048576) : null;
  });
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
  const spec = process.argv[2] ?? "web";
  const base = process.argv[3] ?? "http://localhost:1420/";
  const total = Number(process.argv[4] ?? 120);
  const url = base.replace(/\/$/, "") + "/local";

  const ep = spec === "mob"
    ? await Endpoint.browser("stress", url, { mobile: true })
    : await Endpoint.browser("stress", url);
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
  const alive = await ep.page.evaluate(() => !!document.querySelector(".board-wrap svg"));
  check("快速连点后页面未崩溃", alive);

  const heap1 = await heapMB(ep);
  if (heap0 !== null && heap1 !== null) {
    check("JS 堆未失控（< 200MB）", heap1 < 200, `${heap0} → ${heap1} MB`);
  } else {
    console.log("SKIP JS 堆检查（该环境不暴露 performance.memory）");
  }

  console.log(`\n${placed} 手用时 ${elapsed.toFixed(1)}s\n${passed} passed, ${failed} failed`);
  await ep.close();
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
