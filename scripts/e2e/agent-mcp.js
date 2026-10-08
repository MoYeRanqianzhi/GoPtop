/**
 * agent-mcp.js —— 「Agent 对战·MCP 驱动」桌面壳级全链 e2e（阶段④ H2 路）。
 *
 * 全链 = 壳里起内嵌 MCP 服务器（agent_mcp_set 真启停）→ 脚本扮演**外部 Agent**
 * （node 最小 MCP 客户端：裸 JSON-RPC over streamable HTTP）+ 脚本扮演**页面上的人**
 * （CDP 真实点击落子/聊天）→ 两侧打完整一局：人落子上盘 → wait_events 收 move 事件 →
 * Agent read /game/board → write 暂存（页面出幽灵子）→ submit 落子（实子上盘）→
 * 聊天一来一回 → Agent write+submit in/resign 认输收尾 → 页面终局渲染。
 * 断言「页面渲染与 MCP 侧返回一致」+ 无 token/错 token 一律 401。
 *
 * MCP 客户端协议细节（与 crates/goptop-agent/tests/mcp_server.rs 同一份规范口径）：
 * Accept 双形态、initialize 后回传 mcp-session-id 与 MCP-Protocol-Version 头、
 * 响应按 content-type 解 JSON 或 SSE（取最后一个可解析 data 帧）。
 * **无任何真实 LLM 端点/key**：本脚本不涉模型，token 读自壳的 agent_mcp_info。
 *
 * 用法：node agent-mcp.js [--no-build] [cdp端口]     默认 9222
 */
const { spawn, execSync } = require("child_process");
const path = require("path");
const fs = require("fs");
const os = require("os");
const http = require("http");
const { Endpoint } = require("./device.js");

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

/** 读「手数 N」（对局卡内文案）。 */
async function moveCount(ep) {
  return ep.page.evaluate(() => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });
}

/** 等手数达到 n。 */
function waitMoves(ep, n, timeout = 30000) {
  return ep.page.waitForFunction((x) => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) >= x : false;
  }, n, { timeout, polling: 300 }).then(() => true).catch(() => false);
}

/** 落子（真实点击；格点换算与 BoardSvg 严格互逆）。 */
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

/** 落一手并等手数推进到 expect（配对刚收口的点击会被 disabled 棋盘吞掉——以推进为准）。 */
async function placeAndWait(ep, x, y, expect, timeoutPerTry = 15000) {
  for (let i = 0; i < 3; i++) {
    await place(ep, x, y);
    if (await waitMoves(ep, expect, timeoutPerTry)) return true;
    console.log(`[diag] 落子 (${x},${y}) 未推进到手数 ${expect}（第 ${i + 1} 次），重试`);
  }
  return false;
}

/** 发聊天（侧栏停靠面板；真实键入 + 真实点击发送）。 */
async function sendChat(ep, text) {
  const input = ep.page.locator('input[placeholder="说点什么…"]:visible').first();
  await input.waitFor({ state: "visible", timeout: 15000 });
  await input.click();
  await input.fill("");
  await input.type(text, { delay: 10 });
  await ep.page.locator("button:visible", { hasText: "发送" }).first().click();
}

/** 失败现场全景：Agent 运行状态 + store 快照。 */
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
  console.log(`[diag] store: ${store.slice(0, 600)}`);
}

/* ========================= 最小 MCP 客户端（裸 JSON-RPC over streamable HTTP） ========================= */

/** 响应载荷解析：application/json 直解；SSE 按事件块拆 data: 行，取最后可解析帧；空体 null。 */
function parsePayload(ctype, body) {
  if (!body || !body.trim()) return null;
  if (ctype.includes("text/event-stream")) {
    let last = null;
    for (const block of body.split("\n\n")) {
      const data = block.split("\n")
        .filter((l) => l.startsWith("data:"))
        .map((l) => l.slice(5).trim())
        .join("\n");
      if (!data) continue;
      try { last = JSON.parse(data); } catch { /* 半帧忽略 */ }
    }
    return last;
  }
  try { return JSON.parse(body); } catch { return null; }
}

