/**
 * agent-web.js —— 「Agent 对战·内置驱动」Web 端全链 e2e（Mock LLM，阶段⑤）。
 *
 * 与 agent-builtin.js（桌面壳）镜像同一组关键链断言，但跑在**纯浏览器**里：
 * transport wasm 产物（agent feature）+ AgentPage web 内置路（createDetachedSession
 * 的 A' + Rust 侧 Hub 的 B）+ 本地 Anthropic 协议桩 agent-mock-llm.js 扮演对手模型。
 * 真实 LLM 端点/key 一律不入库——本脚本只认 127.0.0.1 随机口的本地桩。
 *
 * 流程：
 *   1) 前置：frontend/dist 已构建（wasm 产物须带 agent 导出——build-transport.sh
 *      默认带 agent feature）；本脚本自起 serve.js（5173，占用自动换 5174）；
 *   2) 起本地 mock 桩（127.0.0.1 随机口，CORS 全开）；
 *   3) 开浏览器：addInitScript 预置 localStorage（浏览器后端 = localStorage，store.ts）
 *      的 llm-config/llm-key/server-sel → 直达 /agent。**key 必须 ASCII**：浏览器
 *      fetch 的头值是 ByteString（ISO-8859-1），非 ASCII 字符直接 TypeError——
 *      壳版剧本里的中文假 key 在 web 通道是非法输入（reqwest 的 obs-text 字节
 *      通道在浏览器不存在），与真实 key 恒为 ASCII 的事实一致；
 *   4) 等 webOk 启用面 → 点开始对局 → 断言镜像 agent-builtin.js：配对完成、Agent 落子
 *      （幽灵子先现后实子）、工具日志增长、聊天往返、拦截面（局中主会话 createInvite
 *      被拒）、Agent 认输终局与用量收口。
 *
 * 用法：node agent-web.js [--no-build]（占位保持与 agent-builtin 同形）[端口]   默认 5173
 */
const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");
const { Endpoint } = require("./device.js");
const { startStub } = require("./agent-mock-llm.js");

const ARGS = process.argv.slice(2);
const PORT_ARG = ARGS.find((a) => /^\d+$/.test(a)) || "5173";
const ROOT = path.resolve(__dirname, "..", "..");
const DIST = path.join(ROOT, "frontend", "dist");
const SHOTS = "G:\\tmp\\shots";

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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

/** 等页面文案出现。超时必须把现场文本带出来——光报超时没法归因。 */
async function waitText(ep, text, timeout, label) {
  try {
    await ep.page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout, polling: 300 });
    return true;
  } catch {
    const dump = await ep.page.evaluate(() => document.body.innerText).catch(() => "(evaluate 失败)");
    console.log(`[diag] 等「${label || text}」超时(${timeout}ms)。页面文本：\n-----8<-----\n${String(dump).slice(0, 2500)}\n----->8-----`);
    return false;
  }
}

/** 读「手数 N」（AgentPage 对局卡内）。 */
async function moveCount(ep) {
  return ep.page.evaluate(() => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });
}

/** 等手数达到 n。 */
function waitMoves(ep, n, timeout = 45000) {
  return ep.page.waitForFunction((x) => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) >= x : false;
  }, n, { timeout, polling: 300 }).then(() => true).catch(() => false);
}

/** 落子（真实鼠标事件序列；与 agent-builtin.js 同一实现）。 */
async function place(ep, x, y) {
  await ep.page.locator('svg[role="grid"]').first().scrollIntoViewIfNeeded().catch(() => {});
  await ep.page.evaluate(([gx, gy]) => {
    const svg = document.querySelector('svg[role="grid"]');
    const r = svg.getBoundingClientRect();
    const vb = svg.viewBox.baseVal.width;
    const n = Number((svg.getAttribute("aria-label") || "").match(/(\d+)x\d+/)?.[1]) || 15;
    const pad = 30, cell = (vb - pad * 2) / (n - 1);
    const cx = r.left + (pad + gx * cell) * (r.width / vb);
    const cy = r.top + (pad + gy * cell) * (r.height / vb);
    for (const type of ["pointermove", "pointerdown", "pointerup", "click"]) {
      const ev = type.startsWith("pointer")
        ? new PointerEvent(type, { bubbles: true, clientX: cx, clientY: cy, pointerId: 1 })
        : new MouseEvent(type, { bubbles: true, clientX: cx, clientY: cy });
      svg.dispatchEvent(ev);
    }
  }, [x, y]);
}

