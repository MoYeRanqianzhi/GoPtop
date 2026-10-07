/**
 * ai.js — 人机对战（`/ai`）与实时胜率的实机验证。
 *
 * 全部交互走**真实输入**（真实鼠标点击），DOM 只用于读取断言值。
 * 断言刻意选「能区分对错」的量：手数、红蓝条宽度、走势图点数——而不是
 * 「元素存在」这类恒真检查。
 *
 * 用法：
 *   node ai.js [端] [URL]
 *   端：web（默认）/ mob / android / cdp:<url>
 *   URL 默认 http://localhost:1420/
 *
 * 依赖：AI wasm 已构建（bash scripts/build-ai-wasm.sh）。
 */
const { Endpoint } = require("./device.js");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let passed = 0;
let failed = 0;

function check(name, cond, extra = "") {
  if (cond) { passed++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { failed++; console.log(`FAIL ${name}${extra ? " — " + extra : ""}`); }
}

/** 对局卡上的手数（读不到返回 -1）。 */
async function moveCount(ep) {
  return ep.page.evaluate(() => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });
}

/** 切到底部的胜率卡。
 *  宽屏卡与窄屏 `.bp-swap` 由容器查询互斥显示，隐藏的那个点不了——必须按可见性选，
 *  `count()` 对 display:none 的元素照样返回非零（第一版就栽在这）。 */
async function openOdds(ep) {
  const narrow = ep.page.locator(".bp-swap button");
  if (await narrow.isVisible().catch(() => false)) {
    for (let i = 0; i < 2; i++) { await narrow.click(); await sleep(700); }
  } else {
    const wide = ep.page.locator('button[title*="对局 / 胜率"]');
    if (await wide.isVisible().catch(() => false)) await wide.click();
    await sleep(700);
  }
  await sleep(3000);
}

