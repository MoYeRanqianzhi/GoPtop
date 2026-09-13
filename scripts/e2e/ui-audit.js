/** UI 审查截图脚本：515px 窄屏遍历全部页面/状态，输出 shots/audit-*.png 供人工核验。
 *  全程按钮导航（pushState），不用 page.goto——整页刷新会清空内存对局状态，非人类路径。 */
const { chromium } = require("playwright");

const CFG = JSON.stringify([{ id: "local", label: "local", url: "ws://127.0.0.1:9527/ws", builtin: false }]);
const ORIGIN = "http://localhost:5173";

async function newPage(ctx, name) {
  const p = await ctx.newPage();
  p.on("pageerror", (e) => console.log(`[${name} pageerror]`, String(e).slice(0, 300)));
  p.on("console", (m) => { if (m.type() === "error") console.log(`[${name} console.error]`, m.text().slice(0, 300)); });
  await p.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await p.evaluate(([cfg, n]) => {
    localStorage.clear();
    localStorage.setItem("goptop:name", n);
    localStorage.setItem("goptop:servers", cfg);
    localStorage.setItem("goptop:server-sel", "local");
  }, [CFG, name]);
  // 配置只在启动时读取：必须整页刷新一次才会连本地服务器（pushState 导航不重载）
  await p.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  return p;
}
async function shot(p, file) {
  console.log("shot:", file);
  await p.screenshot({ path: `shots/audit-${file}.png` });
}
async function clickBtn(p, text) {
  try {
    await p.getByRole("button", { name: text, exact: true }).click({ timeout: 8000 });
  } catch (e) {
    const txt = await p.evaluate(() => document.body.innerText.slice(0, 300).replace(/\n/g, "|"));
    console.error(`clickBtn FAILED [${text}] body=${txt}`);
    throw e;
  }
}
async function bodyHas(p, t, timeout = 15000) {
  try {
    await p.waitForFunction((x) => document.body.innerText.includes(x), t, { timeout });
  } catch (e) {
    const txt = await p.evaluate(() => document.body.innerText.slice(0, 250).replace(/\n/g, "|"));
    console.error(`FAILED [${t}] body=${txt}`);
    throw e;
  }
}

(async () => {
  const browser = await chromium.launch({
    executablePath: "C:/Users/MoYeR/AppData/Local/ms-playwright/chromium-1228/chrome-win64/chrome.exe",
    headless: true,
  });
  const ctxA = await browser.newContext({ viewport: { width: 515, height: 933 } });
  const ctxB = await browser.newContext({ viewport: { width: 515, height: 933 } });
  const ctxC = await browser.newContext({ viewport: { width: 515, height: 933 } });

  const A = await newPage(ctxA, "审查甲");
  await shot(A, "01-menu");
  await clickBtn(A, "本地对战");
  await A.waitForTimeout(500);
  await shot(A, "02-local");
  await clickBtn(A, "菜单");
  await clickBtn(A, "P2P 对战");
  await bodyHas(A, "服务器已连接");

  const B = await newPage(ctxB, "审查乙");
  await B.goto(ORIGIN + "/p2p", { waitUntil: "domcontentloaded" });
  await bodyHas(B, "服务器已连接");
  await A.waitForTimeout(1500);

  // A 开局等待（邀请者视角）
  await clickBtn(A, "开启对战（等对手）");
  await bodyHas(A, "等待对手");
  await A.waitForTimeout(800);
  await shot(A, "03-waiting-inviter");

  // 按钮导航逛页面（状态应保留：等待不丢）
  await clickBtn(A, "菜单");
  await shot(A, "04-menu-during-waiting");
  await clickBtn(A, "在线用户（1）");
  await A.waitForTimeout(500);
  await shot(A, "05-users");
  await clickBtn(A, "菜单");
  await clickBtn(A, "设置");
  await A.waitForTimeout(500);
  await shot(A, "06-settings");
  await A.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  await A.waitForTimeout(300);
  await shot(A, "07-settings-bottom");
  await clickBtn(A, "菜单");
  await clickBtn(A, "P2P 对战");
  await bodyHas(A, "等待对手");
  // 挑战只能在被挑战方空闲（home）时送达：等待中的邀请者会自动拒绝（设计如此）
  await clickBtn(A, "取消");

  // B 从列表挑战 A（双方 home → A 弹窗）
  try {
    await clickBtn(B, "挑战");
  } catch (e) {
    const aTxt = await A.evaluate(() => document.body.innerText.slice(0, 250).replace(/\n/g, "|"));
    console.error("A side at failure:", aTxt);
    throw e;
  }
  await B.waitForTimeout(2000);
  await shot(B, "08-challenger-view");

  // A 同意弹窗 → 双方进对局
  const mbox = await A.evaluate(() => {
    const divs = [...document.querySelectorAll("div")].filter((d) =>
      d.innerText.includes("邀请你加入对局") && [...d.querySelectorAll("button")].some((b) => b.textContent === "同意"));
    const row = divs.find((d) => !divs.some((o) => o !== d && d.contains(o)));
    const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent === "同意");
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  if (!mbox) throw new Error("invite modal not found on A");
  await A.mouse.click(mbox.x, mbox.y);
  await bodyHas(B, "已连接", 20000);
  await bodyHas(A, "已连接", 20000);
  await B.waitForTimeout(1200);
  await shot(B, "09-playing-inviter");
  await shot(A, "10-playing-invitee");

  // C 用 B 卡里的观战链接开观战页（全新页面加载——观战本就是开链接场景）
  const specUrl = await B.evaluate(() => {
    const c = [...document.querySelectorAll("code")].map((x) => x.textContent).find((t) => t && t.includes("spec=1"));
    return c || null;
  });
  if (specUrl) {
    const C = await newPage(ctxC, "审查丙");
    const u = new URL(specUrl);
    await C.goto(ORIGIN + u.pathname + u.search, { waitUntil: "domcontentloaded" });
    await C.waitForTimeout(4000);
    await shot(C, "11-watch");
  } else {
    console.log("no spec url on B");
  }

  // A 对局中经菜单去自己主页（回局入口）
  await clickBtn(A, "菜单");
  await clickBtn(A, "我的主页");
  await A.waitForTimeout(800);
  await shot(A, "12-userpage-playing");

  console.log("audit shots done");
  await browser.close();
})().catch((e) => { console.error("CRASH:", String(e).slice(0, 400)); process.exit(2); });
