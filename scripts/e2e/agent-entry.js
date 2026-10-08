/**
 * agent-entry.js —— 「Agent 对战」入口与启用/降级两面的浏览器基线（Web 5173/5174）。
 *
 * 覆盖（阶段⑤集成后 Web 内置模式已上线，本脚本改测「web 内置可用」形态）：
 * 1. 菜单页恰好一个「Agent 对战」入口，点击真进 /agent；
 * 2. Web（非 Tauri）**启用面**：降级横幅不在、MCP 卡/驱动按钮仍不渲染（桌面专属）、
 *    设置卡功能控件全部可用（webOk 判定四道全过：产物带 agent 导出 + Vfs 钩子已装）；
 * 3. 设置卡（内置）形态正确：LLM 配置各字段在场、内置驱动按钮选中；
 * 4. **降级回归路径**：装上鸿蒙壳同款 goptopHost 桥（harmonyHost 判定命中）后
 *    横幅重新在场——鸿蒙降级面（主计划 R7）仍有断言可依。
 *
 * 交互全部走真实点击；断言读 DOM 文案/禁用态（与 ai.js 同一口径）。
 * 前置：frontend/dist 已构建（transport wasm 产物须带 agent 导出）；本脚本自起
 * serve.js（复用基建，端口可传）。
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
  // 资源句柄提到 try 外：起服务器/开浏览器任何一步 throw，finally 都要能收尾，
  // 否则 serve.js 子进程留尸占着 5173/5174（process.exit 会跳过 finally——
  // agent-builtin.js 同一条教训：先收尾，退出码收尾后再落）。
  let server = null;
  let ep = null;
  let epDegraded = null;
  let exitCode;
  try {
    server = await serve(PORT);
    let port = PORT;
    if (server.portBusy) {
      port = PORT === "5173" ? "5174" : "5173";
      console.log(`端口 ${PORT} 被占，改用 ${port}`);
      server = await serve(port);
      if (server.portBusy) throw new Error(`5173/5174 均被占用`);
    }
    const origin = `http://127.0.0.1:${port}`;
    console.log(`origin: ${origin}`);
    ep = await Endpoint.browser("web", `${origin}/`);
    await ep.ready(30000);

    // —— 1) 菜单恰好一个「Agent 对战」入口，点击真进 /agent ——
    const entries = ep.page.locator("button", { hasText: "Agent 对战" });
    const entryCount = await entries.count();
    check("菜单恰好一个「Agent 对战」入口", entryCount === 1, `count=${entryCount}`);
    await entries.first().click();
    await ep.page.waitForFunction(() => location.pathname === "/agent", null, { timeout: 8000, polling: 150 })
      .then(() => check("点击入口进入 /agent", true))
      .catch(() => check("点击入口进入 /agent", false, `停在 ${ep.page.url()}`));

    // —— 2) Web 启用面（webOk 异步置位：先等「开始对局」变可用，再断言横幅不在） ——
    // 首帧 webOk=false 会先渲染一拍横幅，直接断言会撞上闪烁——以控件启用为就绪信号。
    const enabled = await ep.page.waitForFunction(() => {
      const scope = document.querySelector(".play-stack");
      const b = scope && [...scope.querySelectorAll("button")].find((x) => x.textContent.includes("开始对局"));
      return !!b && !b.disabled;
    }, null, { timeout: 20000, polling: 200 }).then(() => true).catch(() => false);
    check("Web 内置模式启用（webOk 四道判定通过）", enabled);
    const banner = await ep.page.evaluate(() => document.body.innerText.includes("内置 Agent 对战仅桌面版可用"));
    check("降级横幅不在场（webOk 下不渲染）", !banner);
    const mcpDriverBtn = await ep.page.locator("button", { hasText: "MCP（外部 Agent）" }).count();
    check("MCP 驱动按钮不渲染（桌面专属）", mcpDriverBtn === 0, `count=${mcpDriverBtn}`);
    const mcpCard = await ep.page.evaluate(() => document.body.innerText.includes("MCP 服务器（外部 Agent 经此认领对手席）"));
    check("MCP 连接卡不渲染（桌面专属）", !mcpCard);

    const enabledStats = await ep.page.evaluate(() => {
      // 只看设置卡自身的控件：顶栏 KindSizePicker 是主会话的（/agent 页仍挂载），
      // 它的五子棋/围棋按钮不受本页管——不圈作用域会把「头部的按钮」误计进来。
      const scope = document.querySelector(".play-stack");
      const byText = (t) => [...scope.querySelectorAll("button")].filter((b) => b.textContent.includes(t));
      // 空数组的 every 恒真——控件被回归删掉时断言会假 PASS，先断 count >= 1。
      const allEnabled = (list) => list.length > 0 && list.every((el) => !el.disabled);
      return {
        start: allEnabled(byText("开始对局")),
        kinds: allEnabled(byText("五子棋").concat(byText("围棋"))),
        colors: allEnabled(byText("我执黑").concat(byText("我执白"))),
        builtinDriver: allEnabled(byText("内置（LLM 循环）")),
        protocols: allEnabled(byText("Anthropic")),
        testConn: allEnabled(byText("测试连接")),
        baseUrl: allEnabled([...scope.querySelectorAll("input")].filter((i) => i.placeholder.includes("your-gateway"))),
      };
    });
    check("开始对局可用", enabledStats.start);
    check("棋种按钮可用", enabledStats.kinds);
    check("执子按钮可用", enabledStats.colors);
    check("驱动按钮可用", enabledStats.builtinDriver);
    check("协议按钮可用", enabledStats.protocols);
    check("测试连接可用", enabledStats.testConn);
    check("Base URL 输入可用", enabledStats.baseUrl);

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

    // 路由直达 /agent 同样落启用面（刷新/直链用户路径）
    await ep.goto(`${origin}/agent`);
    await ep.ready(30000);
    const directEnabled = await ep.page.waitForFunction(() => {
      const scope = document.querySelector(".play-stack");
      const b = scope && [...scope.querySelectorAll("button")].find((x) => x.textContent.includes("开始对局"));
      return !!b && !b.disabled;
    }, null, { timeout: 20000, polling: 200 }).then(() => true).catch(() => false);
    const directBanner = await ep.page.evaluate(() => document.body.innerText.includes("内置 Agent 对战仅桌面版可用"));
    check("直达 /agent 同样启用（无降级横幅）", directEnabled && !directBanner);

    // —— 4) 降级回归路径：鸿蒙壳同款 goptopHost 桥 → harmonyHost() 命中 → 整页降级 ——
    // 主计划 R7：鸿蒙壳内 wasm 产物虽在，但钩子通道/存储面未验收，维持整页降级。
    // addInitScript 必须在导航前装（goptopHost 在页面脚本跑之前就要在场）。
    const ctx = ep.page.context();
    const page2 = await ctx.newPage();
    await page2.addInitScript(() => {
      Object.defineProperty(window, "goptopHost", {
        value: { call: () => "", aiPost: () => Promise.resolve("{}"), storeSet: () => {}, storeLoad: () => "" },
        writable: false, configurable: true,
      });
    });
    epDegraded = new Endpoint("web-鸿蒙同判", page2, { mobile: false, tap: false });
    epDegraded._browser = { close: () => {} }; // 关页面即可，别把共用 browser 关了
    await page2.goto(`${origin}/agent`, { waitUntil: "domcontentloaded" });
    const degradedBanner = await page2.waitForFunction(
      () => document.body.innerText.includes("内置 Agent 对战仅桌面版可用"),
      null, { timeout: 20000, polling: 200 },
    ).then(() => true).catch(() => false);
    const degradedStart = await page2.evaluate(() => {
      const scope = document.querySelector(".play-stack");
      const b = scope && [...scope.querySelectorAll("button")].find((x) => x.textContent.includes("开始对局"));
      return b ? b.disabled : null;
    });
    check("鸿蒙桥在场 → 降级横幅重新在场（R7 面仍有断言）", degradedBanner);
    check("鸿蒙桥在场 → 开始对局禁用", degradedStart === true, `disabled=${degradedStart}`);

    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    exitCode = fail > 0 ? 1 : 0;
  } catch (e) {
    console.error("E2E CRASH:", e);
    exitCode = 2;
  } finally {
    if (epDegraded) { await epDegraded.page.close().catch(() => {}); }
    if (ep) await ep.close().catch(() => {});
    // serve 的失败路径可能只回 {portBusy} 标记（子进程已自己退出），kill 需判别
    if (server && typeof server.kill === "function") server.kill();
  }
  process.exit(exitCode ?? 2);
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