class McpClient {
  constructor(url, token) {
    this.url = url;
    this.token = token;
    this.session = null;
    this.nextId = 0;
  }

  /** 单次 POST。回 {status, payload}。`auth`=true 带 Bearer，false 裸发（401 断言用）。 */
  async post(body, { auth = true, timeoutMs = 30000 } = {}) {
    const headers = {
      "Content-Type": "application/json",
      "Accept": "application/json, text/event-stream",
    };
    if (auth) headers["Authorization"] = `Bearer ${this.token}`;
    if (this.session) {
      headers["mcp-session-id"] = this.session;
      headers["MCP-Protocol-Version"] = "2025-06-18";
    }
    const resp = await fetch(this.url, {
      method: "POST", headers, body: JSON.stringify(body), signal: AbortSignal.timeout(timeoutMs),
    });
    const sid = resp.headers.get("mcp-session-id");
    if (sid) this.session = sid;
    const ctype = resp.headers.get("content-type") ?? "";
    const text = await resp.text();
    return { status: resp.status, payload: parsePayload(ctype, text) };
  }

  /** JSON-RPC 请求：断言 200/id 回配/无协议错，回 result。 */
  async request(method, params, timeoutMs) {
    this.nextId += 1;
    const id = this.nextId;
    const { status, payload } = await this.post(
      { jsonrpc: "2.0", id, method, params }, { timeoutMs },
    );
    if (status !== 200) throw new Error(`${method} HTTP ${status}`);
    if (!payload) throw new Error(`${method} 无响应载荷`);
    if (payload.id !== id) throw new Error(`${method} 响应 id 不匹配: ${JSON.stringify(payload).slice(0, 200)}`);
    if (payload.error) throw new Error(`${method} 协议错误: ${JSON.stringify(payload.error).slice(0, 200)}`);
    return payload.result;
  }

  /** tools/call 简写：回 (isError, 首个 text content)。 */
  async callTool(tool, args, timeoutMs = 60000) {
    const result = await this.request("tools/call", { name: tool, arguments: args }, timeoutMs);
    const isErr = result.isError === true;
    const text = (result.content ?? []).at(0)?.text ?? "";
    return [isErr, text];
  }

  /** tools/call 成功回执 JSON（text content 解析）。 */
  async callToolJson(tool, args, timeoutMs) {
    const [isErr, text] = await this.callTool(tool, args, timeoutMs);
    if (isErr) throw new Error(`${tool} 意外 isError: ${text.slice(0, 200)}`);
    try { return JSON.parse(text); } catch (e) { throw new Error(`${tool} 回执不是 JSON: ${e}: ${text.slice(0, 200)}`); }
  }

  /** read 动态文件：read 回执的 content 才是文件本体（JSON 再解一层）。 */
  async readJson(path_, timeoutMs) {
    const receipt = await this.callToolJson("read", { path: path_ }, timeoutMs);
    const content = receipt.content ?? "";
    try { return JSON.parse(content); } catch (e) { throw new Error(`${path_} 内容不是 JSON: ${e}: ${String(content).slice(0, 200)}`); }
  }

  /** 轮询 wait_events 直到谓词命中（事件分批到达——单次调用会撞上「只等到先到批」）。 */
  async waitEvent(what, pred, secs = 30) {
    const deadline = Date.now() + secs * 1000;
    let last = [];
    while (Date.now() < deadline) {
      const events = await this.callToolJson("wait_events", { timeout_secs: 1 });
      last = Array.isArray(events) ? events : [];
      if (last.some(pred)) return true;
      await sleep(50);
    }
    console.log(`[diag] 超时等事件（${what}）；最后一批: ${JSON.stringify(last).slice(0, 400)}`);
    return false;
  }
}

