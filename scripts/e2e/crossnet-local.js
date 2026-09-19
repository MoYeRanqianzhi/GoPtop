/**
 * 跨设备真实对战测试——本机端（挑战者，执黑）。
 * 用法：node crossnet-local.js → stdout 打 JSON 状态行。
 * 流程：开官服 → 等 remote 上名册 → 从 /p2p 用户列表点「挑战」→ 等进对局 →
 *       执黑连五落子 → 等黑胜。对端脚本：crossnet-remote.js（ssh remote 上跑）。
 */
const { chromium } = require("playwright");

const ORIGIN = "http://localhost:5173";
const MY_NAME = "本机甲";
const PEER_NAME = "远端乙";
const BLACK_MOVES = [[0, 0], [0, 1], [0, 2], [0, 3], [0, 4]]; // 第 0 列连五（x=0 固定，y 递增）
const SIZE = 15;

function log(obj) { console.log("JSON " + JSON.stringify(obj)); }

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

/** 从 /p2p 用户列表点指定名字那一行的「挑战」按钮（取含名字的最小 div） */
async function challengeFromList(page, peerName) {
  return page.evaluate((name) => {
    const divs = [...document.querySelectorAll("div")].filter((d) =>
      d.innerText.includes(name) && [...d.querySelectorAll("button")].some((b) => b.textContent === "挑战" && !b.disabled));
    const row = divs.find((d) => !divs.some((o) => o !== d && d.contains(o)));
    const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent === "挑战" && !x.disabled);
    if (b) { b.click(); return true; }
    return false;
  }, peerName);
}

(async () => {
  // 本机 playwright npm 包与浏览器缓存版本不匹配，必须显式指定可执行文件（见 memory 2026-09-12）
  const browser = await chromium.launch({
    headless: true,
    executablePath: "C:/Users/MoYeR/AppData/Local/ms-playwright/chromium-1228/chrome-win64/chrome.exe",
  });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => log({ type: "pageerror", msg: String(e).slice(0, 160) }));

  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate((n) => localStorage.setItem("goptop:name", n), MY_NAME);
  await page.goto(ORIGIN + "/p2p", { waitUntil: "domcontentloaded" });

  const connected = await bodyHas(page, "服务器已连接", 20000);
  log({ step: "server", ok: connected });
  if (!connected) { log({ result: "FAIL", why: "official server not connected (proxy?)" }); await browser.close(); process.exit(1); }

  if (!(await bodyHas(page, PEER_NAME, 60000))) {
    log({ result: "FAIL", why: "remote peer not in roster" }); await browser.close(); process.exit(1);
  }
  log({ step: "peer-in-roster", ok: true });

  const sent = await challengeFromList(page, PEER_NAME);
  log({ step: "challenge-sent", ok: sent });
  if (!sent) { log({ result: "FAIL", why: "challenge click failed" }); await browser.close(); process.exit(1); }

  if (!(await bodyHas(page, "对局开始", 30000))) {
    log({ result: "FAIL", why: "not entered playing after accept" }); await browser.close(); process.exit(1);
  }
  log({ step: "playing", status: await statusLine(page) });

  // 执黑循环：等「黑 落子」→ 落子 → 等手数+1（含白方回手）；最后一手后等「黑 胜」
  let mc = await moveCount(page);
  for (let i = 0; i < BLACK_MOVES.length; i++) {
    const [bx, by] = BLACK_MOVES[i];
    if (!(await bodyHas(page, "黑 落子", 45000))) {
      log({ result: "FAIL", why: "black turn not detected", moveCount: mc }); await browser.close(); process.exit(1);
    }
    if (!(await placeAt(page, bx, by))) {
      log({ result: "FAIL", why: "place failed", at: [bx, by] }); await browser.close(); process.exit(1);
    }
    mc += 1;
    log({ step: "black-placed", at: [bx, by], moveCount: mc });
    if (i < BLACK_MOVES.length - 1) {
      for (let t = 0; t < 60; t++) {
        const now = await moveCount(page);
        if (now !== null && now > mc) { mc = now; break; }
        await new Promise((r) => setTimeout(r, 500));
      }
    }
  }
  const win = await bodyHas(page, "黑 胜", 30000);
  log({ result: win ? "PASS" : "FAIL", role: "black", moveCount: await moveCount(page), status: await statusLine(page) });
  await browser.close();
  process.exit(win ? 0 : 1);
})().catch((e) => { log({ result: "CRASH", why: String(e).slice(0, 300) }); process.exit(2); });
