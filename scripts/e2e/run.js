/** GoPtop 服务器模式全流程 E2E：A/B 对局 + 聊天 + 悔棋/换棋/重开协商 + C 观战全链 + D/E/F/G 大厅挑战（弹窗）。 */
const { chromium } = require("playwright");

const LOCAL_SERVER_CFG = JSON.stringify([{ id: "local", label: "local", url: "ws://127.0.0.1:9527/ws", builtin: false }]);

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}

async function newClient(browser, origin, name) {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log(`[${name} pageerror]`, String(e).slice(0, 120)));
  await page.goto(origin + "/", { waitUntil: "domcontentloaded" });
  await page.evaluate(([cfg, n]) => {
    localStorage.setItem("goptop:servers", cfg);
    localStorage.setItem("goptop:server-sel", "local");
    localStorage.setItem("goptop:name", n);
  }, [LOCAL_SERVER_CFG, name]);
  return { ctx, page, name, origin };
}

async function gotoP2P(c) {
  await c.page.goto(c.origin + "/p2p", { waitUntil: "domcontentloaded" });
  await c.page.waitForFunction(() => document.body.innerText.includes("服务器已连接"), null, { timeout: 15000 });
}

async function bodyHas(c, text, timeout = 12000) {
  try {
    await c.page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout });
    return true;
  } catch { return false; }
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

function moveCount(c) {
  return c.page.evaluate(() => {
    const lines = document.body.innerText.split("\n");
    const i = lines.findIndex((l) => l === "手数");
    return i >= 0 ? lines[i + 1] : null;
  });
}

async function openChat(c) {
  await c.page.evaluate(() => {
    const btn = [...document.querySelectorAll("button")].find((b) => b.querySelector("svg"));
    if (btn) btn.click();
  });
  await c.page.waitForSelector('input[placeholder="说点什么…"]', { timeout: 5000 });
}

async function sendChat(c, text) {
  await c.page.evaluate((t) => {
    const input = [...document.querySelectorAll("input")].find((i) => i.placeholder === "说点什么…");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(input, t);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, text);
  await c.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "发送").click(); });
}

/** 在 c 页找到含关键字的横幅并点指定按钮 */
async function answerConfirm(c, keyword, buttonText) {
  const ok = await c.page.evaluate(([kw, btn]) => {
    const cards = [...document.querySelectorAll(".brutal-card")].filter((d) => d.innerText.includes(kw) && d.innerText.includes(btn));
    if (cards.length === 0) return false;
    const b = [...cards[cards.length - 1].querySelectorAll("button")].find((x) => x.textContent === btn);
    if (!b) return false;
    b.click();
    return true;
  }, [keyword, buttonText]);
  return ok;
}

/** 在 c 的用户列表里点指定名字那一行的「挑战」按钮。
 *  必须取包含名字的「最小」div（行），向上爬会越过行边界撞进整张卡片误点别人。 */
async function challengeFromList(c, peerName) {
  return c.page.evaluate((name) => {
    const divs = [...document.querySelectorAll("div")].filter((d) =>
      d.innerText.includes(name) && [...d.querySelectorAll("button")].some((b) => b.textContent === "挑战" && !b.disabled));
    const row = divs.find((d) => !divs.some((o) => o !== d && d.contains(o)));
    const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent === "挑战" && !x.disabled);
    if (b) { b.click(); return true; }
    return false;
  }, peerName);
}

