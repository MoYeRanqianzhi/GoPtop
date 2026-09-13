/**
 * 跨设备拟人化测试控制服务（本机与 remote 通用，argv 区分）。
 * 用法：node challenge-server.js <origin> <myName> <port> [chromeExe]
 * 常驻 headless 浏览器，HTTP 接口逐步驱动，所有点击走真实鼠标事件（mouse.click），
 * 每步由外部取 /shot 截图人工核验。
 *   POST /open            → 打开 /p2p 并等服务器已连接
 *   GET  /status          → 页面关键状态 JSON
 *   GET  /shot            → PNG 截图
 *   POST /challenge {peer}→ 在用户列表点该用户行的「挑战」
 *   POST /accept          → 点邀请弹窗「同意」
 *   POST /place {x,y}     → 真实点击棋盘交叉点
 */
const { chromium } = require("playwright");
const http = require("http");

const ORIGIN = process.argv[2];
const MY_NAME = process.argv[3];
const PORT = Number(process.argv[4]);
const CHROME = process.argv[5] || null;
const SIZE = 15;

let page = null;
let browser = null;

async function ensure() {
  if (page) return;
  browser = await chromium.launch(CHROME ? { executablePath: CHROME, headless: true } : { args: ["--no-sandbox"] });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  page = await ctx.newPage();
  page.on("pageerror", (e) => console.log("[pageerror]", String(e).slice(0, 160)));
  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.evaluate((n) => localStorage.setItem("goptop:name", n), MY_NAME);
}

async function openP2P() {
  await ensure();
  await page.goto(ORIGIN + "/p2p", { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.getByText("服务器已连接").waitFor({ timeout: 20000 });
}

function readStatus() {
  return page.evaluate(() => {
    const t = document.body.innerText;
    const lines = t.split("\n");
    const i = lines.findIndex((l) => l === "手数");
    return {
      connected: t.includes("服务器已连接"),
      modal: t.includes("邀请你加入对局"),
      challengeSent: t.includes("挑战已发出"),
      inGame: i >= 0,
      moveCount: i >= 0 ? Number(lines[i + 1]) : null,
      turn: t.includes("黑 胜") ? "black-win" : t.includes("白 胜") ? "white-win"
        : t.includes("黑 落子") ? "black" : t.includes("白 落子") ? "white" : null,
      link: (t.match(/(已直连|经服务器中转|连接中)[^\n]*/) || [null])[0],
      roster: [...document.querySelectorAll("button")].some((b) => b.textContent === "挑战"),
    };
  });
}

/** 拿「含目标文本 + 目标按钮」的最小 div，返回按钮中心坐标（真实鼠标点击用） */
function buttonBox(page, text_kw, btnText) {
  return page.evaluate(([kw, btn]) => {
    const divs = [...document.querySelectorAll("div")].filter((d) =>
      d.innerText.includes(kw) && [...d.querySelectorAll("button")].some((b) => b.textContent === btn && !b.disabled));
    const row = divs.find((d) => !divs.some((o) => o !== d && d.contains(o)));
    const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent === btn && !x.disabled);
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }, [text_kw, btnText]);
}

async function humanClick(page, box) {
  await page.mouse.move(box.x, box.y);
  await page.waitForTimeout(120);
  await page.mouse.click(box.x, box.y);
}

async function placeStone(x, y) {
  const box = await page.evaluate(([bx, by, n]) => {
    const svg = document.querySelector('svg[role="grid"]');
    if (!svg) return null;
    const r = svg.getBoundingClientRect();
    const vb = svg.viewBox.baseVal.width;
    const pad = 30;
    const cell = (vb - pad * 2) / (n - 1);
    return { x: r.left + (pad + bx * cell) * (r.width / vb), y: r.top + (pad + by * cell) * (r.height / vb) };
  }, [x, y, SIZE]);
  if (!box) throw new Error("board not found");
  await humanClick(page, box);
}

const server = http.createServer(async (req, res) => {
  try {
    if (req.method === "POST" && req.url === "/open") {
      await openP2P();
      return json(res, { ok: true, ...(await readStatus()) });
    }
    if (req.method === "GET" && req.url === "/status") {
      if (!page) return json(res, { ok: false, why: "not opened" });
      return json(res, { ok: true, ...(await readStatus()) });
    }
    if (req.method === "GET" && req.url === "/shot") {
      if (!page) { res.writeHead(404); return res.end(); }
      const buf = await page.screenshot();
      res.writeHead(200, { "Content-Type": "image/png", "Content-Length": buf.length });
      return res.end(buf);
    }
    if (req.method === "POST" && req.url === "/eval") {
      const { expr, arg } = await body(req);
      const result = await page.evaluate(new Function("arg", `return (${expr})(arg)`), arg);
      return json(res, { ok: true, result });
    }
    if (req.method === "POST" && req.url === "/challenge") {
      const { peer } = await body(req);
      const box = await buttonBox(page, peer, "挑战");
      if (!box) return json(res, { ok: false, why: "challenge button not found" });
      await humanClick(page, box);
      return json(res, { ok: true, ...(await readStatus()) });
    }
    if (req.method === "POST" && req.url === "/accept") {
      const box = await buttonBox(page, "邀请你加入对局", "同意");
      if (!box) return json(res, { ok: false, why: "modal not found" });
      await humanClick(page, box);
      return json(res, { ok: true, ...(await readStatus()) });
    }
    if (req.method === "POST" && req.url === "/place") {
      const { x, y } = await body(req);
      await placeStone(x, y);
      await new Promise((r) => setTimeout(r, 400));
      return json(res, { ok: true, ...(await readStatus()) });
    }
    res.writeHead(404); res.end();
  } catch (e) {
    json(res, { ok: false, why: String(e).slice(0, 200) });
  }
});

function body(req) {
  return new Promise((resolve) => {
    let d = "";
    req.on("data", (c) => (d += c));
    req.on("end", () => { try { resolve(JSON.parse(d || "{}")); } catch { resolve({}); } });
  });
}
function json(res, o) { res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(o)); }

server.listen(PORT, "127.0.0.1", () => console.log(`challenge-server on ${PORT} for ${ORIGIN} as ${MY_NAME}`));
