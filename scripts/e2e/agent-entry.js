/**
 * agent-entry.js —— 「Agent 对战」入口与降级面的浏览器基线（Web 5173/5174）。
 *
 * 覆盖（阶段③集成门禁的入口部分）：
 * 1. 菜单页恰好一个「Agent 对战」入口，点击真进 /agent；
 * 2. Web（非 Tauri）整页降级：横幅在、MCP 卡/驱动按钮不渲染、功能控件全部禁用；
 * 3. 设置卡（内置）形态正确：LLM 配置各字段在场、内置驱动按钮选中。
 *
 * 交互全部走真实点击；断言读 DOM 文案/禁用态（与 ai.js 同一口径）。
 * 前置：frontend/dist 已构建；本脚本自起 serve.js（复用基建，端口可传）。
 *
 * 用法：node agent-entry.js [端口]      默认 5173（占用时自动换 5174）
 */
const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");
const { Endpoint } = require("./device.js");

const PORT = process.argv[2] || "5173";
const ROOT = path.resolve(__dirname, "..", "..");
const DIST = path.join(ROOT, "frontend", "dist");

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}

/** 起 serve.js 静态服务器；EADDRINUSE 以 `portBusy` 标记返回（调用方换口重试）。 */
function serve(port) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [path.join(__dirname, "serve.js"), String(port), "127.0.0.1", DIST], { stdio: "pipe" });
    let out = "";
    p.stdout.on("data", (d) => { out += d; });
    p.stderr.on("data", (d) => { out += d; });
    p.on("exit", (code) => {
      if (/EADDRINUSE/i.test(out)) { resolve({ portBusy: true }); return; }
      reject(new Error(`serve.js 提前退出 rc=${code}: ${out}`));
    });
    // 就绪判据 = 服务器真能应答（光看 stdout 有进程竞态）
    const t0 = Date.now();
    const probe = () => {
      fetch(`http://127.0.0.1:${port}/`).then((r) => {
        if (r.ok) resolve(p);
        else retry();
      }).catch(retry);
    };
    const retry = () => {
      if (Date.now() - t0 > 8000) { p.kill(); reject(new Error(`serve.js 未就绪: ${out}`)); return; }
      setTimeout(probe, 250);
    };
    probe();
  });
}

(async () => {
  if (!fs.existsSync(path.join(DIST, "index.html"))) {
    throw new Error(`缺少 ${DIST}——先构建前端（cd frontend && npm run build）`);
  }
  let server = await serve(PORT);
  let port = PORT;
  if (server.portBusy) {
    port = PORT === "5173" ? "5174" : "5173";
    console.log(`端口 ${PORT} 被占，改用 ${port}`);
    server = await serve(port);
    if (server.portBusy) throw new Error(`5173/5174 均被占用`);
  }
  const origin = `http://127.0.0.1:${port}`;
  console.log(`origin: ${origin}`);
  const ep = await Endpoint.browser("web", `${origin}/`);
  try {
    await ep.ready(30000);

    // —— 1) 菜单恰好一个「Agent 对战」入口，点击真进 /agent ——
    const entries = ep.page.locator("button", { hasText: "Agent 对战" });
    const entryCount = await entries.count();
    check("菜单恰好一个「Agent 对战」入口", entryCount === 1, `count=${entryCount}`);
    await entries.first().click();
    await ep.page.waitForFunction(() => location.pathname === "/agent", null, { timeout: 8000, polling: 150 })
      .then(() => check("点击入口进入 /agent", true))
      .catch(() => check("点击入口进入 /agent", false, `停在 ${ep.page.url()}`));

    // —— 2) Web 降级面 ——
    await ep.page.waitForFunction(() => document.body.innerText.includes("Agent 对战 · 对局设置"), null, { timeout: 8000 })
      .catch(() => {});
    const banner = await ep.page.evaluate(() => document.body.innerText.includes("内置 Agent 对战仅桌面版可用"));
    check("Web 降级横幅在场", banner);
    const mcpDriverBtn = await ep.page.locator("button", { hasText: "MCP（外部 Agent）" }).count();
    check("MCP 驱动按钮不渲染（Web）", mcpDriverBtn === 0, `count=${mcpDriverBtn}`);
    const mcpCard = await ep.page.evaluate(() => document.body.innerText.includes("MCP 服务器（外部 Agent 经此认领对手席）"));
    check("MCP 连接卡不渲染（Web）", !mcpCard);

    const disabledStats = await ep.page.evaluate(() => {
      // 只看设置卡自身的控件：顶栏 KindSizePicker 是主会话的（/agent 页仍挂载），
      // 它的五子棋/围棋按钮不受本页降级管——不圈作用域会把「头部的可用按钮」误计进来。
      const scope = document.querySelector(".play-stack");
      const byText = (t) => [...scope.querySelectorAll("button")].filter((b) => b.textContent.includes(t));
      return {
        start: byText("开始对局").every((b) => b.disabled),
        kinds: byText("五子棋").concat(byText("围棋")).every((b) => b.disabled),
        colors: byText("我执黑").concat(byText("我执白")).every((b) => b.disabled),
        builtinDriver: byText("内置（LLM 循环）").every((b) => b.disabled),
        protocols: byText("Anthropic").every((b) => b.disabled),
        testConn: byText("测试连接").every((b) => b.disabled),
        baseUrl: [...scope.querySelectorAll("input")].filter((i) => i.placeholder.includes("your-gateway")).every((i) => i.disabled),
      };
    });
    check("开始对局禁用", disabledStats.start);
    check("棋种按钮禁用", disabledStats.kinds);
    check("执子按钮禁用", disabledStats.colors);
    check("驱动按钮禁用", disabledStats.builtinDriver);
    check("协议按钮禁用", disabledStats.protocols);
    check("测试连接禁用", disabledStats.testConn);
    check("Base URL 输入禁用", disabledStats.baseUrl);

    // —— 3) 设置卡（内置）形态 ——
    const cardText = await ep.page.evaluate(() => document.body.innerText);
    for (const label of ["对手驱动", "内置 LLM 配置", "Base URL", "模型", "API Key", "上下文上限", "回复语言", "子代理"]) {
      check(`设置卡含「${label}」`, cardText.includes(label));
    }
    const builtinPressed = await ep.page.evaluate(() => {
      const b = [...document.querySelectorAll("button")].find((x) => x.textContent.includes("内置（LLM 循环）"));
      return b?.getAttribute("aria-pressed");
    });
    check("内置驱动按钮为选中态", builtinPressed === "true", `aria-pressed=${builtinPressed}`);

    // 路由直达 /agent 同样落降级面（刷新/直链用户路径）
    await ep.goto(`${origin}/agent`);
    await ep.ready(30000);
    const direct = await ep.page.evaluate(() => document.body.innerText.includes("内置 Agent 对战仅桌面版可用"));
    check("直达 /agent 同样降级", direct);

    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    process.exit(fail > 0 ? 1 : 0);
  } finally {
    await ep.close().catch(() => {});
    server.kill();
  }
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
