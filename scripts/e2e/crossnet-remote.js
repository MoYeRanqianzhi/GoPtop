/**
 * 跨设备真实对战测试——remote 端（受邀者，执白）。
 * 用法：node challenge-bot.js  → stdout 打 JSON 状态行，全程自足，结束自动退出。
 * 流程：开官服 → 等挑战者上名册 → 等邀请弹窗 → 同意 → 执白按计划落子 → 等黑胜。
 */
const { chromium } = require("playwright");

const ORIGIN = "http://127.0.0.1:8000";
const MY_NAME = "远端乙";
const PEER_NAME = "本机甲";
const WHITE_MOVES = [[1, 0], [1, 1], [1, 2], [1, 3]]; // 让黑连五：白只在第 1 行陪跑
const SIZE = 15;

function log(obj) { console.log("JSON " + JSON.stringify(obj)); }

/** 点棋盘交叉点（bx,by）：由 SVG viewBox 反推屏幕坐标后派发指针事件 */
async function placeAt(page, bx, by) {
  return page.evaluate(([x, y, n]) => {
    const svg = document.querySelector('svg[role="grid"]');
    if (!svg) return false;
    const r = svg.getBoundingClientRect();
    const vb = svg.viewBox.baseVal.width;
    const pad = 30;
    const cell = (vb - pad * 2) / (n - 1);
    const cx = r.left + (pad + x * cell) * (r.width / vb);
    const cy = r.top + (pad + y * cell) * (r.height / vb);
    for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
      const ev = type.startsWith("pointer")
        ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 })
        : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
      svg.dispatchEvent(ev);
    }
    return true;
  }, [bx, by, SIZE]);
}

async function bodyHas(page, text, timeout) {
  try { await page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout }); return true; }
  catch { return false; }
}

async function moveCount(page) {
  return page.evaluate(() => {
    const lines = document.body.innerText.split("\n");
    const i = lines.findIndex((l) => l === "手数");
    return i >= 0 ? Number(lines[i + 1]) : null;
  });
}

async function statusLine(page) {
  return page.evaluate(() => {
    const m = document.body.innerText.match(/(已直连|经服务器中转|连接中)[^\n]*/);
    return m ? m[0] : null;
  });
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => log({ type: "pageerror", msg: String(e).slice(0, 160) }));

  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate((n) => localStorage.setItem("goptop:name", n), MY_NAME);
  await page.goto(ORIGIN + "/p2p", { waitUntil: "domcontentloaded" });

  const connected = await bodyHas(page, "服务器已连接", 20000);
  log({ step: "server", ok: connected });
  if (!connected) { log({ result: "FAIL", why: "server not connected" }); await browser.close(); process.exit(1); }

  if (!(await bodyHas(page, PEER_NAME, 60000))) {
    log({ result: "FAIL", why: "challenger not in roster" }); await browser.close(); process.exit(1);
  }
  log({ step: "peer-in-roster", ok: true });

  // 等邀请弹窗（横幅文案已废弃，弹窗文案唯一）→ 点弹窗内「同意」
  if (!(await bodyHas(page, "邀请你加入对局", 90000))) {
    log({ result: "FAIL", why: "invite modal not shown" }); await browser.close(); process.exit(1);
  }
  const clicked = await page.evaluate(() => {
    const divs = [...document.querySelectorAll("div")].filter((d) =>
      d.innerText.includes("邀请你加入对局") && [...d.querySelectorAll("button")].some((b) => b.textContent === "同意"));
    const row = divs.find((d) => !divs.some((o) => o !== d && d.contains(o)));
    const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent === "同意");
    if (b) { b.click(); return true; }
    return false;
  });
  log({ step: "modal-accept", ok: clicked });

  if (!(await bodyHas(page, "对局开始", 30000))) {
    log({ result: "FAIL", why: "not entered playing" }); await browser.close(); process.exit(1);
  }
  log({ step: "playing", status: await statusLine(page) });

  // 执白循环：等「白 落子」→ 按计划落子 → 等手数+1；黑五连后出现「黑 胜」
  let mc = await moveCount(page);
  for (const [bx, by] of WHITE_MOVES) {
    if (!(await bodyHas(page, "白 落子", 45000))) {
      log({ result: "FAIL", why: "white turn not detected", moveCount: mc }); await browser.close(); process.exit(1);
    }
    if (!(await placeAt(page, bx, by))) {
      log({ result: "FAIL", why: "place failed", at: [bx, by] }); await browser.close(); process.exit(1);
    }
    for (let i = 0; i < 40; i++) {
      const now = await moveCount(page);
      if (now !== null && now > mc) { mc = now; break; }
      await new Promise((r) => setTimeout(r, 500));
    }
    log({ step: "white-placed", at: [bx, by], moveCount: mc });
  }
  const win = await bodyHas(page, "黑 胜", 30000);
  log({ result: win ? "PASS" : "FAIL", role: "white", moveCount: await moveCount(page), status: await statusLine(page) });
  await browser.close();
  process.exit(win ? 0 : 1);
})().catch((e) => { log({ result: "CRASH", why: String(e).slice(0, 300) }); process.exit(2); });
