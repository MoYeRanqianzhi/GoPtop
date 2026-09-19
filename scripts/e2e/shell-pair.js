/**
 * shell-pair.js —— **桌面壳原生传输**的双实例真实对局。
 *
 * 这是本轮「原生端不再跑 wasm」改动的核心验证：桌面壳现在把会话交给
 * `goptop-transport-native`（Rust + tokio + webrtc-rs），而不再是 WebView 里的
 * `WasmSession`。改的正是刚修好并验过的 P2P 路径，所以必须真打一局。
 *
 * 两个壳是**两个进程**，进程内广播 hub 各一份 → 没有同源通道可退，
 * 因此这条路必然走「邀请链接 → 回执 → 直连（DataChannel）」的全链，
 * 顺带把回执那条此前无自动化覆盖的通路也压上。
 *
 * 用法：node shell-pair.js [portA] [portB]      默认 9222 / 9223
 * 前置：
 *   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./target/release/goptop.exe
 *   WEBVIEW2_USER_DATA_FOLDER="<dir>" WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9223" ./target/release/goptop.exe
 *   （第二个实例必须换 user data folder，否则共用一个 WebView2 进程、两个页面挤在同一个 CDP 端点里）
 */
const { Endpoint } = require("./device.js");

const PORT_A = process.argv[2] || "9222";
const PORT_B = process.argv[3] || "9223";

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}

/** 铺设置 → 整页重载（让 storeInit 重新读平台存储，否则门面里还是旧的内存副本）。 */
async function prep(ep, name) {
  await ep.setSetting("goptop:server-sel", "none");
  await ep.setSetting("goptop:name", name);
  const origin = new URL(ep.page.url()).origin;
  await ep.page.goto(origin + "/p2p", { waitUntil: "domcontentloaded" });
  await ep.ready(30000);
  return origin;
}

const snap = async (ep) => JSON.parse(await ep.page.evaluate(() => window.__session.snapshot()));

(async () => {
  const A = await Endpoint.cdp("壳甲", `http://127.0.0.1:${PORT_A}`);
  const B = await Endpoint.cdp("壳乙", `http://127.0.0.1:${PORT_B}`);

  const originA = await prep(A, "壳甲");
  const originB = await prep(B, "壳乙");
  check("两个壳都是原生会话（__session 已就绪）", true, `${originA} / ${originB}`);

  // 1) A 开局，等带 rtc 的邀请链接
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click();
  });
  const inviteOk = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.inviteUrl && s.inviteUrl.includes("rtc="));
  }, null, { timeout: 40000, polling: 500 }).then(() => true).catch(() => false);
  check("1. A 生成带 rtc 的无服务器邀请链接", inviteOk);
  if (!inviteOk) { console.log("A 快照:", JSON.stringify(await snap(A))); await done(2); }

  const inviteUrl = (await snap(A)).inviteUrl;
  const invitePathQ = new URL(inviteUrl).pathname + new URL(inviteUrl).search;

  // 2) B 换源打开邀请链接（两个进程，没有同源广播可用）
  await B.page.goto(originB + invitePathQ, { waitUntil: "domcontentloaded" });
  const bJoined = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && (s.phase === "waiting" || s.phase === "playing"));
  }, null, { timeout: 40000, polling: 500 }).then(() => true).catch(() => false);
  check("2. B 打开邀请链接后进入等待/对局态", bJoined);

  // 3) B 出回执（answer 回传的唯一通路）
  const receiptOk = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.answerBackUrl);
  }, null, { timeout: 40000, polling: 500 }).then(() => true).catch(() => false);
  check("3. B 生成回执链接", receiptOk);
  if (!receiptOk) { console.log("B 快照:", JSON.stringify(await snap(B))); await done(3); }
  const receipt = (await snap(B)).answerBackUrl;

  // 4) A 走真实 UI 粘贴回执受理
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "回执").click();
  });
  await A.page.fill('input[placeholder="粘贴受邀者发来的回执链接"]', receipt);
  await A.page.evaluate(() => {
    [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "确认回执").click();
  });

  const aPlaying = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.phase === "playing");
  }, null, { timeout: 60000, polling: 500 }).then(() => true).catch(() => false);
  check("4. A 受理回执后进入对局", aPlaying);

  const bPlaying = await B.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.phase === "playing");
  }, null, { timeout: 60000, polling: 500 }).then(() => true).catch(() => false);
  check("5. B 同步进入对局", bPlaying);

  const connected = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.peerConnected);
  }, null, { timeout: 60000, polling: 500 }).then(() => true).catch(() => false);
  check("6. 直连建立（peerConnected，走原生 DataChannel）", connected);
  if (!connected) {
    console.log("A ice:", await A.page.evaluate(() => window.__session.ice_debug()));
    console.log("A 快照:", JSON.stringify(await snap(A)));
  }

  // 5) 落子双向同步（数据面真的通）
  const placeAt = async (ep, x, y) => {
    await ep.page.evaluate(([bx, by]) => {
      const svg = document.querySelector('svg[role="grid"]');
      const r = svg.getBoundingClientRect();
      const vb = svg.viewBox.baseVal.width;
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
  }, null, { timeout: 25000, polling: 400 }).then(() => true).catch(() => false);
  check("7. A 落子经原生直连同步到 B", bSawOne);

  await placeAt(B, 8, 7);
  const aSawTwo = await A.page.waitForFunction(() => {
    const s = window.__session && JSON.parse(window.__session.snapshot());
    return !!(s && s.moveCount >= 2);
  }, null, { timeout: 25000, polling: 400 }).then(() => true).catch(() => false);
  check("8. B 落子经原生直连同步回 A", aSawTwo);

  // 6) 确认没退回 wasm：壳里不该加载 transport wasm
  const chunks = await A.page.evaluate(() =>
    performance.getEntriesByType("resource").map((r) => r.name).filter((n) => /goptop_transport/.test(n)));
  check("9. 壳内未加载 transport wasm（没有可退的 wasm 后端）", chunks.length === 0, chunks.join(",") || "无");

  console.log("A 终态:", JSON.stringify(await snap(A)).slice(0, 160));
  await done();

  async function done(exitCode) {
    if (A) await A.close().catch(() => {});
    if (B) await B.close().catch(() => {});
    if (exitCode !== undefined) { console.log(`\n${pass} passed, ${fail} failed`); process.exit(1); }
    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    process.exit(fail > 0 ? 1 : 0);
  }
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
