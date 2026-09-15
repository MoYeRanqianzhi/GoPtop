/** 跨设备测试·本机 A 端。用法：
 *  node xdev-a.js stage1 server    —— 服务器模式开局，输出邀请/观战链接（path+query）到 /tmp/xdev-a.json
 *  node xdev-a.js stage1 noserver  —— 无服务器模式开局（含 specrtc），同上
 *  node xdev-a.js stage2 server    —— 服务器模式对局断言（B 加入后跑）：直连/中转、落子同步
 *  node xdev-a.js stage2 noserver  —— 读取 /tmp/xdev-receipt.json 粘贴观战回执，断言 C 连上
 */
const { chromium } = require("playwright");
const fs = require("fs");
const TMP = require("os").tmpdir();
const mode = process.argv[2] || "server";
const stage = process.argv[3] || "stage1";
const FULL = stage === "full";

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log("[A pageerror]", String(e).slice(0, 150)));
  await page.goto("http://localhost:5173/", { waitUntil: "domcontentloaded" });
  await page.evaluate((m) => {
    localStorage.removeItem("goptop:tabUser");
    if (m === "noserver") localStorage.setItem("goptop:server-sel", "none");
    else localStorage.removeItem("goptop:server-sel");
    localStorage.setItem("goptop:name", "本机甲");
  }, mode);
  await page.goto("http://localhost:5173/p2p", { waitUntil: "domcontentloaded" });
  if (mode === "server") {
    await page.waitForFunction(() => document.body.innerText.includes("服务器已连接"), null, { timeout: 20000, polling: 500 });
    console.log("[A] official server connected");
  } else {
    await page.waitForTimeout(1500);
    console.log("[A] noserver mode");
  }

  await page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click(); });
  if (stage === "stage1" || FULL) {
    // 等邀请链接出现（服务器模式立即；无服务器等 offer 编码完成）。
    const deadline = mode === "noserver" ? 30000 : 20000;
    await page.waitForFunction(() => {
      const codes = [...document.querySelectorAll("code")].map((x) => x.textContent);
      return codes.some((t) => t && t.includes("pwd="));
    }, null, { timeout: deadline, polling: 500 });
    // 无服务器模式再等 specUrl（观战 offer gathering 完成才生成）。
    if (mode === "noserver") {
      await page.waitForFunction(() => {
        const codes = [...document.querySelectorAll("code")].map((x) => x.textContent);
        return codes.some((t) => t && t.includes("specrtc="));
      }, null, { timeout: 20000, polling: 500 }).catch(() => console.log("[A] WARN specrtc link not ready"));
    }
    const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
    const out = {
      mode,
      invite: snap.inviteUrl,
      spec: snap.specUrl,
      specPwdFallback: snap.specPwd,
      pwd: snap.pwd,
    };
    fs.writeFileSync(TMP + "/xdev-a.json", JSON.stringify(out, null, 2));
    console.log("[A] stage1 done:", JSON.stringify({ invite: !!out.invite, spec: !!out.spec, pwd: out.pwd }));
    if (out.invite) console.log("INVITE_PATH=" + new URL(out.invite).pathname + new URL(out.invite).search);
    if (out.spec) console.log("SPEC_PATH=" + new URL(out.spec).pathname + new URL(out.spec).search);
    if (mode === "server" && out.specPwdFallback) console.log("SPEC_CONSTRUCT=" + JSON.stringify({ userId: snap.userId, specPwd: out.specPwdFallback }));
    if (!FULL) { await browser.close(); return; }
    // FULL：保持在线，轮询对端加入（remote 端由外部 ssh 触发）。
    if (mode === "server") {
      const ok = await page.waitForFunction(() => {
        if (!window.__session) return false;
        const s = JSON.parse(window.__session.snapshot());
        return s.phase === "playing";
      }, null, { timeout: 90000, polling: 500 }).then(() => true).catch(() => false);
      const s = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
      console.log("[A] peer joined:", ok, JSON.stringify({ phase: s.phase, peer: s.peerConnected, moveCount: s.moveCount }));
      // 落子一手验证数据面（直连或 relay 任一通路即可）。
      await page.waitForTimeout(1000);
      const s2 = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
      const svg = await page.$('svg[role="grid"]');
      if (svg && !s2.boardDisabled) {
        const box = await svg.boundingBox();
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await page.waitForTimeout(1500);
        const s3 = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
        console.log("[A] placed:", s3.moveCount > s2.moveCount, "moveCount:", s3.moveCount);
      } else {
        console.log("[A] board disabled, skip place");
      }
      console.log("[A ice]", await page.evaluate(() => window.__session.ice_debug()));
    }
    if (mode === "noserver" && FULL) {
      // 轮询回执文件（外部把 C 端输出的 RECEIPT_JSON 写入），出现即受理并断言直连。
      const rf = TMP + "/xdev-receipt.json";
      const t0 = Date.now();
      // 等文件出现且非空（写入方是重定向，创建与写完之间有窗口）。
      while (Date.now() - t0 < 120000) {
        if (fs.existsSync(rf)) {
          try { if (fs.statSync(rf).size > 100 && JSON.parse(fs.readFileSync(rf, "utf8")).rtcAns) break; } catch { /* 半写，继续等 */ }
        }
        await page.waitForTimeout(1000);
      }
      if (!fs.existsSync(rf)) { console.log("[A] receipt file timeout"); await browser.close(); return; }
      const receipt = JSON.parse(fs.readFileSync(rf, "utf8"));
      console.log("[A] receipt received, applying...");
      await page.evaluate((r) => window.__session.accept_spec_receipt(JSON.stringify(r)), receipt);
      await page.waitForTimeout(12000);
      const snap2 = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
      const ice = JSON.parse(await page.evaluate(() => window.__session.ice_debug()));
      const specPeer = ice.find((x) => x.tag.startsWith("spec-live")) || {};
      console.log("[A] spec result:", JSON.stringify({ spectators: snap2.spectators.length, specIce: specPeer.ice, specDc: specPeer.dcState, remote: (specPeer.remote || []).length }));
    }
    await browser.close();
    return;
  }

  // stage2：等待对方加入并断言。
  if (mode === "server") {
    const ok = await page.waitForFunction(() => document.body.innerText.includes("已直连") || document.body.innerText.includes("经服务器中转"), null, { timeout: 60000, polling: 500 })
      .then(() => true).catch(() => false);
    const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
    console.log("[A] peer joined:", ok, JSON.stringify({ phase: snap.phase, peer: snap.peerConnected, moveCount: snap.moveCount }));
    if (!ok) { console.log("[A ice]", await page.evaluate(() => window.__session.ice_debug())); }
    await browser.close();
    return;
  }

  // 无服务器 stage2：粘贴观战回执。
  const receipt = JSON.parse(fs.readFileSync(TMP + "/xdev-receipt.json", "utf8"));
  console.log("[A] applying spec receipt:", receipt.answer.slice(0, 60) + "...");
  await page.evaluate((r) => window.__session.accept_spec_receipt(JSON.stringify(r)), receipt);
  const ok = await page.waitForFunction(() => document.body.innerText.includes("观战回执已受理"), null, { timeout: 10000, polling: 500 })
    .then(() => true).catch(() => false);
  await page.waitForTimeout(8000);
  const snap = JSON.parse(await page.evaluate(() => window.__session.snapshot()));
  const ice = JSON.parse(await page.evaluate(() => window.__session.ice_debug()));
  const specPeer = ice.find((x) => x.tag.startsWith("spec-live")) || {};
  console.log("[A] receipt applied:", ok, JSON.stringify({ spectators: snap.spectators.length, specIce: specPeer.ice, specDc: specPeer.dcState }));
  await browser.close();
})().catch((e) => { console.error("A CRASH:", e); process.exit(2); });