/** 发聊天（面板常开在侧栏；真实键入 + 真实点击发送）。 */
async function sendChat(ep, text) {
  const input = ep.page.locator('input[placeholder="说点什么…"]:visible').first();
  await input.waitFor({ state: "visible", timeout: 15000 });
  await input.click();
  await input.fill("");
  await input.type(text, { delay: 10 });
  await ep.page.locator("button:visible", { hasText: "发送" }).first().click();
}

/** 落一手并等手数推进到 expect；未推进就重点（配对刚收口的点击会被吞）。 */
async function placeAndWait(ep, x, y, expect, timeoutPerTry = 12000) {
  for (let i = 0; i < 3; i++) {
    await place(ep, x, y);
    if (await waitMoves(ep, expect, timeoutPerTry)) return true;
    console.log(`[diag] 落子 (${x},${y}) 未推进到手数 ${expect}（第 ${i + 1} 次），重试`);
  }
  return false;
}

/** 失败现场全景：页面全文（Agent 状态卡的 state/detail 在里面）+ 平台存储快照。
 *  Web 端 agent_status 的窗口就是状态卡本体（页面轮询 agent_events/status 渲染）。 */
async function diagDump(ep) {
  const text = await ep.page.evaluate(() => document.body.innerText).catch(() => "(evaluate 失败)");
  console.log(`[diag] 页面文本：\n-----8<-----\n${String(text).slice(0, 3000)}\n----->8-----`);
  const store = await ep.page.evaluate(() => JSON.stringify(window.__store?.dump?.() ?? {})).catch(() => "{}");
  console.log(`[diag] store: ${store.slice(0, 600)}`);
}

