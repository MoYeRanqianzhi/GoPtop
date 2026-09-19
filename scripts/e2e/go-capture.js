/** 围棋提子浏览器验证（wasm 规则下沉回归）：本地模式 9 路，7 手提白 (1,1)，再悔棋还原。 */
const { chromium } = require("playwright");
const EXE = "C:/Users/MoYeR/AppData/Local/ms-playwright/chromium-1228/chrome-win64/chrome.exe";
const SHOTS = require("path").join(__dirname, "shots", "phase5");

(async () => {
  const fs = require("fs");
  fs.mkdirSync(SHOTS, { recursive: true });
  const browser = await chromium.launch({ executablePath: EXE });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  let errors = 0;
  page.on("pageerror", (e) => { errors++; console.log("pageerror:", String(e).slice(0, 200)); });
  await page.goto("http://localhost:5173/local", { waitUntil: "domcontentloaded" });
  await page.evaluate(() => localStorage.setItem("goptop:server-sel", "none"));
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForSelector('svg[role="grid"]');
  await page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "围棋").click(); });
  await page.waitForTimeout(250);
  await page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "9×9").click(); });
  await page.waitForTimeout(350);

  async function clickPoint(gx, gy) {
    await page.evaluate(([gx, gy]) => {
      const svg = document.querySelector('svg[role="grid"]');
      const r = svg.getBoundingClientRect();
      // 9 路 viewBox 宽 = 2×30(padding) + 8×36(cell)，与 BoardSvg.tsx 的 padding/cell 同源：
      // 改棋盘几何这里必须同步（下方 30 + gx*36 与 stones() 的 (cx-30)/36 同理）
      const vb = 60 + 8 * 36;
      const cx = r.left + ((30 + gx * 36) / vb) * r.width;
      const cy = r.top + ((30 + gy * 36) / vb) * r.height;
      for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
        const ev = type.startsWith("pointer") ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 }) : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
        svg.dispatchEvent(ev);
      }
    }, [gx, gy]);
    await page.waitForTimeout(160);
  }
  const stones = () => page.evaluate(() =>
    [...document.querySelectorAll('svg[role="grid"] circle[r="15"]')].map((c) => [
      Math.round((+c.getAttribute("cx") - 30) / 36),
      Math.round((+c.getAttribute("cy") - 30) / 36),
      c.getAttribute("fill"),
    ])
  );
  const check = (name, ok) => console.log(`${ok ? "PASS" : "FAIL"} ${name}`);

  // 棋种切换后首手前：9 路空盘
  check("1. 切到围棋 9 路空盘", (await stones()).length === 0);

  // 7 手提白 (1,1)：B(1,0) W(1,1) B(2,1) W(5,5) B(0,1) W(6,6) B(1,2)
  for (const [x, y] of [[1, 0], [1, 1], [2, 1], [5, 5], [0, 1], [6, 6], [1, 2]]) await clickPoint(x, y);
  let st = await stones();
  check("2. 共 6 子（黑4 白2）", st.length === 6 && st.filter((s) => s[2] === "#0A0A0A").length === 4);
  check("3. 白 (1,1) 已被提", !st.some(([x, y]) => x === 1 && y === 1));
  check("4. 黑 (1,2) 在盘", st.some(([x, y, f]) => x === 1 && y === 2 && f === "#0A0A0A"));
  await page.screenshot({ path: `${SHOTS}/go-capture.png` });

  // 悔棋 → Rust 重放还原被提子
  await page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "悔棋").click(); });
  await page.waitForTimeout(400);
  st = await stones();
  check("5. 悔棋后白 (1,1) 还原", st.some(([x, y, f]) => x === 1 && y === 1 && f === "#FFFFFF"));
  check("6. 悔棋后黑 (1,2) 移除", !st.some(([x, y]) => x === 1 && y === 2));
  await page.screenshot({ path: `${SHOTS}/go-undo.png` });

  // 五子棋回归：切回五子棋（15 路），横排五连判胜（规则走 wasm）
  await page.evaluate(() => { [...document.querySelectorAll("button")].find((b) => b.textContent === "五子棋").click(); });
  await page.waitForTimeout(350);
  // 15 路的 viewBox 不同：点击映射改用 n=15
  clickPoint = (() => {
    const n = 15;
    return async (gx, gy) => {
      await page.evaluate(([gx, gy, n]) => {
        const svg = document.querySelector('svg[role="grid"]');
        const r = svg.getBoundingClientRect();
        const vb = 60 + (n - 1) * 36;
        const cx = r.left + ((30 + gx * 36) / vb) * r.width;
        const cy = r.top + ((30 + gy * 36) / vb) * r.height;
        for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
          const ev = type.startsWith("pointer") ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 }) : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
          svg.dispatchEvent(ev);
        }
      }, [gx, gy, n]);
      await page.waitForTimeout(160);
    };
  })();
  for (const [x, y] of [[3, 7], [3, 8], [4, 7], [4, 8], [5, 7], [5, 8], [6, 7], [6, 8], [7, 7]]) await clickPoint(x, y);
  await page.waitForTimeout(300);
  const bodyText = await page.evaluate(() => document.body.innerText);
  check("7. 五子棋黑五连判胜（wasm）", bodyText.includes("黑 胜"));
  await page.screenshot({ path: `${SHOTS}/gomoku-win.png` });

  console.log(errors === 0 ? "无 JS 报错" : `有 ${errors} 个 JS 报错`);
  await browser.close();
  process.exit(0);
})().catch((e) => { console.error("CRASH:", e); process.exit(2); });
