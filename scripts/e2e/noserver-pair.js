/**
 * 无服务器跨设备直连 E2E：A 邀请 → B 出回执 → A 受理 → 落子同步。
 *
 * **为什么单独有这条**：浏览器面其余脚本（run.js）走服务器模式，信令经服务器直达，
 * 回执那条路根本不经过。无服务器 + 跨设备时 A 与 B 不在同一个源，BroadcastChannel
 * 递不过去，**回执是 answer 回传的唯一通路**——这条路曾长期不可用（回执链接把 pwd
 * 写死成 ""，受理端一律回「钥匙不符」），而它没有任何自动化覆盖，只能靠人肉双机。
 *
 * 两个上下文天然隔离（BroadcastChannel 按源 + 存储分区划分），B 再换到 127.0.0.1:5174，
 * 双保险确保走的是回执而不是同源广播。
 *
 * 前置：两份静态服务（5173 localhost / 5174 127.0.0.1，指向 frontend/dist）。
 * 用法：node noserver-pair.js        退出码即结果
 */
const { chromium } = require("playwright");

const A_ORIGIN = "http://localhost:5173";
const B_ORIGIN = "http://127.0.0.1:5174";

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
  await page.evaluate((n) => {
    localStorage.setItem("goptop:server-sel", "none");   // 无服务器模式
    localStorage.setItem("goptop:name", n);
    localStorage.removeItem("goptop:tabUser");
  }, name);
  await page.goto(origin + "/p2p", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(1200); // 等会话建立（无服务器模式没有「服务器已连接」可等）
  return { ctx, page, name, origin };
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const A = await newClient(browser, A_ORIGIN, "本机甲");
  const B = await newClient(browser, B_ORIGIN, "远端乙");

  const snap = async (c) => JSON.parse(await c.page.evaluate(() => window.__session.snapshot()));

  // ---- 1. A 开启对战，等带 rtc 的邀请链接 ----
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click();
  });
  const inviteOk = await A.page.waitForFunction(() => {
    const codes = [...document.querySelectorAll("code")].map((x) => x.textContent || "");
    return codes.some((t) => t.includes("pwd=") && t.includes("rtc="));
  }, null, { timeout: 30000, polling: 500 }).then(() => true).catch(() => false);
  check("1. A 生成带 rtc 的无服务器邀请链接", inviteOk);
  if (!inviteOk) { console.log("A 快照:", JSON.stringify(await snap(A))); await browser.close(); process.exit(1); }

  const inviteUrl = (await snap(A)).inviteUrl;
  const invitePathQ = new URL(inviteUrl).pathname + new URL(inviteUrl).search;
  console.log("  邀请链接 path+query:", invitePathQ.slice(0, 80) + "…");

  // ---- 2. B 换源打开邀请链接（跨源 = 没有同源广播可用）----
  await B.page.goto(B_ORIGIN + invitePathQ, { waitUntil: "domcontentloaded" });

  const bWaiting = await B.page.waitForFunction(() => {
    if (!window.__session) return false;
    const s = JSON.parse(window.__session.snapshot());
    return s.phase === "waiting" || s.phase === "playing";
  }, null, { timeout: 30000, polling: 500 }).then(() => true).catch(() => false);
  check("2. B 打开邀请链接后进入等待/对局态", bWaiting);

  // B 生成回执（answer 回传的唯一通路）
  const receiptOk = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.answerBackUrl);
  }, null, { timeout: 30000, polling: 500 }).then(() => true).catch(() => false);
  check("3. B 生成回执链接", receiptOk);
  if (!receiptOk) { console.log("B 快照:", JSON.stringify(await snap(B))); await browser.close(); process.exit(1); }

  const receipt = (await snap(B)).answerBackUrl;
  const receiptPwd = new URL(receipt).searchParams.get("pwd");
  const invitePwd = new URL(inviteUrl).searchParams.get("pwd");
  check("4. 回执带回本轮邀请钥匙（曾写死为空 → 受理端一律拒绝）",
    !!receiptPwd && receiptPwd === invitePwd, `receipt=${receiptPwd} invite=${invitePwd}`);

  // ---- 3. A 走真实 UI 粘贴回执受理 ----
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "回执").click();
  });
  await A.page.fill('input[placeholder="粘贴受邀者发来的回执链接"]', receipt);
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "确认回执").click();
  });

  // ---- 4. 双方进入对局且直连建立 ----
  const aPlaying = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.phase === "playing");
  }, null, { timeout: 45000, polling: 500 }).then(() => true).catch(() => false);
  check("5. A 受理回执后进入对局", aPlaying);

  const bPlaying = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.phase === "playing");
  }, null, { timeout: 45000, polling: 500 }).then(() => true).catch(() => false);
  check("6. B 同步进入对局", bPlaying);

  const connected = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.peerConnected);
  }, null, { timeout: 45000, polling: 500 }).then(() => true).catch(() => false);
  check("7. 直连建立（peerConnected，走 DataChannel 而非中转）", connected);
  if (!connected) {
    console.log("A ice:", await A.page.evaluate(() => window.__session.ice_debug()));
    console.log("A 快照:", JSON.stringify(await snap(A)));
  }

  // ---- 5. 落子同步（数据面真的通）----
  const placeAt = async (c, x, y) => {
    await c.page.evaluate(([bx, by]) => {
      const svg = document.querySelector('svg[role="grid"]');
      const r = svg.getBoundingClientRect();
      const vb = svg.viewBox.baseVal.width;
      // 尺寸从 aria-label（"gomoku board 15x15"）取，别写死——本页可能切 9/13/19 路
      const n = Number((svg.getAttribute("aria-label") || "").match(/(\d+)x\d+/)?.[1]) || 15;
      const pad = 30, cell = (vb - pad * 2) / (n - 1);
      const cx = r.left + (pad + bx * cell) * (r.width / vb);
      const cy = r.top + (pad + by * cell) * (r.height / vb);
      for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
        const ev = type.startsWith("pointer")
          ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 })
          : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
        svg.dispatchEvent(ev);
      }
    }, [x, y]);
  };

  await placeAt(A, 7, 7);
  const bSawOne = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.moveCount >= 1);
  }, null, { timeout: 20000, polling: 400 }).then(() => true).catch(() => false);
  check("8. A 落子经直连同步到 B", bSawOne);

  await placeAt(B, 8, 7);
  const aSawTwo = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.moveCount >= 2);
  }, null, { timeout: 20000, polling: 400 }).then(() => true).catch(() => false);
  check("9. B 落子经直连同步回 A", aSawTwo);

  const aSnap = await snap(A);
  console.log("  A 终态:", JSON.stringify({ phase: aSnap.phase, peer: aSnap.peerConnected, moves: aSnap.moveCount }));
  console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
  await browser.close();
  process.exit(fail > 0 ? 1 : 0);
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