(async () => {
  let exitCode;
  fs.mkdirSync(SHOTS, { recursive: true });
  if (!fs.existsSync(path.join(DIST, "index.html"))) {
    throw new Error(`缺少 ${DIST}——先构建前端（transport wasm 产物须带 agent 导出）`);
  }

  // —— 1) mock 桩（随机口；剧本参数与 agent-builtin.js 同一套） ——
  const stub = await startStub({ myColor: "white", resignAfter: 3, waitMs: 700, stageMs: 1500 });
  console.log(`[mock-llm] ${stub.url}/messages`);
  const llmConfig = JSON.stringify({
    protocol: "anthropic", baseUrl: stub.url, model: "mock-1",
    maxOutputTokens: 1024, replyLang: "简体中文", enableSubagent: false,
  });

  // —— 2) 起 serve（5173 占用换 5174，与 agent-entry.js 同一手 法） ——
  let server = await serve(PORT_ARG);
  let port = PORT_ARG;
  if (server.portBusy) {
    port = PORT_ARG === "5173" ? "5174" : "5173";
    console.log(`端口 ${PORT_ARG} 被占，改用 ${port}`);
    server = await serve(port);
    if (server.portBusy) { await stub.close().catch(() => {}); throw new Error(`5173/5174 均被占用`); }
  }
  const origin = `http://127.0.0.1:${port}`;
  console.log(`origin: ${origin}`);

  // —— 3) 开浏览器：预置 localStorage 后直达 /agent（浏览器后端 = localStorage） ——
  let ep = null;
  const consoleLog = [];
  try {
    ep = await Endpoint.browser("web", null);
    ep.page.on("console", (m) => consoleLog.push(`[${m.type()}] ${m.text().slice(0, 300)}`));
    // addInitScript 在页面任何脚本前跑：store 门面首拍就能读到配置（与
    // setSetting+reload 等效，但少一次导航；同源页共享 localStorage）。
    await ep.page.addInitScript(([cfg, key]) => {
      localStorage.setItem("goptop:server-sel", "none");
      localStorage.setItem("goptop:name", "web甲");
      localStorage.setItem("goptop:userId", "u-agent-web-0001");
      localStorage.setItem("goptop:llm-config", cfg);
      localStorage.setItem("goptop:llm-key", key);
    }, [llmConfig, "stub-key-e2e-local-mock-only"]);
    await ep.page.goto(`${origin}/agent`, { waitUntil: "domcontentloaded" });
    await ep.ready(40000);

    // —— 4) 等 webOk 启用面（首帧横幅闪一拍，以控件启用为就绪信号） ——
    const enabled = await ep.page.waitForFunction(() => {
      const scope = document.querySelector(".play-stack");
      const b = scope && [...scope.querySelectorAll("button")].find((x) => x.textContent.includes("开始对局"));
      return !!b && !b.disabled;
    }, null, { timeout: 20000, polling: 200 }).then(() => true).catch(() => false);
    check("Web 内置模式启用（webOk 就绪）", enabled);
    if (!enabled) {
      await diagDump(ep);
      throw new Error("webOk 未就绪，终止");
    }

    // —— 5) 开局 ——
    await ep.page.locator("button:visible", { hasText: "开始对局" }).first().click();
    console.log("[game] 已点开始，等配对…");
    const playing = await waitText(ep, "黑 落子", 120000, "配对完成（黑 落子）");
    check("配对完成、轮到人类（我执黑先行）", playing);
    if (!playing) {
      await diagDump(ep);
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-web-pairing.png"), fullPage: true }).catch(() => {});
      throw new Error("配对失败，终止（详见上方诊断与截图）");
    }

    // 第 1 手：人类 (7,7) → Agent 打招呼 + 应手（先撞占点被拒再纠正的剧本与壳版一致）
    check("人类首手落盘", await placeAndWait(ep, 7, 7, 1, 20000), `手数=${await moveCount(ep)}`);
    check("Agent 开局打招呼（聊天在案）", await waitText(ep, "请多指教", 40000, "开场 chat"));
    // 幽灵子先现（拟落 N,M + 橙圈 r=15）→ 提交后实子上盘（手数=2，随后拟落清空）。
    // 圈选择器必须带 r=15：#FF8C1A 还被悬停高亮与最后一手标记使用（壳版实测踩过）。
    const GHOST = 'svg circle[r="15"][stroke="#FF8C1A"]';
    const ghostShown = await ep.page.waitForFunction((sel) =>
      document.body.innerText.includes("拟落") && !!document.querySelector(sel),
    GHOST, { timeout: 40000, polling: 150 }).then(() => true).catch(() => false);
    const ghostText = await ep.page.evaluate(() => document.body.innerText.match(/拟落 \d+,\d+/)?.[0] ?? null);
    check("暂存幽灵子先现（拟落坐标 + 盘面橙圈）", ghostShown, ghostText ?? "未见");
    const mc2 = await waitMoves(ep, 2, 30000);
    const ghostGone = await ep.page.waitForFunction((sel) =>
      !document.body.innerText.includes("拟落") && !document.querySelector(sel),
    GHOST, { timeout: 8000, polling: 150 }).then(() => true).catch(() => false);
    check("提交后实子上盘（手数=2）", mc2, `手数=${await moveCount(ep)}`);
    check("提交后幽灵子消失", ghostGone);

    // 工具日志与用量：操作式日志（Read/Submit）在案、llm HTTP 行不上屏、调用计数 > 0
    const stat = await ep.page.evaluate(() => {
      const t = document.body.innerText;
      return {
        calls: Number(t.match(/LLM 调用 (\d+)/)?.[1] ?? -1),
        opLog: /(Read|Write|Wait)\(/.test(t),
        llmLineGone: !/\[llm\]/.test(t),
        moveLog: /Submit\(落子/.test(t),
        chatLog: /Submit\(发送消息/.test(t),
      };
    });
    check("工具日志为操作式（Read/Write/Wait 在案）", stat.calls > 0 && stat.opLog, `LLM 调用=${stat.calls} 操作行=${stat.opLog}`);
    check("llm HTTP 行不再上屏", stat.llmLineGone, `llmLineGone=${stat.llmLineGone}`);
    check("工具日志含落子/聊天动作行", stat.moveLog || stat.chatLog, JSON.stringify(stat));

    // 聊天往返：人发一句，Agent 剧本回一句
    await sendChat(ep, "你好 Agent，这一局请指教");
    check("聊天往返（Agent 回复在案）", await waitText(ep, "收到", 30000, "Agent 聊天回复"));

    // 拦截面：Agent 局存活期间，主会话（window.__session）的开局命令必须被拒。
    // Web 的 wasm 导出不回值——断言两路：goptopNotice 收到「Agent 对局进行中」提示
    // （拦截面契约文案），且主会话快照不产生含 rtc 的邀请链接（命令确未入队）。
    // snapshot() 在 web 是 JSON 串（WasmSessionAdapter 直通 wasm 导出），必须
    // 解析后取 inviteUrl——对串直取属性恒 undefined，「未入队」一腿会恒真。
    const intercepted = await ep.page.evaluate(async () => {
      const s = window.__session;
      if (!s) return { ok: false, error: "__session 不在场" };
      const parseSnap = (t) => { try { return JSON.parse(t); } catch { return null; } };
      const hasInvite = (snap) => {
        const u = snap && typeof snap === "object" ? snap.inviteUrl : null;
        return !!u && String(u).includes("rtc=");
      };
      const hadInvite = hasInvite(parseSnap(s.snapshot()));
      let noticed = null;
      const w = window;
      const orig = w.goptopNotice;
      w.goptopNotice = (arg) => {
        try { noticed = (JSON.parse(arg).text ?? "") + ""; } catch { noticed = String(arg); }
        if (typeof orig === "function") orig(arg); // 保真透传（提示条照常出）
      };
      try { s.create_invite(); } finally { w.goptopNotice = orig; }
      await new Promise((r) => setTimeout(r, 300));
      const gainedInvite = !hadInvite && hasInvite(parseSnap(s.snapshot()));
      return { ok: !gainedInvite && (noticed ?? "").includes("Agent 对局进行中"), noticed, gainedInvite };
    });
    check("拦截面：局中主会话 createInvite 被拒（notice 文案 + 未入队）",
      intercepted.ok === true, JSON.stringify(intercepted));

    // 第 2/3 手（有来有回），第 4 手后 Agent 剧本认输
    check("第 2 手应手落盘", await placeAndWait(ep, 3, 3, 4), `手数=${await moveCount(ep)}`);
    check("第 3 手应手落盘", await placeAndWait(ep, 11, 11, 6), `手数=${await moveCount(ep)}`);
    check("第 4 手（认输触发）落盘", await placeAndWait(ep, 5, 9, 7), `手数=${await moveCount(ep)}`);

    const over = await waitText(ep, "黑 胜", 60000, "终局（黑 胜）");
    const done = await waitText(ep, "已终局", 8000, "Agent 状态卡（已终局）");
    check("终局：白方认输、黑胜渲染", over, `手数=${await moveCount(ep)}`);
    check("Agent 状态卡收口为「已终局」", done);
    const finalStat = await ep.page.evaluate(() => {
      const t = document.body.innerText;
      return {
        calls: Number(t.match(/LLM 调用 (\d+)/)?.[1] ?? -1),
        tin: Number(t.match(/输入 (\d+) tok/)?.[1] ?? -1),
        tout: Number(t.match(/输出 (\d+) tok/)?.[1] ?? -1),
      };
    });
    check("终局用量在案（LLM 调用/输入/输出 tokens）",
      finalStat.calls > 0 && finalStat.tin > 0 && finalStat.tout > 0, JSON.stringify(finalStat));
    const stubStats = stub.stats();
    check("mock 桩侧口径吻合（完成 3 手应手后认输）",
      stubStats.myMoves >= 3, JSON.stringify(stubStats));

    if (fail > 0) {
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-web-fail.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 截图：${path.join(SHOTS, "agent-web-fail.png")}`);
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    exitCode = fail > 0 ? 1 : 0;
  } catch (e) {
    console.error("E2E CRASH:", e);
    if (ep) {
      await diagDump(ep).catch(() => {});
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-web-crash.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    exitCode = 2;
  } finally {
    // 收尾必须真跑（process.exit 会跳过 finally——serve/桩就都留尸了）
    if (ep) await ep.close().catch(() => {});
    if (server && typeof server.kill === "function") server.kill();
    await stub.close().catch(() => {});
  }
  process.exit(exitCode ?? 2);
})().catch((e) => { console.error("E2E CRASH:", e); process.exit(2); });