(async () => {
  const browser = await chromium.launch({
    executablePath: "C:/Users/MoYeR/AppData/Local/ms-playwright/chromium-1228/chrome-win64/chrome.exe",
    headless: true,
  args: ["--disable-features=WebRtcHideLocalIpsWithMdns", "--enforce-webrtc-ip-permission-check=false"],
  });

  const A = await newClient(browser, "http://localhost:5173", "阿甲");
  const B = await newClient(browser, "http://127.0.0.1:5174", "阿乙");
  await gotoP2P(A);
  await gotoP2P(B);

  // —— 1. A 开局，拿邀请链接与观战链接 ——
  await A.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent.includes("开启对战")).click(); });
  await A.page.waitForFunction(() => { const c = document.querySelector("code"); return c && c.textContent.includes("pwd="); }, null, { timeout: 15000 });
  const inviteUrl = await A.page.evaluate(() => document.querySelector("code").textContent);
  check("1. A 生成邀请链接(userId 形式)", /\/u-[^?]+\?pwd=/.test(inviteUrl), inviteUrl);

  // —— 2. B 打开邀请 → 免回执直连 ——
  const bUrl = new URL(inviteUrl);
  await B.page.goto(B.origin + bUrl.pathname + bUrl.search, { waitUntil: "domcontentloaded" });
  check("2. B 免回执直连", await bodyHas(B, "已连接", 20000));
  check("2b. B 进入对局", await bodyHas(B, "对局开始", 5000));
  check("2c. A 进入对局", await bodyHas(A, "对方已加入", 5000));

  // —— 3. A 落子 → 同步 ——
  await placeStone(A);
  await new Promise((r) => setTimeout(r, 800));
  check("3. 落子同步", (await moveCount(B)) === "1");

  // —— 4. 聊天 ——
  await openChat(A);
  await sendChat(A, "hello-e2e");
  await openChat(B);
  check("4. 聊天 A→B", await bodyHas(B, "hello-e2e", 5000));

  // —— 5. 悔棋协商 ——
  await B.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "悔棋").click(); });
  check("5a. B 请求已提示", await bodyHas(B, "已请求悔棋", 3000));
  check("5b. A 弹出确认", await bodyHas(A, "请求悔棋", 5000));
  check("5c. A 同意成功", await answerConfirm(A, "请求悔棋", "同意"));
  await new Promise((r) => setTimeout(r, 900));
  check("5d. 双方手数回 0", (await moveCount(A)) === "0" && (await moveCount(B)) === "0");

  // —— 6. 换棋协商 ——
  await A.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "换棋").click(); });
  check("6a. B 弹出换棋确认", await bodyHas(B, "请求换棋", 5000));
  check("6b. B 同意成功", await answerConfirm(B, "请求换棋", "同意"));
  await new Promise((r) => setTimeout(r, 900));
  const aColor = await A.page.evaluate(() => (document.body.innerText.match(/执(黑|白)/) || [])[1]);
  const bColor = await B.page.evaluate(() => (document.body.innerText.match(/执(黑|白)/) || [])[1]);
  check("6c. 黑白互换", aColor === "白" && bColor === "黑", `A=${aColor} B=${bColor}`);

  // —— 7. 重开协商 ——
  await A.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "重开").click(); });
  check("7a. B 弹出重开确认", await bodyHas(B, "请求重开", 5000));
  check("7b. B 同意成功", await answerConfirm(B, "请求重开", "同意"));
  check("7c. A 收到重开同意", await bodyHas(A, "对方已同意重开", 6000));

  // —— 8. C 观战（正确 pwd → 自动同意 → 直连看棋） ——
  const specUrl = await A.page.evaluate(() => { const c = [...document.querySelectorAll("code")].map((x) => x.textContent).find((t) => t && t.includes("spec=1")); return c || null; });
  check("8a. A 有观战链接", !!specUrl, "no spec url");
  const C = await newClient(browser, "http://localhost:5173", "吃瓜群众");
  if (specUrl) {
    const u = new URL(specUrl);
    await C.page.goto(C.origin + u.pathname + u.search, { waitUntil: "domcontentloaded" });
    // 「观战」字样在「正在连接对局观战…」提示即刻出现，真实门槛是 RTC gathering
  // （waitGathering 上限 8s，跨网/STUN 慢时 5s 窗口会假阴）——手数给足 15s
  check("8b. C 自动批准观战", await bodyHas(C, "观战", 15000) && await bodyHas(C, "手数", 15000));
  }

  // —— 9. 观战发言申请：双 host 批准（C 面板无输入框，只有「申请发言」） ——
  await C.page.evaluate(() => {
    const btn = [...document.querySelectorAll("button")].find((b) => b.querySelector("svg"));
    if (btn) btn.click();
  });
  await C.page.waitForSelector('button:text-is("申请发言")', { timeout: 6000 });
  await C.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "申请发言").click(); });
  check("9a. C 申请已发送", await bodyHas(C, "发言申请已发送", 4000));
  check("9b. A 弹出发言批准", await bodyHas(A, "申请参与聊天", 6000));
  check("9c. A 批准", await answerConfirm(A, "申请参与聊天", "同意"));
  check("9d. B 弹出发言批准", await bodyHas(B, "申请参与聊天", 6000));
  check("9e. B 批准", await answerConfirm(B, "申请参与聊天", "同意"));
  const gotVoice = await bodyHas(C, "双方已同意", 6000);
  check("9f. C 获得发言权", gotVoice);
  await sendChat(C, "spec-chat-e2e");
  check("9g. 观战发言到达 A", await bodyHas(A, "spec-chat-e2e", 5000));

  // —— 9h/9i/9j. 悔棋回退对观战者可见（审计 B1 回归：SyncState sv 纪元）。
  //  6c 换棋后 A 执白，重开后轮黑——由 B（黑）落子、A 请求悔棋、B 批准 ——
  await placeStone(B);
  await new Promise((r) => setTimeout(r, 800));
  check("9h. 观战者看到新落子", (await moveCount(C)) === "1");
  await A.page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "悔棋").click(); });
  check("9i. B 弹出悔棋确认", await bodyHas(B, "请求悔棋", 5000));
  await answerConfirm(B, "请求悔棋", "同意");
  await new Promise((r) => setTimeout(r, 900));
  check("9j. 观战者看到回退", (await moveCount(C)) === "0");

  // —— 10. 踢出（观战管理在聊天面板内：先点聊天图标展开） ——
  await A.page.evaluate(() => {
    const btn = [...document.querySelectorAll("button")].find((b) => b.querySelector("svg"));
    if (btn) btn.click();
  });
  await new Promise((r) => setTimeout(r, 500));
  await A.page.evaluate(() => {
    const rows = [...document.querySelectorAll(".brutal-card")].filter((d) => d.innerText.includes("观战（"));
    for (const row of rows) {
      const btns = [...row.querySelectorAll("button")].filter((b) => b.textContent === "踢出");
      for (const b of btns) b.click();
    }
  });
  check("10. C 被踢回主页", await bodyHas(C, "你已被移出观战", 8000));

  // —— 11. D 从 /p2p 用户列表挑战 E：服务器信令 + 邀请弹窗 → 双方自动进对局 ——
  const D = await newClient(browser, "http://localhost:5173", "阿丁");
  const E = await newClient(browser, "http://127.0.0.1:5174", "阿戊");
  await gotoP2P(D);
  await gotoP2P(E);
  check("11a. D 列表看到 E", await bodyHas(D, "阿戊", 8000));
  check("11b. D 点挑战成功", await challengeFromList(D, "阿戊"));
  check("11c. E 弹出邀请弹窗", await bodyHas(E, "邀请你加入对局", 8000));
  check("11d. E 弹窗同意", await answerConfirm(E, "邀请你加入对局", "同意"));
  check("11e. D 自动进入对局", await bodyHas(D, "对局开始", 20000));
  check("11f. E 自动进入对局", await bodyHas(E, "对局开始", 20000));
  await new Promise((r) => setTimeout(r, 800));
  check("11g. E 看到棋盘", (await moveCount(E)) !== null);

  // —— 12. F 从 /users 页挑战 G：同握手走 /users 入口 ——
  const F = await newClient(browser, "http://localhost:5173", "阿己");
  const G = await newClient(browser, "http://127.0.0.1:5174", "阿庚");
  await gotoP2P(F);
  await F.page.goto(F.origin + "/users", { waitUntil: "domcontentloaded" });
  await gotoP2P(G);
  check("12a. F 用户列表看到 G", await bodyHas(F, "阿庚", 8000));
  check("12b. F 点挑战成功", await challengeFromList(F, "阿庚"));
  check("12c. G 弹出邀请弹窗", await bodyHas(G, "邀请你加入对局", 8000));
  check("12d. G 弹窗同意", await answerConfirm(G, "邀请你加入对局", "同意"));
  check("12e. F 自动进入对局", await bodyHas(F, "对局开始", 20000));
  check("12f. G 自动进入对局", await bodyHas(G, "对局开始", 20000));

  // —— 13. 聊天停靠栏三档（用户拍板 2026-09-19）：等宽 → 压缩 → 弹窗 ——
  // A/B 仍在局中：以 A 的视口宽度扫一遍，三种形态各断言一次。
  // 红线：任何宽度下棋盘都不许被聊天挤小（下棋软件，棋盘优先）。
  const chatGeom = () => A.page.evaluate(() => {
    const q = (s) => document.querySelector(s);
    const vis = (el) => !!el && getComputedStyle(el).display !== "none";
    const w = (el) => (el ? Math.round(el.getBoundingClientRect().width) : 0);
    const layout = q(".game-layout");
    return {
      form: vis(q(".chat-modal-bg")) ? "modal" : vis(q(".chat-dock")) ? "dock" : "none",
      stack: w(q(".play-stack")), board: w(q(".board-wrap > .brutal-card")), dock: w(q(".chat-dock")),
      overflow: layout.scrollWidth - layout.clientWidth,
    };
  });
  const atWidth = async (width) => { await A.page.setViewportSize({ width, height: 900 }); await new Promise((r) => setTimeout(r, 450)); return chatGeom(); };
  await A.page.evaluate(() => { const b = [...document.querySelectorAll("button")].find((x) => (x.getAttribute("title") || "").includes("聊天")); if (b) b.click(); });
  await new Promise((r) => setTimeout(r, 400));
  const chatWide = await atWidth(1400);
  check("13a. 宽屏：聊天以停靠栏形态显示", chatWide.form === "dock", JSON.stringify(chatWide));
  check("13b. 宽屏：停靠栏与棋盘整组等宽", chatWide.dock === chatWide.stack, `dock=${chatWide.dock} stack=${chatWide.stack}`);
  const chatTight = await atWidth(800);
  check("13c. 偏窄：停靠栏被压缩（棋盘不动）", chatTight.form === "dock" && chatTight.dock < chatTight.stack, JSON.stringify(chatTight));
  check("13d. 偏窄：棋盘宽度与宽屏一致", chatTight.board === chatWide.board, `${chatTight.board} vs ${chatWide.board}`);
  const chatNarrow = await atWidth(650);
  check("13e. 更窄：退回弹窗形态", chatNarrow.form === "modal", JSON.stringify(chatNarrow));
  check("13f. 更窄：棋盘仍未被挤小", chatNarrow.board === chatWide.board, `${chatNarrow.board} vs ${chatWide.board}`);
  check("13g. 各档均无横向溢出", chatWide.overflow <= 0 && chatTight.overflow <= 0 && chatNarrow.overflow <= 0,
    `wide=${chatWide.overflow} tight=${chatTight.overflow} narrow=${chatNarrow.overflow}`);
  // 本段是最后一步，仍恢复原尺寸（聊天保持打开，不影响后续无步骤的事实）
  await A.page.setViewportSize({ width: 1400, height: 900 });

  console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
  await browser.close();
  process.exit(fail > 0 ? 1 : 0);
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