async function main() {
  const spec = process.argv[2] ?? "web";
  const base = process.argv[3] ?? "http://localhost:1420/";
  const url = base.replace(/\/$/, "") + "/ai";

  // 壳端点（android / 鸿蒙 cdp）连的是**已经在跑的**应用，启动时停在菜单页，
  // 所以照样要导航到 /ai——SPA 路由能处理路径（P2P 的 e2e 同样这么进子页面）。
  // 注意 android 不能用外部的 base：模拟器访问不到宿主机的 localhost，得用应用
  // 自己加载资源的 origin（Tauri 是 http://tauri.localhost，鸿蒙是 appassets.goptop）。
  let ep;
  let target = url;
  if (spec === "android" || spec.startsWith("android:")) {
    ep = await Endpoint.android("ai", { serial: spec.includes(":") ? spec.slice(8) : null });
    target = new URL(ep.page.url()).origin + "/ai";
  } else if (spec.startsWith("cdp:")) {
    ep = await Endpoint.cdp("ai", spec.slice(4));
    // 壳端点各自的资源 origin 不同（鸿蒙 appassets.goptop、桌面 tauri.localhost），
    // 没显式给 base 时就从当前页面推导，免得每端都要记一个地址
    if (!process.argv[3]) target = new URL(ep.page.url()).origin + "/ai";
  } else if (spec === "mob") {
    ep = await Endpoint.browser("ai", url, { mobile: true });
  } else {
    ep = await Endpoint.browser("ai", url);
  }
  await ep.goto(target).catch(() => { /* 浏览器端点构造时已导航过，重复导航失败可忽略 */ });
  // 被吞的导航错误要用落地位置兜住：壳端点只接管不导航，这条是进 /ai 的唯一导航，
  // 静默失败会让后续落子全落在旧页面上（stress 实测过 171 手横坐标 -37 的假失败）
  const landed = ep.page.url();
  if (landed !== target) throw new Error(`[ai] 导航后停在 ${landed}，未进入 ${target}——导航失败被忽略`);
  await ep.ready(40000);

  // —— 1) 人类落子 → AI 自动应手 ——
  // 首次分析含 NNUE 权重解压（约 56ms）+ 搜索，留足余量
  await ep.place(7, 7);
  await sleep(8000);
  let moves = await moveCount(ep);
  check("人类落子后 AI 自动应手", moves === 2, `手数=${moves}`);

  // 再走一个回合：走势图要两个点才画得出线，只有一手时显示的是占位文案
  await ep.place(8, 8);
  await sleep(8000);
  moves = await moveCount(ep);
  check("第二个回合照常（人类落子 + AI 应手）", moves === 4, `手数=${moves}`);

  // —— 2) 胜率卡：红蓝条 + 走势图 ——
  await openOdds(ep);
  const odds = await ep.page.evaluate(() => {
    const scopes = [".bp-swap", ".bp-wide"];
    for (const s of scopes) {
      for (const el of document.querySelectorAll(s)) {
        if (el.innerText.includes("胜率")) {
          return {
            text: el.innerText.replace(/\n/g, " "),
            barW: document.querySelector(".wr-fill--mine")?.style.width ?? null,
            pts: document.querySelector(".wr-chart-line")?.getAttribute("points") ?? "",
            pending: !!document.querySelector(".wr-track--pending"),
          };
        }
      }
    }
    return { text: "", barW: null, pts: "", pending: false };
  });
  check("胜率卡给出了百分比", /%/.test(odds.text), odds.text.slice(0, 60));
  check("红蓝条宽度已按胜率设置", odds.barW !== null && !odds.pending, `width=${odds.barW}`);
  check("走势图已画出数据点", odds.pts.includes(","), `points=${odds.pts.slice(0, 40)}`);

  // —— 3) 设置面板收拢了各项功能 ——
  await ep.page.locator('.play-stack button[aria-label="对局设置"]').click();
  await sleep(700);
  const panel = await ep.page.evaluate(() => document.querySelector(".chat-modal")?.innerText ?? "");
  check(
    "设置面板含 难度/先后手/悔棋/重开",
    ["难度", "先后手", "悔棋", "重开"].every((k) => panel.includes(k)),
    panel.replace(/\n/g, " ").slice(0, 70),
  );
  const modalH = await ep.page.evaluate(() => Math.round(document.querySelector(".chat-modal")?.getBoundingClientRect().height ?? 0));
  // 面板是内容自适应高度：照搬聊天弹窗的固定 560px 会撑出一大片空白
  check("设置面板高度自适应（未撑满）", modalH > 0 && modalH < 480, `height=${modalH}px`);

  // —— 4) 换边：AI 执黑后立刻落子 ——
  await ep.page.locator('button[title*="交换先后手"]').click();
  await sleep(8000);
  const status = await ep.page.evaluate(() => document.querySelector(".play-stack .brutal-card")?.innerText.replace(/\n/g, " ") ?? "");
  check("换边后人类执白", status.includes("你执白"), status);
  // 手数只在「对局」卡里，而此刻底部停在胜率卡上——改从状态行判断：
  // 黑先，若 AI 已落子则轮白（「白 落子」），没落子会停在「黑 落子」
  check("AI 执黑后立刻落第一手", status.includes("白 落子"), status);

  // —— 5) AI 执黑思考中点「重开」：AI 仍要接手（死局回归） ——
  // 依赖里没有代次时，思考中重开五个依赖一个不变、effect 不重跑——空盘无人落子
  // （旧回执落子的幽灵子修掉后才暴露）。选「强」档（3s 思考窗）：此刻 AI 执黑、
  // 轮人类，人类落一手让 AI 进入思考，思考中立刻重开——稳定落在窗口内。
  // 修复后 AI 会重新思考并落第一手：手数=1、轮白。
  // 底部还停在胜率卡（第 2 节切过去的）：先切回对局视图，手数才读得到。
  await ep.page.locator('button[title*="对局 / 胜率"]').click();
  await sleep(700);
  await ep.page.locator('.play-stack button[aria-label="对局设置"]').click();
  await sleep(500);
  await ep.page.locator('button[title="每步思考 3000 毫秒"]').click();
  await sleep(300);
  await ep.place(9, 9); // AI（黑）开始思考，3s 窗口
  await ep.page.locator('.play-stack button[aria-label="对局设置"]').click(); // 思考中重开面板
  await ep.page.locator(".chat-modal button", { hasText: "重开" }).click(); // 落在思考窗内
  await sleep(12000);
  const afterReset = await ep.page.evaluate(() => document.querySelector(".play-stack .brutal-card")?.innerText.replace(/\n/g, " ") ?? "");
  const resetMoves = await moveCount(ep);
  check("AI 执黑思考中重开仍接手（死局回归）", resetMoves === 1 && afterReset.includes("白 落子"), `手数=${resetMoves} | ${afterReset.slice(0, 50)}`);

  console.log(`\n${passed} passed, ${failed} failed`);
  await ep.close();
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
