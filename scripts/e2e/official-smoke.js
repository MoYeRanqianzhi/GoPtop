/** 官服回归冒烟：两客户端连 wss://goptopserver.meowoo.org/ws 走一轮邀请→对局→聊天。 */
const { chromium } = require("playwright");
const OFFICIAL_CFG = JSON.stringify([{ id: "official", label: "official", url: "wss://goptopserver.meowoo.org/ws", builtin: true }]);

async function newClient(browser, origin, name) {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log(`[${name} pageerror]`, String(e).slice(0, 200)));
  await page.goto(origin + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate(([cfg, n]) => {
    localStorage.setItem("goptop:servers", cfg);
    localStorage.setItem("goptop:server-sel", "official");
    localStorage.setItem("goptop:name", n);
  }, [OFFICIAL_CFG, name]);
  return { ctx, page, name, origin };
}
async function bodyHas(c, text, timeout = 20000) {
  try { await c.page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout }); return true; }
  catch { return false; }
}
async function placeStone(c) {
  await c.page.evaluate(() => {
    const svg = document.querySelector('svg[role="grid"]');
    const r = svg.getBoundingClientRect();
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
      const ev = type.startsWith("pointer") ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 }) : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
      svg.dispatchEvent(ev);
    }
  });
}

let pass = 0, fail = 0;
const check = (n, ok, extra) => { if (ok) { pass++; console.log(`PASS ${n}`); } else { fail++; console.log(`FAIL ${n}${extra ? " | " + extra : ""}`); } };

(async () => {
  const browser = await chromium.launch({
    executablePath: "C:/Users/MoYeR/AppData/Local/ms-playwright/chromium-1228/chrome-win64/chrome.exe",
    headless: true,
  });
  const A = await newClient(browser, "http://localhost:5173", "官服甲");
  const B = await newClient(browser, "http://127.0.0.1:5174", "官服乙");
  for (const c of [A, B]) {
    await c.page.goto(c.origin + "/p2p", { waitUntil: "domcontentloaded" });
    check(`连接官服(${c.name})`, await bodyHas(c, "服务器已连接"));
  }
  check("名册互通", await bodyHas(A, "官服乙", 15000));
  await A.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click(); });
  await A.page.waitForFunction(() => { const c = document.querySelector("code"); return c && c.textContent.includes("pwd="); }, null, { timeout: 20000 });
  const inviteUrl = await A.page.evaluate(() => document.querySelector("code").textContent);
  check("邀请链接生成", /\/u-[^?]+\?pwd=/.test(inviteUrl), inviteUrl);
  const bu = new URL(inviteUrl);
  await B.page.goto(B.origin + bu.pathname + bu.search, { waitUntil: "domcontentloaded" });
  check("乙免回执直连", await bodyHas(B, "已连接", 30000));
  check("乙进入对局", await bodyHas(B, "对局开始", 10000));
  check("甲进入对局", await bodyHas(A, "对方已加入", 10000));
  await placeStone(A);
  await new Promise((r) => setTimeout(r, 1200));
  check("落子同步（relay/直连）", await B.page.evaluate(() => document.body.innerText.split("\n").some((l, i, a) => l === "手数" && a[i + 1] === "1")));
  console.log(`\n===== OFFICIAL SMOKE: ${pass} passed, ${fail} failed =====`);
  await browser.close();
  process.exit(fail > 0 ? 1 : 0);
})().catch((e) => { console.error("CRASH:", e); process.exit(2); });
