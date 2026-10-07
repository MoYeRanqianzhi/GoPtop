/**
 * 坏回执重试 E2E：receipt1 应用后对端消失 → receipt2 的二次 SRD 失败 → 提示必须上屏。
 *
 * **为什么单独有这条**：128502f 修「坏回执后同钥匙重试被拒」时，状态机侧的重试
 * 受理有了，但「receipt1 已应用（→stable）、连接未成」这个子态里，receipt2 的
 * setRemoteDescription 会被浏览器拒绝——这条失败链（transport 事件 → 状态机提示 →
 * DOM）没有任何测试压过。状态机单测喂的是构造事件，遮住了两个真路径前提：
 * ICE 失败会先经 gone 分支把 main 槽位**移除**（提示守卫若按槽位过滤就永远静默——
 * 第一版守卫就栽在这，本脚本抓出来的）；SRD 拒绝发生在真浏览器里。
 *
 * 时序：B1 生成回执后**立刻关页面**（PC 销毁）→ A 受理 receipt1（SRD 成功 → stable，
 * 但永远连不上）→ 等对端失联在 ICE 上显现（实测 Chrome 可能停在 disconnected 不
 * 升级 failed，两条时间线都接受）→ B2 重开同一邀请链接生成 receipt2 → A 受理 →
 * 断言提示文案出现在页面上、A 仍可重试。升级到 failed（槽位被移除）的那条时间线
 * 由状态机单测 rtc_apply_failed_after_ice_gone_still_notices 钉住。
 *
 * 前置：两份静态服务（5173 localhost / 5174 127.0.0.1，指向 frontend/dist）。
 * 用法：node receipt-retry.js        退出码即结果
 */
const { chromium } = require("playwright");

const A_ORIGIN = "http://localhost:5173";
const B_ORIGIN = "http://127.0.0.1:5174";
const NOTICE_TEXT = "这份回执无法应用";

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}

async function newClient(browser, origin, name) {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log(`[${name} pageerror]`, String(e).slice(0, 160)));
  await page.goto(origin + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate(() => {
    localStorage.setItem("goptop:server-sel", "none");
    localStorage.removeItem("goptop:tabUser");
  });
  await page.goto(origin + "/p2p", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(1200);
  return { ctx, page, name, origin };
}

const snap = async (c) => JSON.parse(await c.page.evaluate(() => window.__session.snapshot()));

/** 打开邀请链接 → 等回执生成 → 返回回执链接（调用方决定要不要立刻关掉）。 */
async function genReceipt(browser, invitePathQ, tag) {
  const b = await newClient(browser, B_ORIGIN, tag);
  await b.page.goto(B_ORIGIN + invitePathQ, { waitUntil: "domcontentloaded" });
  const ok = await b.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.answerBackUrl);
  }, null, { timeout: 30000, polling: 500 }).then(() => true).catch(() => false);
  if (!ok) throw new Error(`[${tag}] 回执生成失败`);
  return { client: b, receipt: (await snap(b)).answerBackUrl };
}

/** A 的真实 UI 粘贴受理。 */
async function aAccept(A, receipt) {
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "回执").click();
  });
  await A.page.fill('input[placeholder="粘贴受邀者发来的回执链接"]', receipt);
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "确认回执").click();
  });
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const A = await newClient(browser, A_ORIGIN, "本机甲");

  // ---- 1. A 生成无服务器邀请 ----
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click();
  });
  const inviteOk = await A.page.waitForFunction(() => {
    const codes = [...document.querySelectorAll("code")].map((x) => x.textContent || "");
    return codes.some((t) => t.includes("pwd=") && t.includes("rtc="));
  }, null, { timeout: 30000, polling: 500 }).then(() => true).catch(() => false);
  check("1. A 生成无服务器邀请链接", inviteOk);
  if (!inviteOk) { await browser.close(); process.exit(1); }
  const inviteUrl = (await snap(A)).inviteUrl;
  const invitePathQ = new URL(inviteUrl).pathname + new URL(inviteUrl).search;

  // ---- 2. B1 生成 receipt1 后立刻销毁（PC 没了，A 那份 answer 永远连不上）----
  const b1 = await genReceipt(browser, invitePathQ, "远端乙1");
  check("2. B1 生成 receipt1", true);
  await b1.client.ctx.close();

  // ---- 3. A 受理 receipt1：SRD 成功（→stable）但 ICE 必然失败 ----
  await aAccept(A, b1.receipt);
  const stillWaiting = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.phase === "waiting" && !s.peerConnected);
  }, null, { timeout: 40000, polling: 1000 }).then(() => true).catch(() => false);
  check("3. A 受理 receipt1 后保持等待（连接从未建立，没有误进对局）", stillWaiting);

  // ---- 4. 等对端失联在 ICE 上显现（disconnected 或 failed 都算）----
  // Chrome 实测可能长时间停在 disconnected 不升级到 failed（wasm 只在
  // Failed/Closed 才发 gone 事件）——但无论哪条时间线，receipt1 的 SRD 已让
  // signalingState 进入 stable，receipt2 的二次 SRD 必然被拒。读 ice_debug
  // 而不是盲等，保证「对端已死」的前提真实成立。
  const iceDead = await A.page.waitForFunction(async () => {
    try {
      // ice_debug 在适配器上是 async（run.js 栽过同一个坑）：谓词必须 await 它
      const rows = JSON.parse((await window.__session.ice_debug()) || "[]");
      return rows.some((r) => r.tag === "main" && (r.ice === "disconnected" || r.ice === "failed"));
    } catch { return false; }
  }, null, { timeout: 90000, polling: 2000 }).then(() => true).catch(() => false);
  check("4. 对端失联在 ICE 上显现（disconnected/failed）", iceDead);
  if (!iceDead) {
    console.log("A ice:", await A.page.evaluate(() => window.__session.ice_debug()));
    console.log("A 快照:", JSON.stringify(await snap(A)));
    await browser.close(); process.exit(1);
  }

  // ---- 5. B2 重开同一链接生成 receipt2；A 受理 → 二次 SRD 必败 ----
  const b2 = await genReceipt(browser, invitePathQ, "远端乙2");
  await aAccept(A, b2.receipt);
  await b2.client.ctx.close();

  // ---- 6. 核心断言：提示上屏 + A 仍可重试 ----
  const noticed = await A.page.waitForFunction((t) => document.body.innerText.includes(t), NOTICE_TEXT,
    { timeout: 15000, polling: 400 }).then(() => true).catch(() => false);
  check("5. receipt2 应用失败后提示上屏（此前这条链全程静默）", noticed);
  const aPhase = (await snap(A)).phase;
  check("6. A 仍在等待态（重试窗口未焊死）", aPhase === "waiting", `phase=${aPhase}`);
  if (!noticed) {
    console.log("A 快照:", JSON.stringify(await snap(A)));
    console.log("A notice 区:", await A.page.evaluate(() => document.body.innerText.slice(0, 400)));
  }

  console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
  await browser.close();
  process.exit(fail > 0 ? 1 : 0);
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