(async () => {
  let exitCode;
  fs.mkdirSync(SHOTS, { recursive: true });
  // —— 1) 串行构建壳（内存纪律：不与任何其它构建并发；--no-build 跳过） ——
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

  // —— 2) 起壳（独立 profile + CDP；profile 三目录先建再指，shell-pair.js 头注的坑） ——
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "goptop-agent-mcp-e2e-"));
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

    // —— 3) 进 /agent，切 MCP 驱动并启用服务器（独立 profile 的 store 是全新的） ——
    await ep.setSetting("goptop:server-sel", "none");
    await ep.setSetting("goptop:name", "壳甲");
    await ep.setSetting("goptop:userId", "u-agent-mcp-e2e-01");
    const origin = new URL(ep.page.url()).origin;
    await ep.page.goto(origin + "/agent", { waitUntil: "domcontentloaded" });
    await ep.ready(40000);

    const mcpBtn = ep.page.locator("button:visible", { hasText: "MCP（外部 Agent）" }).first();
    await mcpBtn.waitFor({ state: "visible", timeout: 10000 });
    await mcpBtn.click();
    check("切到 MCP：连接卡渲染", await waitText(ep, "MCP 服务器", 6000, "MCP 卡"));

    // 开关归到已关闭再启到已开启（断言与上轮残留无关）。
    const toggleNow = await ep.page.evaluate(() => {
      const vis = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const b = [...document.querySelectorAll("button")].find((x) => vis(x) &&
        (x.textContent.includes("已关闭（点此启用）") || x.textContent.includes("已启用（点此关闭）")));
      return b?.textContent ?? null;
    });
    if (toggleNow?.includes("已启用")) {
      await ep.page.locator("button:visible", { hasText: "已启用（点此关闭）" }).first().click();
      await sleep(500);
    }
    await ep.page.locator("button:visible", { hasText: "已关闭（点此启用）" }).first().click();
    check("MCP 服务器真启动（状态行「服务器运行中」）",
      await waitText(ep, "状态：服务器运行中", 10000, "MCP 状态行"));

    // 读真实连接信息（端口回退/token 以运行实例为准）。
    const info = await ep.page.evaluate(async () => JSON.parse(await window.__TAURI_INTERNALS__.invoke("agent_mcp_info")));
    check("agent_mcp_info 回真实连接信息（enabled + url + token）",
      info.enabled === true && /^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(info.url ?? "") && !!info.token,
      `url=${info.url} token=${info.token ? `${String(info.token).length} 字符` : "无"}`);

    // —— 4) 外部 Agent 侧：鉴权 → initialize → 工具面 ——
    const anon = new McpClient(info.url, "");
    const anonStatus = (await anon.post({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }, { auth: false })).status;
    check("无 token 请求被拒（401）", anonStatus === 401, `status=${anonStatus}`);
    const impostor = new McpClient(info.url, "wrong-token-not-the-real-one");
    const imStatus = (await impostor.post({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} })).status;
    check("错 token 请求被拒（401）", imStatus === 401, `status=${imStatus}`);

    const rpc = new McpClient(info.url, info.token);
    const init = await rpc.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "goptop-agent-mcp-e2e", version: "0.0.0" },
    });
    check("initialize 握手（回显协议版本 + 会话 id 头）",
      init.protocolVersion === "2025-06-18" && !!rpc.session,
      `session=${rpc.session ? `${rpc.session.slice(0, 8)}…` : "无"}`);
    const notified = await rpc.post({ jsonrpc: "2.0", method: "notifications/initialized" });
    check("initialized 通知回 2xx 空回执", notified.status >= 200 && notified.status <= 202, `status=${notified.status}`);

    const tools = await rpc.request("tools/list", {});
    const names = (tools.tools ?? []).map((t) => t.name);
    const readOnlyOk = (tools.tools ?? []).every((t) =>
      (t.annotations?.readOnlyHint === true) === ["read", "grep", "wait_events"].includes(t.name));
    check("tools/list：8 个工具 + read_only 注解",
      JSON.stringify(names) === JSON.stringify(["read", "write", "submit", "edit", "grep", "wait_events", "game_start", "game_leave"]) && readOnlyOk,
      JSON.stringify(names));

    // —— 5) 无待局认领：业务错误（isError 文本），人侧未开局是正常流程 ——
    const [earlyErr, earlyText] = await rpc.callTool("game_start", {});
    check("人侧未开局时 game_start 回业务错误",
      earlyErr === true && earlyText.includes("no game to claim yet"), earlyText.slice(0, 120));

    // —— 6) 人侧开局（我执黑·五子棋 15）：等待接入 → 外部 Agent 认领 ——
    await ep.page.locator("button:visible", { hasText: "开始对局" }).first().click();
    console.log("[game] 已点开始（我执黑），等 waiting_mcp…");
    check("壳进入「等待 MCP Agent 接入」",
      await waitText(ep, "等待 MCP Agent 接入", 60000, "waiting_mcp"));
    const start = await rpc.callToolJson("game_start", {}, 120000);
    check("game_start 认领成功（配对完成，Agent 执白）",
      start.started === true && start.my_color === "white", JSON.stringify(start));

    // —— 7) 完整一局：人（页面点击）vs Agent（MCP 工具面）——
    // 黑方（人）与白方（Agent）坐标表：全不重叠、白方不挡黑方。
    const BLACK = [[7, 7], [9, 9], [5, 11]];
    const WHITE = [[8, 8], [10, 10], [4, 12]];
    let mc = 0;
    for (let round = 0; round < BLACK.length; round++) {
      const [bx, by] = BLACK[round];
      // 人落子 → 页面推进 → wait_events 收 move 事件
      check(`第 ${mc + 1} 手（人 ${bx},${by}）落盘`, await placeAndWait(ep, bx, by, mc + 1), `手数=${await moveCount(ep)}`);
      mc += 1;
      check(`wait_events 收到人方 move 事件 (${bx},${by})`,
        await rpc.waitEvent(`人方第 ${mc} 手`, (e) =>
          e.t === "move" && e.by === "black" && e.x === bx && e.y === by, 30));

      // Agent 读盘 → 与页面手数一致
      const board = await rpc.readJson("/game/board");
      check(`read /game/board 与页面一致（手数=${mc}）`,
        board.move_count === mc && (board.stones.black ?? []).some(([x, y]) => x === bx && y === by),
        `MCP move_count=${board.move_count}`);

      // write 暂存（页面出幽灵子）→ submit 落子（实子上盘）
      const [wx, wy] = WHITE[round];
      const staged = await rpc.callToolJson("write", { path: "/game/in/move", content: `${wx},${wy}` });
      check(`write 暂存 (${wx},${wy}) 只回执不执行`, staged.ok === true && staged.staged?.path === "/game/in/move", JSON.stringify(staged).slice(0, 120));
      const GHOST = 'svg circle[r="15"][stroke="#FF8C1A"]';
      const ghostShown = await ep.page.waitForFunction((sel) =>
        document.body.innerText.includes("拟落") && !!document.querySelector(sel),
      GHOST, { timeout: 10000, polling: 150 }).then(() => true).catch(() => false);
      const ghostText = await ep.page.evaluate(() => document.body.innerText.match(/拟落 \d+,\d+/)?.[0] ?? null);
      check("暂存幽灵子上盘（拟落坐标 + 橙圈）", ghostShown, ghostText ?? "未见");
      const placed = await rpc.callToolJson("submit", { path: "/game/in/move" });
      check(`submit 落子 (${wx},${wy})`, placed.ok === true && placed.action === "move" && placed.move_count === mc + 1, JSON.stringify(placed).slice(0, 120));
      mc += 1;
      check(`第 ${mc} 手（Agent ${wx},${wy}）同步到页面`, await waitMoves(ep, mc, 20000), `手数=${await moveCount(ep)}`);
      const ghostGone = await ep.page.waitForFunction((sel) =>
        !document.body.innerText.includes("拟落") && !document.querySelector(sel),
      GHOST, { timeout: 8000, polling: 150 }).then(() => true).catch(() => false);
      check("提交后幽灵子消失", ghostGone);

      // 聊天一来一回（只在第 1 轮走全流程，后面轮次省时间）
      if (round === 0) {
        await sendChat(ep, "你好 Agent，这一局请指教");
        check("聊天：wait_events 收到人方消息",
          await rpc.waitEvent("人方聊天", (e) => e.t === "chat" && e.text === "你好 Agent，这一局请指教", 30));
        await rpc.callToolJson("write", { path: "/game/in/chat", content: "你好！请多指教。" });
        await rpc.callToolJson("submit", { path: "/game/in/chat" });
        check("聊天：Agent 回复渲染到页面", await waitText(ep, "你好！请多指教。", 30000, "Agent 聊天"));
      }
    }

    // —— 8) Agent 认输收尾 → 页面终局渲染与 MCP 侧一致 ——
    await rpc.callToolJson("write", { path: "/game/in/resign", content: "这一局我认输" });
    const resigned = await rpc.callToolJson("submit", { path: "/game/in/resign" });
    check("Agent 认输（submit in/resign → 黑胜）", resigned.ok === true && resigned.winner === "black", JSON.stringify(resigned).slice(0, 140));
    check("页面终局渲染（黑 胜）", await waitText(ep, "黑 胜", 30000, "终局"), `手数=${await moveCount(ep)}`);
    check("Agent 状态卡收口为「已终局」", await waitText(ep, "已终局", 10000, "状态卡"));
    const boardFinal = await rpc.readJson("/game/board").catch(() => null);
    check("终局后 read /game/board 与页面同账（winner/black）",
      boardFinal && boardFinal.winner === "black" && boardFinal.move_count === 6,
      JSON.stringify(boardFinal?.winner));

    // 认输后拆局（game_leave：已终局无事可做，回 note 不报错）。
    const leave = await rpc.callToolJson("game_leave", {});
    check("game_leave 收尾（终局后无需再认输）", leave.ok === true && leave.resigned === false, JSON.stringify(leave).slice(0, 140));
    const [afterErr, afterText] = await rpc.callTool("read", { path: "/game/board" });
    check("拆局后工具面回「no live game」", afterErr === true && afterText.includes("no live game"), afterText.slice(0, 100));

    // —— 9) 关闭方向：agent_mcp_set(false) 停服务器，info 回落未运行口径 ——
    const offInfo = await ep.page.evaluate(async () =>
      JSON.parse(await window.__TAURI_INTERNALS__.invoke("agent_mcp_set", { enabled: false })));
    check("MCP 服务器可关闭（agent_mcp_set(false) → enabled=false）",
      offInfo.enabled === false && /^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(offInfo.url ?? ""),
      JSON.stringify(offInfo).slice(0, 140));

    if (fail > 0) {
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-mcp-fail.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 截图：${path.join(SHOTS, "agent-mcp-fail.png")}`);
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    exitCode = fail > 0 ? 1 : 0;
  } catch (e) {
    console.error("E2E CRASH:", e);
    if (ep) {
      await diagDump(ep).catch(() => {});
      await ep.page.screenshot({ path: path.join(SHOTS, "agent-mcp-crash.png"), fullPage: true }).catch(() => {});
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    exitCode = 2;
  } finally {
    // 收尾必须真跑（process.exit 会跳过 finally——壳/临时 profile 就都留尸了）
    if (ep) await ep.close().catch(() => {});
    try { shell.kill(); } catch { /* 已退 */ }
    // WebView2 退出有尾拍，目录占用是常态——清不掉就留着并留痕，不让清理炸掉退出码
    try { fs.rmSync(tmp, { recursive: true, force: true }); } catch { console.log(`[diag] 临时目录未清（占用）：${tmp}`); }
  }
  process.exit(exitCode ?? 2);
})();
