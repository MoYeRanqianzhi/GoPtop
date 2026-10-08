/**
 * agent-builtin.js —— 「Agent 对战·内置驱动」桌面壳级全链 e2e（Mock LLM）。
 *
 * 全链 = AgentPage 开局（bind 代发邀请 → agent_start 结对）→ goptop-agent 决策循环
 * （本地 Anthropic 协议桩 agent-mock-llm.js 扮演对手模型）→ 落子上盘/聊天往返/认输终局。
 * 真实 LLM 实测是另一个专门阶段；本脚本只证**接线**，剧本按 agent-mock-llm.js 收口。
 *
 * 流程（参照 shell-pair.js / ai.js 与 .agents/docs/real-device-testing.md）：
 *   1) 【串行】构建壳（cargo build --release + custom-protocol；--no-build 跳过）；
 *   2) 起本地 mock 桩（127.0.0.1 随机口，无任何真实端点/key）；
 *   3) 起壳：独立 profile 环境（USERPROFILE/LOCALAPPDATA/APPDATA/WEBVIEW2_USER_DATA_FOLDER
 *      全部重定向到临时目录——不污染真实 ~/.goptop，也避开双实例同 profile 的坑）+ CDP 口；
 *   4) 经 CDP 预置 llm-config 指向 mock 桩 → 进 /agent → 设置卡双驱动切换断言 → 点开始；
 *   5) 断言：Agent 落子上盘（暂存幽灵子先现、后实子）→ 工具日志增长 → 聊天往返 →
 *      拦截面（Agent 局中主会话 createInvite 被拒）→ Agent 认输终局与 winner 渲染。
 *
 * 用法：node agent-builtin.js [--no-build] [cdp端口]     默认 9222
 */
const { spawn, execSync } = require("child_process");
const path = require("path");
const fs = require("fs");
const os = require("os");
const http = require("http");
const { Endpoint } = require("./device.js");
const { startStub } = require("./agent-mock-llm.js");

const ARGS = process.argv.slice(2);
const NO_BUILD = ARGS.includes("--no-build");
const CDP_PORT = ARGS.find((a) => /^\d+$/.test(a)) || "9222";
const ROOT = path.resolve(__dirname, "..", "..");
const EXE = path.join(ROOT, "target", "release", "goptop.exe");
const SHOTS = "G:\\tmp\\shots";

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 等 CDP 端口就绪（壳冷启动含 WebView2 初始化，留足 60s）。 */
function waitCdp(timeoutMs = 60000) {
  const t0 = Date.now();
  return new Promise((resolve, reject) => {
    const probe = () => {
      http.get(`http://127.0.0.1:${CDP_PORT}/json/version`, (r) => {
        r.resume();
        if (r.statusCode === 200) resolve();
        else retry();
      }).on("error", retry);
    };
    const retry = () => {
      if (Date.now() - t0 > timeoutMs) { reject(new Error(`CDP ${CDP_PORT} 端口 ${timeoutMs}ms 未就绪（壳没起来？）`)); return; }
      setTimeout(probe, 400);
    };
    probe();
  });
}

/** 等页面文案出现（轮询 innerText，壳页面上 __session 断言不可用——那是主会话）。
 *  超时必须把现场文本带出来——光报超时没法归因（第一次跑就栽在这）。 */
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

/** 读「手数 N」（对局卡内；AgentPage 与主页面同款文案）。 */
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

/** 落子（真实鼠标点击；不收聊天——Agent 页聊天是侧栏停靠栏，不遮棋盘）。 */
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

/** 落一手并等手数推进到 expect。statusText「黑/白 落子」不含 peerConnected 判据，
 *  配对刚收口时的点击会被 disabled 棋盘静默吞掉——以手数推进为准，未推进就重点。 */
async function placeAndWait(ep, x, y, expect, timeoutPerTry = 12000) {
  for (let i = 0; i < 3; i++) {
    await place(ep, x, y);
    if (await waitMoves(ep, expect, timeoutPerTry)) return true;
    console.log(`[diag] 落子 (${x},${y}) 未推进到手数 ${expect}（第 ${i + 1} 次），重试`);
  }
  return false;
}

/** 失败现场全景：Agent 运行状态（id 1..3 试探）+ 平台存储快照。
 *  配对链路的后半程全在 Rust 侧，页面 DOM 看不见——agent_status 的 detail 是
 *  唯一能说明「卡在哪一步」的窗口（邀请就绪/结队/配置错误都在 detail 里）。 */
async function diagDump(ep) {
  for (const id of [1, 2, 3]) {
    const st = await ep.page.evaluate(async (i) => {
      try {
        return await window.__TAURI_INTERNALS__.invoke("agent_status", { id: i });
      } catch (e) { return `(${String(e).slice(0, 80)})`; }
    }, id).catch((e) => `evaluate 失败: ${e}`);
    console.log(`[diag] agent_status(${id}) = ${typeof st === "string" ? st : JSON.stringify(st)}`);
  }
  const store = await ep.page.evaluate(() => JSON.stringify(window.__store?.dump?.() ?? {})).catch(() => "{}");
  console.log(`[diag] store: ${store.slice(0, 800)}`);
}

