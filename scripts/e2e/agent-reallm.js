/**
 * agent-reallm.js —— 「Agent 对战·内置驱动」真 LLM 实测（用户拍板的必过门槛）。
 *
 * 与 agent-builtin.js（Mock 桩、只证接线）互补：本脚本让内置 Agent 与**真实模型**
 * 打完整五子棋局——落子通道、工具循环、预算与超时、错误恢复全部走真链路。
 *
 * 凭据纪律（红线）：端点/key/模型只从环境变量读取，**绝不写进本文件或任何进 git 的
 * 文件**。三缺一即 SKIP（退出码 0），脚本本身随时可安全入库：
 *   GOPTOP_TEST_LLM_URL   Anthropic 协议网关基地址（约定含版本段，如 https://x/v1）
 *   GOPTOP_TEST_LLM_KEY   API key
 *   GOPTOP_TEST_LLM_MODEL 模型名
 *
 * 诚实性口径：模型下得差 / 拒绝聊天 / 思考慢都不是失败——门槛是「真实 LLM 链路功能
 * 成立」（开局聊天在案、真实落子 ≥5 手、全程无 error 态、llmCalls>0 且 tokensIn>0）。
 * 链路级失败（Fatal / 卡死 / 无落子）才是 FAIL，须附完整诊断。
 *
 * 流程（壳 CDP 驱动与收尾纪律对齐 agent-builtin.js）：
 *   1) 【串行】构建壳（--no-build 跳过）；
 *   2) 起壳：独立 profile + CDP 口（不污染真实 ~/.goptop）；
 *   3) 经 CDP 预置 llm-config/llm-key/agent-ctx-limit → 进 /agent → 点开始；
 *   4) 人类脚本执黑与 Agent 对弈到「有人获胜或 ≥20 手」（无人获胜则主动认输收尾）；
 *   5) 断言：开局聊天（submit:chat）在案 / Agent 真实落子 ≥5 手 / 全程无 error 态 /
 *      llmCalls>0 且 tokensIn>0；关键态截图到 G:/tmp/shots/agent-reallm/。
 *
 * 用法：node agent-reallm.js [--no-build] [cdp端口]     默认 9222
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
/** 壳 exe 可经环境变量指路（默认主仓产物；worktree 构建等场景不改脚本即可换壳）。 */
const EXE = process.env.GOPTOP_SHELL_EXE || path.join(ROOT, "target", "release", "goptop.exe");
const SHOTS = "G:\\tmp\\shots\\agent-reallm";

/* 真实模型的时序预算（Mock 的秒级预算在这里全部放大；链路卡死靠超时兜住报 FAIL）。
 * 卡死判据是「无进展」而不是「单手超时」：慢模型一手想几分钟是常态（实测 4 手烧了
 * 4.7 万输出 token），只要 LLM 调用/落子/暂存仍在推进，链路就是活的，等待不算失败
 * （诚实性口径：思考慢不是失败）；只有连续 TURN_IDLE 无任何进展才收账。
 * 对局总预算 40 分钟——到点无人获胜就主动认输收尾。 */
const PAIRING_TIMEOUT = 150_000;   // 配对（本地 P2P，与 LLM 无关）
const TURN_IDLE = 240_000;         // 4 分钟无任何 LLM/落子/暂存进展＝链路卡死
const FAREWELL_TIMEOUT = 120_000;  // 终局后 Agent 收尾聊天
const GAME_BUDGET = 40 * 60_000;   // 对局总预算
const TARGET_MOVES = 20;           // 手数收尾线（总数；我执黑＝Agent 执白应手 10 手）
const MIN_AGENT_MOVES = 5;         // 门槛：Agent 真实落子 ≥5 手

const LLM_URL = process.env.GOPTOP_TEST_LLM_URL;
const LLM_KEY = process.env.GOPTOP_TEST_LLM_KEY;
const LLM_MODEL = process.env.GOPTOP_TEST_LLM_MODEL;

