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
 * **判定同步真的走了直连**：两个壳都是无服务器模式（没有 WS、没有 relay），
 * 进程内广播又跨不了进程——所以 B 收到 A 的落子只可能是 DataChannel 送的。
 * 复核口径分两条，别混：`GOPTOP_TRACE_BC=1` 下两份日志里 `bc recv` 一次都没有，
 * **只排除原生同进程广播**（wasm 会话的 BroadcastChannel 不产生任何 shell 日志，
 * 对「壳静默跑 wasm」这条是盲的）；排除 wasm 靠的是脚本里逐壳的资源时间线断言
 * （壳内不该出现 transport wasm 的加载记录）。
 *
 * 用法：node shell-pair.js [portA] [portB]      默认 9222 / 9223
 * 前置：
 *   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./target/release/goptop.exe
 *   WEBVIEW2_USER_DATA_FOLDER="<dir>\wv" USERPROFILE="<dir>"  *     LOCALAPPDATA="<dir>\AppData\Local" APPDATA="<dir>\AppData\Roaming"  *     WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9223" ./target/release/goptop.exe
 *
 *   第二个实例要换**一整套 profile 环境**，各治一种病：
 *   - `WEBVIEW2_USER_DATA_FOLDER`：不换则共用一个 WebView2 进程，两个页面挤在同一个
 *     CDP 端点里，分不清谁是谁；
 *   - `USERPROFILE` + `LOCALAPPDATA` + `APPDATA`：**三个都要**。只换 `USERPROFILE`
 *     时 WebView2 起不来（它的 loader/缓存还要 LOCALAPPDATA，缺了窗口建不出来、
 *     调试端口也不开），表现是第二个壳静默不启动、日志空白。
 *     不换则两个壳共用 `%USERPROFILE%\.goptop\store.json`
 *     （`store.rs` 的桌面落盘位置就是 `~/.goptop`）——**同一个身份、同一份设置**，
 *     后起的那个会把先起的覆盖掉。症状是 B 打开 A 的邀请链接时命中状态机里的
 *     「这是你自己的主页链接」守卫（`user_id == s.user_id`）直接返回，表现为
 *     「B 停在主页、什么都不发生」，看着像传输层没接上。
 *     真机跨设备不会遇到，这是同机双实例才有的测试环境问题。
 */
const { Endpoint } = require("./device.js");

const PORT_A = process.argv[2] || "9222";
const PORT_B = process.argv[3] || "9223";

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}

/**
 * 铺设置 → 整页重载（让 storeInit 重新读平台存储，否则门面里还是旧的内存副本）。
 *
 * **`goptop:userId` 必须逐壳指定**：两个壳进程共用同一份 `~/.goptop/store.json`
 * ——`WEBVIEW2_USER_DATA_FOLDER` 只隔离 WebView2 自己的数据，隔离不了 Tauri 的
 * 应用数据目录。不指定的话两个壳读到同一个身份，B 打开 A 的邀请链接会命中状态机里
 * 的「这是你自己的主页链接」直接返回（`user_id == s.user_id` 那条守卫），
 * 表现是 B 停在主页、什么都不发生——**看着像传输层没接上，其实是同一个人的两个窗口**。
 * 真机跨设备不会遇到，这是同机双实例才有的测试环境问题。
 */
async function prep(ep, name, uid) {
  await ep.setSetting("goptop:server-sel", "none");
  await ep.setSetting("goptop:name", name);
  await ep.setSetting("goptop:userId", uid);
  const origin = new URL(ep.page.url()).origin;
  await ep.page.goto(origin + "/p2p", { waitUntil: "domcontentloaded" });
  await ep.ready(30000);
  return origin;
}

const snap = async (ep) => JSON.parse(await ep.page.evaluate(() => window.__session.snapshot()));

/** 资源时间线里的 transport wasm 命中。wasm 分支的动态 import 会立刻 fetch
 *  goptop_transport_bg.wasm（goptop_transport.js 的 wbg 胶水），所以这是页面上
 *  唯一能区分 native/wasm 会话的自动手段——快照没有传输类型字段，
 *  而 [bc recv]=0 只排除原生同进程广播，对「壳乙静默跑 wasm」是盲的。 */
const wasmInTimeline = async (ep) => ep.page.evaluate(() =>
  performance.getEntriesByType("resource").map((r) => r.name).filter((n) => /goptop_transport/.test(n)));

(async () => {
  const A = await Endpoint.cdp("壳甲", `http://127.0.0.1:${PORT_A}`);
  const B = await Endpoint.cdp("壳乙", `http://127.0.0.1:${PORT_B}`);

  const originA = await prep(A, "壳甲", "u-shell-aaa-0001");
  const originB = await prep(B, "壳乙", "u-shell-bbb-0002");
  console.log(`origins: ${originA} / ${originB}`);

  // 「都是原生会话」必须逐壳真断言：壳乙才是环境被改写（USERPROFILE/LOCALAPPDATA/APPDATA
  // 重定向）的那个实例，此前资源时间线只查壳甲，对半边系统假通过。prep 的整页重载
  // 之后会话已按 isTauri 选型完毕，wasm 分支此刻必然已在时间线里留下记录——
  // 提前到开局前拦住，别等落子全绿了才发现壳在跑 wasm。
  for (const ep of [A, B]) {
    const hits = await wasmInTimeline(ep);
    check(`${ep.name} 未加载 transport wasm（原生会话，没有可退的 wasm 后端）`, hits.length === 0, hits.join(",") || "无");
  }

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