(async () => {
  let exitCode;
  fs.mkdirSync(SHOTS, { recursive: true });
  // —— 1) 串行构建壳（内存纪律：不与任何其它构建并发；本脚本余下步骤都在构建后才开始） ——
  if (!NO_BUILD) {
    console.log("[build] 串行构建壳（cargo build --release -p goptop --features tauri/custom-protocol）…");
    try { execSync("taskkill //F //IM goptop.exe", { stdio: "ignore" }); } catch { /* 没在跑就算了 */ }
    const rc = spawn("cargo", ["build", "--release", "-p", "goptop", "--features", "tauri/custom-protocol"], {
      cwd: ROOT, stdio: "inherit", shell: true,
    });
    const buildRc = await new Promise((r) => rc.on("exit", r));
    if (buildRc !== 0) throw new Error(`壳构建失败 rc=${buildRc}`);
  }
  if (!fs.existsSync(EXE)) throw new Error(`找不到 ${EXE}（先构建）`);

  // —— 2) mock 桩（随机口；剧本参数放这里调） ——
  const stub = await startStub({ myColor: "white", resignAfter: 3, waitMs: 700, stageMs: 1500 });
  console.log(`[mock-llm] ${stub.url}/messages`);
  const llmConfig = JSON.stringify({
    protocol: "anthropic", baseUrl: stub.url, model: "mock-1",
    maxOutputTokens: 1024, replyLang: "简体中文", enableSubagent: false,
  });

  // —— 3) 起壳（独立 profile + CDP） ——
  // 三个 profile 目录必须**先建再指**（shell-pair.js 头注的坑：WEBVIEW2 loader 还要
  // LOCALAPPDATA/APPDATA，指到不存在的目录＝壳静默不启动、调试端口也不开）。
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "goptop-agent-e2e-"));
  for (const d of ["", "AppData\\Local", "AppData\\Roaming"]) {
    fs.mkdirSync(path.join(tmp, d), { recursive: true });
  }
  try { execSync("taskkill //F //IM goptop.exe", { stdio: "ignore" }); } catch { /* 同上 */ }
  const shell = spawn(EXE, [], {
    cwd: ROOT,
    detached: false,
    stdio: ["ignore", "ignore", "ignore"],
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
      WEBVIEW2_USER_DATA_FOLDER: path.join(tmp, "wv"),
      USERPROFILE: tmp,
      LOCALAPPDATA: path.join(tmp, "AppData", "Local"),
      APPDATA: path.join(tmp, "AppData", "Roaming"),
    },
  });
  let ep = null;
  const consoleLog = [];
  try {
    await waitCdp();
    await sleep(1200); // 等首页挂载（CDP 口先于页面就绪）
    ep = await Endpoint.cdp("壳", `http://127.0.0.1:${CDP_PORT}`);
    ep.page.on("console", (m) => consoleLog.push(`[${m.type()}] ${m.text().slice(0, 300)}`));

    // —— 4) 预置配置（写平台存储后整页导航让 storeInit 重读）——
    await ep.setSetting("goptop:server-sel", "none");
    await ep.setSetting("goptop:name", "壳甲");
    await ep.setSetting("goptop:userId", "u-agent-e2e-0001");
    await ep.setSetting("goptop:llm-config", llmConfig);
    await ep.setSetting("goptop:llm-key", "stub-key-e2e（本地桩专用假 key）");
    const origin = new URL(ep.page.url()).origin;
    await ep.page.goto(origin + "/agent", { waitUntil: "domcontentloaded" });
    await ep.ready(40000);

    // —— 设置卡双驱动切换（仅桌面壳渲染 MCP 卡；Web 侧由 agent-entry.js 覆盖） ——
    const mcpBtn = ep.page.locator("button:visible", { hasText: "MCP（外部 Agent）" }).first();
    await mcpBtn.waitFor({ state: "visible", timeout: 10000 });
    await mcpBtn.click();
    check("切到 MCP：连接卡渲染", await waitText(ep, "MCP 服务器", 6000, "MCP 卡"));
    const portVal = await ep.page.locator('input[inputmode="numeric"]:visible').first().inputValue().catch(() => null);
    check("MCP 端口缺省 9537", portVal === "9537", `value=${portVal}`);
    // 开关是「当前态」文案（真实 store 的上轮残留会让初始态是已启用）——先归到
    // 已关闭再启到已开启，断言与起点无关。
    const toggleNow = await ep.page.evaluate(() => {
      const vis = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const b = [...document.querySelectorAll("button")].find((x) => vis(x) &&
        (x.textContent.includes("已关闭（点此启用）") || x.textContent.includes("已启用（点此关闭）")));
      return b?.textContent ?? null;
    });
    if (toggleNow?.includes("已启用")) {
      await ep.page.locator("button:visible", { hasText: "已启用（点此关闭）" }).first().click();
      await sleep(400);
    }
    await ep.page.locator("button:visible", { hasText: "已关闭（点此启用）" }).first().click();
    check("MCP 开关真启停（阶段④：状态行「服务器运行中」）",
      await waitText(ep, "状态：服务器运行中", 8000, "MCP 状态行"));
    const mcpKeys = await ep.getSetting("goptop:agent-mcp-enabled");
    const mcpToken = await ep.getSetting("goptop:agent-mcp-token");
    check("agent_mcp_set 落 store 键（enabled=true + token 已生成）",
      mcpKeys === "true" && !!mcpToken, `enabled=${mcpKeys} token=${mcpToken ? `${mcpToken.length} 字符` : "无"}`);
    await ep.page.locator("button:visible", { hasText: "内置（LLM 循环）" }).first().click();
    check("切回内置：LLM 配置卡渲染", await waitText(ep, "内置 LLM 配置", 6000, "内置卡"));

    // —— 5) 开局 ——
    await ep.page.locator("button:visible", { hasText: "开始对局" }).first().click();
    console.log("[game] 已点开始，等配对…");
    const playing = await waitText(ep, "黑 落子", 120000, "配对完成（黑 落子）");
    check("配对完成、轮到人类（我执黑先行）", playing);
    if (!playing) {
      await diagDump(ep);
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-builtin-pairing.png"), fullPage: true }).catch(() => {});
      throw new Error("配对失败，终止（详见上方诊断与截图）");
    }

    // 第 1 手：人类 (7,7) → Agent 打招呼 + 应手（剧本：先故意撞占点被拒，纠正后落子）
    check("人类首手落盘", await placeAndWait(ep, 7, 7, 1, 20000), `手数=${await moveCount(ep)}`);
    check("Agent 开局打招呼（聊天在案）", await waitText(ep, "请多指教", 40000, "开场 chat"));
    // 幽灵子先现（拟落 N,M + 橙圈）→ 提交后实子上盘（手数=2，随后拟落清空）。
    // 顺序先手数后幽灵消失：提交是「take 即清槽」，清空窗口与状态轮询（500ms）交错，
    // 先等手数能锚定「提交已发生」，再给清空断言一个确定的起点。
    // 圈选择器必须带 r=15：#FF8C1A 还被 BoardSvg 的悬停高亮（r=16/10）与最后一手
    // 标记（r=6.5）使用——只按描边色匹配会拿那两样当幽灵子，永不清空（实测踩过）。
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

    // 工具日志与用量：日志有 llm 行、调用计数 > 0
    const stat = await ep.page.evaluate(() => {
      const t = document.body.innerText;
      return {
        calls: Number(t.match(/LLM 调用 (\d+)/)?.[1] ?? -1),
        llmLog: /\[llm\]/.test(t),
        moveLog: /\[submit:move\]/.test(t),
        chatLog: /\[submit:chat\]/.test(t),
      };
    });
    check("工具日志增长（llm 行）", stat.calls > 0 && stat.llmLog, `LLM 调用=${stat.calls} llm行=${stat.llmLog}`);
    check("工具日志含落子/聊天动作行", stat.moveLog || stat.chatLog, JSON.stringify(stat));

    // 聊天往返：人发一句，Agent 剧本回一句
    await sendChat(ep, "你好 Agent，这一局请指教");
    check("聊天往返（Agent 回复在案）", await waitText(ep, "收到", 30000, "Agent 聊天回复"));

    // 拦截面：Agent 局存活期间，主会话的开局命令必须被拒（豁免的 A' 不经此路径）
    const intercepted = await ep.page.evaluate(async () => {
      const r = await window.__TAURI_INTERNALS__.invoke("session_cmd", {
        id: window.__session.nativeId(),
        cmdJson: JSON.stringify("createInvite"),
      });
      return JSON.parse(r);
    });
    check("拦截面：局中主会话 createInvite 被拒（notice 文案）",
      intercepted.ok === false && intercepted.error === "Agent 对局进行中", JSON.stringify(intercepted));

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
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-builtin-fail.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 截图：${path.join(SHOTS, "agent-builtin-fail.png")}`);
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    exitCode = fail > 0 ? 1 : 0;
  } catch (e) {
    console.error("E2E CRASH:", e);
    if (ep) {
      await diagDump(ep).catch(() => {});
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-builtin-crash.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    exitCode = 2;
  } finally {
    // 收尾必须真跑（process.exit 会跳过 finally——壳/桩/临时 profile 就都留尸了）
    if (ep) await ep.close().catch(() => {});
    try { shell.kill(); } catch { /* 已退 */ }
    await stub.close().catch(() => {});
    // WebView2 退出有尾拍，目录占用是常态——清不掉就留着并留痕，不让清理炸掉退出码
    try { fs.rmSync(tmp, { recursive: true, force: true }); } catch (e) { console.log(`[diag] 临时目录未清（占用）：${tmp}`); }
  }
  process.exit(exitCode ?? 2);
})();