let pass = 0, fail = 0;
function check(name, ok, extra) {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " | " + extra : ""}`); }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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

/** 等页面文案出现；超时必须带出页面文本（光报超时没法归因）。 */
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

async function shot(ep, name) {
  const p = path.join(SHOTS, name);
  await ep.page.screenshot({ path: p, fullPage: true }).catch(() => {});
  console.log(`[shot] ${p}`);
}

/** 读「手数 N」（对局卡内，与主页面同款文案）。 */
async function moveCount(ep) {
  return ep.page.evaluate(() => {
    const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });
}

/** 落子（真实输入事件；坐标换算与 BoardSvg 互逆，同 agent-builtin.js）。
 *  坐标在页面里换算成视口像素，点击走 page.mouse.click——CDP 受信输入、过浏览器
 *  命中测试。page.evaluate 里 svg.dispatchEvent 的合成事件（isTrusted=false）绕过
 *  命中测试，会让「覆盖层挡住棋盘导致真人点不进」这类回归被假绿掩盖。 */
async function place(ep, x, y) {
  await ep.page.locator('svg[role="grid"]').first().scrollIntoViewIfNeeded().catch(() => {});
  const { cx, cy } = await ep.page.evaluate(([gx, gy]) => {
    const svg = document.querySelector('svg[role="grid"]');
    const r = svg.getBoundingClientRect();
    const vb = svg.viewBox.baseVal.width;
    const n = Number((svg.getAttribute("aria-label") || "").match(/(\d+)x\d+/)?.[1]) || 15;
    const pad = 30, cell = (vb - pad * 2) / (n - 1);
    return {
      cx: r.left + (pad + gx * cell) * (r.width / vb),
      cy: r.top + (pad + gy * cell) * (r.height / vb),
    };
  }, [x, y]);
  await ep.page.mouse.click(cx, cy);
}

/** 落一手并等手数推进到 expect；棋盘 disabled 的静默吞点用重试兜住。 */
async function placeAndWait(ep, x, y, expect, timeoutPerTry = 15000) {
  for (let i = 0; i < 3; i++) {
    await place(ep, x, y);
    const ok = await ep.page.waitForFunction((v) => {
      const m = document.querySelector(".play-stack")?.innerText.match(/手数\s*(\d+)/);
      return m ? Number(m[1]) >= v : false;
    }, expect, { timeout: timeoutPerTry, polling: 300 }).then(() => true).catch(() => false);
    if (ok) return true;
    console.log(`[diag] 落子 (${x},${y}) 未推进到手数 ${expect}（第 ${i + 1} 次），重试`);
  }
  return false;
}

/** AgentHub 运行 id 探测（开局后 live run 在 1..9 中恰好一个能查到）。 */
async function findRunId(ep) {
  for (const id of [1, 2, 3, 4, 5, 6, 7, 8, 9]) {
    const st = await ep.page.evaluate(async (i) => {
      try { return await window.__TAURI_INTERNALS__.invoke("agent_status", { id: i }); }
      catch { return null; }
    }, id).catch(() => null);
    if (typeof st === "string" && st.includes("state")) return id;
  }
  return null;
}

/** Hub 侧观察器：增量拉 agent_status/agent_events。真链路里「落了几手/聊了几句/
 *  有没有 error 态」的权威口径都在 Hub，页面 DOM 只是旁证。 */
function makeObserver(ep) {
  return {
    id: null, since: 0,
    agentMoves: [], chats: 0, llmCalls: 0, otherFails: [],
    errorState: null, lastStatus: null, firstDoneAt: null,
    async poll() {
      if (this.id === null) this.id = await findRunId(ep);
      if (this.id === null) return null;
      const raw = await ep.page.evaluate(async (i) => {
        try { return await window.__TAURI_INTERNALS__.invoke("agent_status", { id: i }); }
        catch (e) { return null; }
      }, this.id).catch(() => null);
      if (raw) {
        try {
          const st = JSON.parse(raw);
          this.lastStatus = st;
          if (st.state === "error" && this.errorState === null) {
            this.errorState = st.detail || "error";
            console.log(`[obs] 首次进入 error 态：${this.errorState}`);
          }
          if (st.state === "done" && this.firstDoneAt === null) this.firstDoneAt = Date.now();
        } catch { /* 单拍坏包容忍 */ }
      }
      const rev = await ep.page.evaluate(async (i) => {
        try { return await window.__TAURI_INTERNALS__.invoke("agent_events", { id: i, since: 0 }); }
        catch { return null; }
      }, this.id).catch(() => null);
      if (rev) {
        try {
          const r = JSON.parse(rev);
          this.agentMoves = []; this.chats = 0; this.llmCalls = 0; this.otherFails = [];
          for (const it of r.items ?? []) {
            if (it.tool === "submit:move") this.agentMoves.push(it.summary);
            else if (it.tool === "submit:chat") this.chats++;
            else if (it.tool === "llm") this.llmCalls++;
            if (it.ok === false) this.otherFails.push(`${it.tool}: ${it.summary}`);
          }
        } catch { /* 同上 */ }
      }
      return this.lastStatus;
    },
  };
}

/** 失败现场全景：各 id 的 agent_status + 平台存储快照。 */
async function diagDump(ep) {
  for (const id of [1, 2, 3]) {
    const st = await ep.page.evaluate(async (i) => {
      try { return await window.__TAURI_INTERNALS__.invoke("agent_status", { id: i }); }
      catch (e) { return `(${String(e).slice(0, 80)})`; }
    }, id).catch((e) => `evaluate 失败: ${e}`);
    console.log(`[diag] agent_status(${id}) = ${typeof st === "string" ? st : JSON.stringify(st)}`);
  }
  const store = await ep.page.evaluate(() => JSON.stringify(window.__store?.dump?.() ?? {})).catch(() => "{}");
  console.log(`[diag] store: ${store.slice(0, 800)}`);
}

/** 人类执黑的选点：从天元按距离展开的确定性序列，跳过已占点。 */
function nextEmptyPoint(occupied, size = 15) {
  const c = (size - 1) / 2;
  const pts = [];
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) pts.push({ x, y, d: Math.abs(x - c) + Math.abs(y - c) });
  pts.sort((a, b) => a.d - b.d || a.y - b.y || a.x - b.x);
  for (const p of pts) {
    if (!occupied.has(`${p.x},${p.y}`)) return p;
  }
  return null;
}

/** 从工具日志摘出 Agent 落子坐标（summary 形如「落子 (7,7)」）。 */
function coordsOf(moves) {
  return moves.map((s) => s.match(/\((\d+),(\d+)\)/)?.slice(1, 3).map(Number)).filter(Boolean);
}

(async () => {
  // —— 0) 凭据门：三缺一即 SKIP（退出码 0；这是「脚本可安全入库」的另一半含义） ——
  if (!LLM_URL || !LLM_KEY || !LLM_MODEL) {
    console.log("SKIP agent-reallm：缺环境变量 GOPTOP_TEST_LLM_URL / GOPTOP_TEST_LLM_KEY / GOPTOP_TEST_LLM_MODEL");
    console.log("SKIP 说明：真 LLM 凭据只经环境变量注入（绝不入库）；补齐三变量后重跑即测。");
    process.exit(0);
  }
  console.log(`[llm] 端点已注入（URL 长度=${LLM_URL.length}，key 长度=${LLM_KEY.length}，model=${LLM_MODEL}）`);

  let exitCode;
  fs.mkdirSync(SHOTS, { recursive: true });
  // —— 1) 串行构建壳（内存纪律：不与任何其它构建并发） ——
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

  // —— 2) 起壳（独立 profile + CDP；三个 profile 目录先建再指） ——
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "goptop-reallm-e2e-"));
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
    await sleep(1200);
    ep = await Endpoint.cdp("壳", `http://127.0.0.1:${CDP_PORT}`);
    ep.page.on("console", (m) => consoleLog.push(`[${m.type()}] ${m.text().slice(0, 300)}`));

    // —— 3) 预置配置（写平台存储后整页导航让 storeInit 重读）——
    await ep.setSetting("goptop:server-sel", "none");
    await ep.setSetting("goptop:name", "壳甲");
    await ep.setSetting("goptop:userId", "u-reallm-e2e-0001");
    // llm-config：protocol/baseUrl/model 全部来自环境变量；maxOutputTokens/replyLang
    // 与 agent-builtin.js 的 Mock 配置同值（变量只换端点，其余链路面不变）。
    await ep.setSetting("goptop:llm-config", JSON.stringify({
      protocol: "anthropic", baseUrl: LLM_URL, model: LLM_MODEL,
      maxOutputTokens: 1024, replyLang: "简体中文", enableSubagent: false,
    }));
    await ep.setSetting("goptop:llm-key", LLM_KEY);
    await ep.setSetting("goptop:agent-ctx-limit", "176000"); // 缺省值显式落盘（口径与设置卡一致）
    const origin = new URL(ep.page.url()).origin;
    await ep.page.goto(origin + "/agent", { waitUntil: "domcontentloaded" });
    await ep.ready(40000);
    check("设置卡渲染（内置 LLM 配置在案）", await waitText(ep, "内置 LLM 配置", 10000, "设置卡"));
    await shot(ep, "01-setup.png");

    // —— 4) 开局（我执黑＝默认设置，邀请方 A' 由 bind 代发邀请） ——
    await ep.page.locator("button:visible", { hasText: "开始对局" }).first().click();
    console.log("[game] 已点开始，等配对…");
    await sleep(1500);
    await shot(ep, "02-pairing.png");
    const playing = await waitText(ep, "黑 落子", PAIRING_TIMEOUT, "配对完成（黑 落子）");
    check("配对完成、轮到人类（我执黑先行）", playing);
    if (!playing) {
      await diagDump(ep);
      await shot(ep, "fail-pairing.png");
      throw new Error("配对失败，终止（详见上方诊断与截图）");
    }
    const obs = makeObserver(ep);
    await obs.poll();

    // —— 5) 对弈主循环：到「有人获胜或 ≥TARGET_MOVES 手」，期间盯 error 态/关键态截图 ——
    // 人类策略就是「推进对局」的工具：落子快慢无门槛意义，Agent 的应手才是被测链路。
    const t0 = Date.now();
    const occupied = new Set();
    const seenStates = new Set();
    let ghostShotDone = false, thinkingShotDone = false, midShotDone = false;
    let winner = null;
    let lastHandledMc = -1; // 我已处理过的「落子前手数」（偶数）；防同一手重复落子
    // 卡死判据：进展（LLM 调用数 / Agent 落子数 / 暂存幽灵子**变化**）一出现就刷新时钟，
    // 连续 TURN_IDLE 无任何进展才视为链路卡死（思考慢不是失败，卡死才是）。
    // 幽灵子只认「变化」不认「存在」：staged 落子提交前会一直挂着，拿存在当进展
    // 会让「write 暂存后卡死」的局永远等下去（实测踩过）。
    let lastProgressAt = Date.now();
    let lastLlmCalls = -1, lastAgentMoveCount = -1, lastAgentMoveAt = null, lastGhost = null;
    let humanPlaced = 0; // 我方**真正落盘**的手数（placeAndWait 以手数推进为准）

    while (Date.now() - t0 < GAME_BUDGET) {
      const st = await obs.poll();
      const mc = await moveCount(ep);
      const text = await ep.page.evaluate(() => document.body.innerText);
      const ghost = text.match(/拟落 \d+,\d+/)?.[0] ?? null;

      const progressed = obs.llmCalls !== lastLlmCalls
        || obs.agentMoves.length !== lastAgentMoveCount
        || (ghost !== null && ghost !== lastGhost);
      if (ghost !== lastGhost) lastGhost = ghost;
      if (progressed) {
        if (obs.agentMoves.length !== lastAgentMoveCount && lastAgentMoveAt !== null) {
          console.log(`[obs] Agent 第 ${obs.agentMoves.length} 手落定，距上一手 ${((Date.now() - lastAgentMoveAt) / 1000).toFixed(0)}s`);
        }
        if (obs.agentMoves.length !== lastAgentMoveCount) lastAgentMoveAt = Date.now();
        lastProgressAt = Date.now();
        lastLlmCalls = obs.llmCalls;
        lastAgentMoveCount = obs.agentMoves.length;
      } else if (Date.now() - lastProgressAt > TURN_IDLE) {
        console.log(`[obs] 连续 ${(TURN_IDLE / 1000).toFixed(0)}s 无任何进展（LLM 调用=${obs.llmCalls}），判链路卡死，收账`);
        break;
      }

      if (st?.state === "thinking" && !thinkingShotDone) {
        thinkingShotDone = true;
        console.log("[obs] Agent 思考中（首次）");
        await shot(ep, "03-thinking.png");
      }
      if (/拟落 \d+,\d+/.test(text) && !ghostShotDone) {
        ghostShotDone = true;
        console.log(`[obs] 幽灵子先现：${text.match(/拟落 \d+,\d+/)?.[0]}`);
        await shot(ep, "04-ghost.png");
      }
      if (!midShotDone && obs.agentMoves.length >= 3) {
        midShotDone = true;
        await shot(ep, "05-midgame.png");
      }

      const w = text.match(/(黑|白) 胜/)?.[1];
      if (w) { winner = w === "黑" ? "black" : "white"; break; }
      if (st?.state === "error") {
        console.log(`[obs] error 态（detail=${st.detail}），停止对局循环`);
        break;
      }
      if (mc >= TARGET_MOVES) break;

      // 我执黑：黑子永远落在偶数手数上（0,2,4…）——文案判轮次 + 偶数判手别，
      // 双条件防「点了但快照没跟上」的重复落子。
      const myTurn = text.includes("黑 落子");
      if (myTurn && mc >= 0 && mc % 2 === 0 && mc !== lastHandledMc) {
        for (const [x, y] of coordsOf(obs.agentMoves)) occupied.add(`${x},${y}`);
        const p = nextEmptyPoint(occupied);
        if (!p) break;
        console.log(`[game] 手数=${mc} 我落 (${p.x},${p.y})，等 Agent（Agent 已落 ${obs.agentMoves.length} 手）`);
        const advanced = await placeAndWait(ep, p.x, p.y, mc + 1);
        if (!advanced) {
          occupied.add(`${p.x},${p.y}`); // 该点被占/被吞，下一轮换点重试同一手
          continue;
        }
        occupied.add(`${p.x},${p.y}`);
        humanPlaced += 1;
        lastHandledMc = mc;
      } else {
        // 等 Agent（进展时钟由上方统一维护，这里只让轮询节奏）
        await sleep(700);
      }
    }
    const gameElapsed = Date.now() - t0;
    await obs.poll();
    console.log(`[game] 循环收束：耗时 ${(gameElapsed / 1000).toFixed(1)}s，我方落盘 ${humanPlaced} 手，Agent 下达落子命令 ${obs.agentMoves.length} 手，聊天 ${obs.chats} 条，LLM 调用 ${obs.llmCalls} 次，winner=${winner ?? "无"}`);

    // —— 6) 无人获胜就主动认输收尾（认输是两步确认，ChatPanel 内） ——
    if (!winner && obs.errorState === null) {
      console.log("[game] 手数收尾线已到/预算到点，人类主动认输收尾…");
      const resignBtn = ep.page.locator('button[title="认输（对手获胜）"]:visible').first();
      await resignBtn.waitFor({ state: "visible", timeout: 15000 }).catch(() => {});
      if (await resignBtn.count()) {
        await resignBtn.click();
        await ep.page.locator("button:visible", { hasText: "再点确认认输" }).first()
          .click({ timeout: 10000 }).catch(() => {});
        const resigned = await ep.page.waitForFunction(() => /(黑|白) 胜/.test(document.body.innerText), null, { timeout: 20000, polling: 300 }).then(() => true).catch(() => false);
        const wText = await ep.page.evaluate(() => document.body.innerText.match(/(黑|白) 胜/)?.[1] ?? null);
        winner = wText === "黑" ? "black" : wText === "白" ? "white" : null;
        check("人类认输后终局渲染", resigned && winner !== null, `winner=${winner}`);
      } else {
        console.log("[diag] 没找到认输按钮（聊天面板形态变化？）——不阻塞判定，按无 winner 收账");
      }
    }

    // —— 7) 终局路径：Agent 见 winner → 收尾聊天 → 状态卡「已终局」 ——
    if (winner) {
      const done = await waitText(ep, "已终局", FAREWELL_TIMEOUT, "Agent 状态卡（已终局）");
      check("Agent 状态卡收口为「已终局」", done);
      await obs.poll();
    }
    await shot(ep, "06-endgame.png");

    // —— 8) 门槛判定（落盘口径为权威：终局手数 − 我方落子 = Agent 真正上盘的子。
    //    agent_events 环的 submit:move 是 LogPlayer 在 UiCommand **下达时**记的条目
    //    （ok 恒 true 表「已下达」），下达≠落盘——被状态机竞态拒绝的落子同样留条目，
    //    只可作旁证，不作门槛）。 ——
    const finalMc = await moveCount(ep);
    const agentOnBoard = finalMc - humanPlaced;
    check("开局聊天在案（submit:chat ≥1）", obs.chats >= 1, `聊天 ${obs.chats} 条`);
    check(`Agent 真实落盘 ≥${MIN_AGENT_MOVES} 手（均为白方应手）`,
      agentOnBoard >= MIN_AGENT_MOVES,
      `落盘 ${agentOnBoard} 手（手数 ${finalMc} − 我方 ${humanPlaced}）；日志下达 ${obs.agentMoves.length} 手：${obs.agentMoves.slice(0, 8).join(" / ")}${obs.agentMoves.length > 8 ? " …" : ""}`);
    check("全程无 error 态", obs.errorState === null, obs.errorState ?? "无");
    const st = obs.lastStatus ?? {};
    check("LLM 真实调用在案（llmCalls>0 且 tokensIn>0）",
      (st.llmCalls ?? 0) > 0 && (st.tokensIn ?? 0) > 0,
      `llmCalls=${st.llmCalls} tokensIn=${st.tokensIn} tokensOut=${st.tokensOut} compactions=${st.compactions}`);
    check("对局推进（有人获胜或 ≥20 手）",
      winner !== null || (await moveCount(ep)) >= TARGET_MOVES,
      `winner=${winner ?? "无"} 手数=${await moveCount(ep)}`);

    // 页面无 JS 异常（旁证；真链路的功能门槛在上面四条）
    check("页面无 JS 异常", ep.errors.length === 0, ep.errors.slice(0, 3).join(" | "));

    console.log("\n===== 实测账目 =====");
    console.log(JSON.stringify({
      agentMovesOnBoard: agentOnBoard,
      agentMovesIssued: obs.agentMoves.length,
      moveCoordsIssued: coordsOf(obs.agentMoves),
      chats: obs.chats,
      llmCalls: st.llmCalls ?? obs.llmCalls,
      tokensIn: st.tokensIn ?? null,
      tokensOut: st.tokensOut ?? null,
      compactions: st.compactions ?? null,
      winner,
      gameElapsedMs: gameElapsed,
      model: LLM_MODEL,
    }, null, 2));

    if (fail > 0) {
      await diagDump(ep).catch(() => {});
      await shot(ep, "fail.png");
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
    exitCode = fail > 0 ? 1 : 0;
  } catch (e) {
    console.error("E2E CRASH:", e);
    if (ep) {
      await diagDump(ep).catch(() => {});
      await shot(ep, "crash.png");
      console.log(`[diag] 控制台尾部：\n${consoleLog.slice(-25).join("\n")}`);
    }
    exitCode = 2;
  } finally {
    // 收尾必须真跑（process.exit 会跳过 finally——壳/临时 profile 就都留尸了）
    if (ep) await ep.close().catch(() => {});
    try { shell.kill(); } catch { /* 已退 */ }
    try { execSync("taskkill //F //IM goptop.exe", { stdio: "ignore" }); } catch { /* 已退 */ }
    // WebView2 退出有尾拍，目录占用是常态——清不掉就留着并留痕
    try { fs.rmSync(tmp, { recursive: true, force: true }); } catch (e) { console.log(`[diag] 临时目录未清（占用）：${tmp}`); }
  }
  process.exit(exitCode ?? 2);
})();
