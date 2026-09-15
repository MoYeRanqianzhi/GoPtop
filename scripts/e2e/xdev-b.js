/** 跨设备测试·remote B/C 端（v2：等待条件全部用快照轮询，兼容意图直达对局）。
 *  node xdev-b.js join   <invitePathQ>   —— B 打开邀请链接（服务器模式），断言入局并落子
 *  node xdev-b.js watch  <specPathQ>     —— C 打开观战链接（服务器模式），断言观战+手数同步
 *  node xdev-b.js nswatch <specPathQ>    —— C 无服务器观战：打开 specrtc 链接，输出观战回执
 */
const { chromium } = require("playwright");
const fs = require("fs");
const action = process.argv[2];
const pathQ = process.argv[3];
const ORIGIN = "http://127.0.0.1:5175";
const TMP = require("os").tmpdir();

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log("[B pageerror]", String(e).slice(0, 150)));
  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate((a) => {
    localStorage.removeItem("goptop:tabUser");
    if (a === "nswatch") localStorage.setItem("goptop:server-sel", "none");
    else localStorage.removeItem("goptop:server-sel");
    localStorage.setItem("goptop:name", a === "join" ? "remote乙" : "remote丙");
  }, action);
  await page.goto(ORIGIN + pathQ, { waitUntil: "domcontentloaded" });
  console.log("[B] opened", ORIGIN + pathQ);

  // 通用等待：serverState ready 或已进对局（意图直达时「服务器已连接」文案不出现）。
  const waitReady = () => page.waitForFunction(() => {
    if (!window.__session) return false;
    const s = JSON.parse(window.__session.snapshot());
    return s.serverState === "ready" || s.phase === "playing";
  }, null, { timeout: 25000, polling: 400 }).then(() => true).catch(() => false);

  if (action === "join") {
    const ready = await waitReady();
    console.log("[B] server ready:", ready);
    const ok = await page.waitForFunction(() => document.body.innerText.includes("已直连") || document.body.innerText.includes("经服务器中转") || document.body.innerText.includes("对方已同意"), null, { timeout: 60000, polling: 500 })
      .then(() => true).catch(() => false);
    const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
    console.log("[B] joined:", ok, JSON.stringify({ phase: snap.phase, peer: snap.peerConnected, moveCount: snap.moveCount }));
    if (ok) {
      await page.waitForTimeout(2500);
      const s2 = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
      const svg = await page.$('svg[role="grid"]');
      if (svg && !s2.boardDisabled) {
        const box = await svg.boundingBox();
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await page.waitForTimeout(1500);
        const s3 = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
        console.log("[B] placed:", s3.moveCount > s2.moveCount, "moveCount:", s3.moveCount);
      } else {
        console.log("[B] board disabled, waiting opponent (B is white)");
      }
    } else {
      console.log("[B ice]", await page.evaluate(() => window.__session.ice_debug()));
    }
    await browser.close();
    return;
  }

  if (action === "watch") {
    const ok = await page.waitForFunction(() => {
      if (!window.__session) return false;
      const s = JSON.parse(window.__session.snapshot());
      return s.role === "spectator" && s.peerConnected;
    }, null, { timeout: 60000, polling: 500 }).then(() => true).catch(() => false);
    const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
    console.log("[C] watching:", ok, JSON.stringify({ phase: snap.phase, role: snap.role, peer: snap.peerConnected, moveCount: snap.moveCount }));
    await browser.close();
    return;
  }

  if (action === "nswatch") {
    const ok = await page.waitForFunction(() => {
      if (!window.__session) return false;
      return !!JSON.parse(window.__session.snapshot()).answerBackUrl;
    }, null, { timeout: 60000, polling: 500 }).then(() => true).catch(() => false);
    if (!ok) {
      console.log("[C] receipt not generated");
      const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
      console.log("[C] state:", JSON.stringify({ phase: snap.phase, role: snap.role, answerBack: !!snap.answerBackUrl }));
      console.log("[C ice]", await page.evaluate(() => window.__session.ice_debug()));
      await browser.close();
      process.exit(1);
    }
    const answerUrl = JSON.parse(await page.evaluate(() => window.__session.snapshot())).answerBackUrl;
    const parsed = JSON.parse(await page.evaluate((u) => window.__session.parse_answer(u), answerUrl));
    fs.writeFileSync(TMP + "/xdev-receipt.json", JSON.stringify(parsed.answer, null, 2));
    console.log("[C] receipt ready:", JSON.stringify({ spectator: parsed.answer.spectator, rtcAns: (parsed.answer.rtcAns || "").slice(0, 20) + "..." }));
    console.log("RECEIPT_JSON=" + fs.readFileSync(TMP + "/xdev-receipt.json", "utf8").replace(/\n/g, ""));
    await browser.close();
    return;
  }
})().catch((e) => { console.error("B CRASH:", e); process.exit(2); });
