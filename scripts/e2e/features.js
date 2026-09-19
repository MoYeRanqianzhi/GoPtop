/**
 * features.js — 设计功能实机验证（非「两两对战」矩阵，专测各功能的可达性与正确性）。
 *
 * 全部交互走真实输入（真实点击/键入），快照只用于读取断言。
 *
 * 用法：
 *   node features.js local <端>                     本地对战：落子/悔棋/重开
 *   node features.js go <A> <B>                     围棋：提子 → 双停一手 → 终局计分
 *   node features.js challenge <A> <B>              大厅挑战（在线用户 → 挑战 → 弹窗同意）
 *   node features.js resign <A> <B>                 认输（聊天区内两步确认）
 *   node features.js specchat <A> <B> <C>           观战者申请发言（双方批准后发言到达）
 *   node features.js kick <A> <B> <C>               房主踢出观战者
 *
 * 端规格同 match.js：web / mob / android / cdp:<url>。
 */
const { Endpoint, playOneMove } = require("./device.js");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const HOME = "http://localhost:5173/";

let passed = 0;
let failed = 0;

/** 本轮唯一昵称后缀：名册里可能残留同名旧条目（异常断开在服务器空闲超时前仍在册），
 *  按名字定位同伴行时会命中陈旧条目——加随机后缀保证唯一。 */
const RUN_TAG = Math.random().toString(36).slice(2, 6);
function check(name, cond, extra = "") {
  if (cond) { passed++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { failed++; console.log(`FAIL ${name}${extra ? " — " + extra : ""}`); }
}

async function open(spec, name) {
  let ep;
  if (spec === "android" || spec.startsWith("android:")) {
    ep = await Endpoint.android(name, { serial: spec.includes(":") ? spec.slice(8) : null });
    await prepShell(ep, name);
    return ep;
  }
  if (spec.startsWith("cdp:")) {
    ep = await Endpoint.cdp(name, spec.slice(4));
    await prepShell(ep, name);
    return ep;
  }
  if (spec === "mob" || spec.startsWith("mob:")) {
    ep = await Endpoint.browser(name, spec.includes(":") ? spec.slice(4) : HOME, { mobile: true });
  } else {
    ep = await Endpoint.browser(name, spec.includes(":") ? spec.slice(4) : HOME);
  }
  ep.displayName = `${name}${RUN_TAG}`;
  await ep.page.evaluate((n) => {
    localStorage.setItem("goptop:name", n);
    if (process?.env?.XDEV_SERVER === "none") localStorage.setItem("goptop:server-sel", "none");
    else localStorage.removeItem("goptop:server-sel");
  }, ep.displayName).catch(() => {});
  await ep.goto(HOME + "p2p");
  await ep.ready(40000);
  return ep;
}

/** 壳端点预置：走 ep.setSetting（壳端数据在平台存储，写 localStorage 已无效）。 */
async function prepShell(ep, name) {
  ep.displayName = `${name}${RUN_TAG}`;
  await ep.setSetting("goptop:name", ep.displayName).catch(() => {});
  if (process?.env?.XDEV_SERVER === "none") await ep.setSetting("goptop:server-sel", "none").catch(() => {});
  await ep.page.reload({ waitUntil: "domcontentloaded" });
  await ep.ready(40000);
}

/** 按执色找到对应端点。 */
async function byColor(A, B, color) {
  const [ca, cb] = [(await A.snap()).myColor, (await B.snap()).myColor];
  if (ca === color) return A;
  if (cb === color) return B;
  throw new Error(`没有执${color}的一端`);
}

async function pairUp(A, B) {
  for (const ep of [A, B]) { await ep.enterP2P(); await ep.waitServerReady(); }
  const link = await A.createInvite();
  await B.joinByPaste(link);
  await A.waitSnap((s) => s.phase === "playing", 90000, `${A.name} 进入对局`);
  await B.waitSnap((s) => s.phase === "playing", 90000, `${B.name} 进入对局`);
  return link;
}

async function waitSync(A, B, timeout = 30000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    const [a, b] = [await A.snap(), await B.snap()];
    if (a.moveCount === b.moveCount && a.winner === b.winner && a.scoring === b.scoring) return;
    await sleep(220);
  }
  throw new Error("两端未同步");
}

/** 棋盘上某色的棋子数（从快照数）。 */
function countStones(board, color) {
  let n = 0;
  for (const row of board) for (const c of row) if (c === color) n++;
  return n;
}

/* ------------------------- 场景：本地对战 ------------------------- */

async function scenarioLocal(ep) {
  // 菜单 → 本地对战
  await ep.home().catch(() => {});
  const menu = ep.page.locator("button:visible", { hasText: "本地对战" }).first();
  if (await menu.count() && await menu.isVisible().catch(() => false)) await menu.click();
  else { await ep.clickButton("菜单"); await sleep(300); await ep.clickButton("本地对战"); }
  await ep.page.waitForFunction(() => location.pathname === "/local", null, { timeout: 10000, polling: 120 });
  await sleep(400);

  const stones = () => ep.page.evaluate(() => document.querySelectorAll('svg[role="grid"] circle[r="15"]').length);
  const moves = () => ep.page.evaluate(() => {
    const m = document.body.innerText.match(/手数\s*\n?\s*(\d+)/);
    return m ? Number(m[1]) : -1;
  });

  await ep.place(7, 7); await sleep(300);
  await ep.place(7, 8); await sleep(300);
  await ep.place(8, 8); await sleep(300);
  check("本地落子三手", (await stones()) === 3 && (await moves()) === 3, `石子=${await stones()} 手数=${await moves()}`);

  await ep.clickButton("悔棋"); await sleep(400);
  check("本地悔棋回退一手", (await stones()) === 2, `石子=${await stones()}`);

  await ep.clickButton("重开"); await sleep(400);
  check("本地重开清盘", (await stones()) === 0, `石子=${await stones()}`);
}

/* ------------------------- 场景：围棋提子 + 终局计分 ------------------------- */

/**
 * 选围棋 9 路。顺序很重要：尺寸按钮随当前规则变（五子棋只有 15×15，围棋才有 9/13/19），
 * 所以必须先切围棋再选 9×9。窄屏（手机/短窗口）两者都收在「类型」折叠面板里，
 * 且面板选完规则会自动收起——每步都要重新判断是否需要再展开。
 */
async function pickGo9(ep) {
  const vis = async (t) => {
    const b = ep.page.locator("button:visible", { hasText: t }).first();
    return (await b.count()) > 0 && (await b.isVisible().catch(() => false));
  };
  if (!(await vis("围棋"))) { await ep.clickButton("类型"); await sleep(350); }
  await ep.clickButton("围棋"); await sleep(400);
  if (!(await vis("9×9"))) { await ep.clickButton("类型"); await sleep(350); }
  await ep.clickButton("9×9"); await sleep(350);
}

async function scenarioGo(A, B) {
  await pickGo9(A);
  await pairUp(A, B);
  const [sa, sb] = [await A.snap(), await B.snap()];
  check("围棋 9 路建局", sa.kind === "go" && sa.size === 9 && sb.kind === "go" && sb.size === 9, `${sa.kind}/${sa.size}`);

  // 脚本走子：黑围吃白 (4,4)
  const seq = [
    ["black", 3, 4], ["white", 4, 4],
    ["black", 4, 3], ["white", 5, 5],
    ["black", 5, 4], ["white", 0, 0],
    ["black", 4, 5], // 提子
  ];
  for (const [color, x, y] of seq) {
    const ep = await byColor(A, B, color);
    await ep.waitSnap((s) => s.toMove === s.myColor && !s.winner, 30000, `${color} 轮次`);
    await ep.place(x, y);
    await waitSync(A, B);
  }
  const after = await A.snap();
  check("围棋提子生效（白 (4,4) 被提）", after.board[4][4] === "empty" && countStones(after.board, "white") === 2,
    `白子=${countStones(after.board, "white")} (4,4)=${after.board[4][4]}`);

  // 双方各停一手 → 终局计分
  for (let i = 0; i < 4; i++) {
    const s = await A.snap();
    if (s.scoring) break;
    const ep = s.toMove === s.myColor ? A : B;
    await ep.clickButton("停一手");
    await sleep(600);
  }
  await waitSync(A, B);
  const sc = await A.snap();
  check("双停一手进入终局计分", sc.scoring === true, `scoring=${sc.scoring}`);

  // 标记死子（点盘上一颗黑子）
  const first = sc.board.flatMap((row, y) => row.map((c, x) => ({ c, x, y }))).find((p) => p.c === "black");
  if (first) {
    const rect = await A.page.evaluate(() => { const el = document.querySelector('svg[role="grid"]'); const r = el.getBoundingClientRect(); return { w: Math.round(r.width), h: Math.round(r.height), top: Math.round(r.top) }; });
    console.log(`  [诊断] 点击 (${first.x},${first.y})，A 棋盘 ${JSON.stringify(rect)}`);
    await A.place(first.x, first.y);
    await sleep(800);
    console.log(`  [诊断] A 自身标记=${JSON.stringify((await A.snap()).myDead)}`);
  }
  // 标记不改变手数，waitSync 会立即返回——必须显式等对方收到 ScoreMark。
  const marked = await B.waitSnap((s) => s.peerDead.length >= 1 || s.myDead.length >= 1, 20000, "死子标记同步");
  check("死子标记同步到对方", marked.peerDead.length >= 1 || marked.myDead.length >= 1,
    `对方收到=${marked.peerDead.length} 子`);

  // 双方确认 → 出计分结果
  await A.clickButton("确认计分");
  await B.waitSnap((s) => s.confirmReq?.kind === "score-confirm", 20000, "B 弹出计分确认");
  await B.approve();
  await A.waitSnap((s) => !!s.scoreResult, 30000, "计分结果");
  const [ra, rb] = [await A.snap(), await B.snap()];
  check("双方计分结果一致", ra.scoreResult && rb.scoreResult && ra.scoreResult.winner === rb.scoreResult.winner,
    `黑 ${ra.scoreResult?.black} 白 ${ra.scoreResult?.white} → ${ra.scoreResult?.winner} 胜`);
  check("计分后宣布胜者", !!ra.winner && ra.winner === rb.winner, `winner=${ra.winner}`);
}

/* ------------------------- 场景：大厅挑战 ------------------------- */

async function scenarioChallenge(A, B) {
  for (const ep of [A, B]) { await ep.enterP2P(); await ep.waitServerReady(); }
  // 名册要等对方上线（服务器广播有延迟）。
  const target = await B.snap();
  await A.waitSnap((s) => s.peers.some((p) => p.id === target.userId), 40000, `名册出现 ${B.displayName}`);
  // A 去在线用户页找 B 并挑战。
  await A.clickButton("菜单"); await sleep(300);
  await A.clickButton("在线用户");
  await A.page.waitForFunction(() => location.pathname === "/users", null, { timeout: 10000, polling: 120 });
  await sleep(500);

  // 名册里可能有别的在线用户（含残留条目），必须点到**对方那一行**的挑战按钮——
  // 首行的按钮可能在「对局中」被禁用，点它只会超时。
  const handle = await A.page.evaluateHandle((name) => {
    const rows = [...document.querySelectorAll("div")].filter((d) => {
      const t = d.textContent || "";
      return t.includes(name) && t.includes("挑战");
    });
    // 取最内层（行）的那一个，避免命中包住整个列表的容器
    const row = rows.sort((a, b) => a.querySelectorAll("button").length - b.querySelectorAll("button").length)[0];
    if (!row) return null;
    return [...row.querySelectorAll("button")].find((b) => (b.textContent || "").includes("挑战")) || null;
  }, B.displayName);
  const el = handle.asElement();
  if (!el) { check("大厅挑战：找到对方的挑战按钮", false, `名册未出现 ${B.displayName}`); return; }
  const disabled = await el.isDisabled();
  if (disabled) { check("大厅挑战：按钮可用", false, "对方状态为对局中"); return; }
  await el.click(); // 元素定位后仍是真实点击
  await B.waitSnap((s) => !!s.serverIncoming || !!s.incoming, 30000, "B 弹出邀请弹窗");
  check("挑战到达对方（弹窗）", true);
  await B.approve();
  await A.waitSnap((s) => s.phase === "playing", 60000, "A 进入对局");
  await B.waitSnap((s) => s.phase === "playing", 60000, "B 进入对局");
  check("挑战双方进入对局", (await A.snap()).phase === "playing" && (await B.snap()).phase === "playing");
  // 挑战受理后从 /users 被导航回 /p2p，等棋盘真正渲染出来再落子。
  for (const ep of [A, B]) {
    await ep.page.waitForFunction(() => location.pathname === "/p2p" && !!document.querySelector('svg[role="grid"]'), null, { timeout: 20000, polling: 150 });
  }
  await A.waitSnap((s) => s.peerConnected, 30000, "A 直连建立");
  // 与对战同源的下棋能力
  const w = await playOneMove(A);
  await waitSync(A, B);
  check("挑战对局可正常落子", (await A.snap()).moveCount === 1, `首手=${JSON.stringify(w)}`);
}

/* ------------------------- 场景：认输 ------------------------- */

async function scenarioResign(A, B) {
  await pairUp(A, B);
  await playOneMove(A); await waitSync(A, B);
  // 认输在聊天区（对局操作集合区），不在棋盘旁的操作行——先进聊天面板再点
  await A.openChat();
  await A.clickInChat("认输"); await sleep(300);
  await A.clickInChat("再点确认认输");
  await A.closeChat();
  await B.waitSnap((s) => !!s.winner, 30000, "B 收到认输");
  const [ra, rb] = [await A.snap(), await B.snap()];
  check("认输判负、对手获胜", ra.winner === rb.winner && ra.winner === "white", `A(黑)认输 → winner=${ra.winner}`);
}

/* ------------------------- 场景：观战者申请发言 ------------------------- */

async function scenarioSpecChat(A, B, C) {
  await pairUp(A, B);
  const spec = await A.inviteSpectate();
  if (C._browser && !C.page.url().startsWith("http://tauri") && !C.page.url().startsWith("https://appassets")) {
    await C.goto(spec);
  } else {
    await C.enterP2P(); await C.waitServerReady(); await C.joinByPaste(spec);
  }
  await C.waitSnap((s) => s.role === "spectator" && s.phase === "playing", 90000, "C 进入观战");
  await C.waitSnap((s) => s.peerConnected, 60000, "C 直连");
  check("观战者接入", true);

  // C 申请发言 → 到达房主/对手的是**协商横幅**（spec-chat），需双方各自同意。
  await C.openChat();
  await C.clickInChat("申请发言");
  await C.closeChat();
  await A.waitSnap((s) => s.confirmReq?.kind === "spec-chat", 30000, "A 弹出观战发言批准");
  check("发言申请到达房主（横幅）", true);
  await A.approve();
  await B.waitSnap((s) => s.confirmReq?.kind === "spec-chat", 30000, "B 弹出观战发言批准");
  check("批准后转达对手（横幅）", true);
  await B.approve();
  await C.waitSnap((s) => s.specCanChat, 30000, "C 获得发言权");
  check("双方批准后观战者获得发言权", (await C.snap()).specCanChat === true);

  // C 发言 → 对局者聊天区可见
  await C.sendChat("观众报到");
  await A.waitSnap((s) => s.chatLog.some((m) => m.text.includes("观众报到")), 30000, "A 收到观战者发言");
  check("观战者发言到达对局者", true);
}

/* ------------------------- 场景：踢出观战者 ------------------------- */

async function scenarioKick(A, B, C) {
  await pairUp(A, B);
  const spec = await A.inviteSpectate();
  if (C._browser && !C.page.url().startsWith("http://tauri") && !C.page.url().startsWith("https://appassets")) {
    await C.goto(spec);
  } else {
    await C.enterP2P(); await C.waitServerReady(); await C.joinByPaste(spec);
  }
  await C.waitSnap((s) => s.role === "spectator" && s.peerConnected, 90000, "C 进入观战");
  await A.waitSnap((s) => s.spectators.length > 0, 30000, "A 名册出现观战者");
  check("房主名册出现观战者", true);

  await A.openChat();
  await A.clickInChat("踢出");
  await A.closeChat();
  await C.waitSnap((s) => s.role !== "spectator", 30000, "C 被踢出");
  const cs = await C.snap();
  check("观战者被移出观战", cs.role !== "spectator" && cs.phase === "home", `role=${cs.role} phase=${cs.phase}`);
}

/* ------------------------------ 入口 ------------------------------ */

const SCENARIOS = {
  local: { n: 1, fn: scenarioLocal },
  go: { n: 2, fn: scenarioGo },
  challenge: { n: 2, fn: scenarioChallenge },
  resign: { n: 2, fn: scenarioResign },
  specchat: { n: 3, fn: scenarioSpecChat },
  kick: { n: 3, fn: scenarioKick },
};

(async () => {
  const [name, ...specs] = process.argv.slice(2);
  const sc = SCENARIOS[name];
  if (!sc) { console.error(`未知场景: ${name}（可选：${Object.keys(SCENARIOS).join(" / ")}）`); process.exit(2); }
  const eps = [];
  try {
    const names = ["A", "B", "C"];
    for (let i = 0; i < sc.n; i++) eps.push(await open(specs[i] || "web", names[i]));
    console.log(`[就绪] ${eps.map((e) => e.name).join(" ")}  场景=${name}`);
    await sc.fn(...eps);
  } catch (e) {
    failed++;
    console.error(`FAIL ${name} — 异常：${e.message}`);
    for (const ep of eps) { try { console.error(`  [${ep.name}] ${await ep.brief()}`); } catch { /* ignore */ } }
  } finally {
    for (const ep of eps) await ep.close();
  }
  console.log(`\n===== ${name}: ${passed} passed, ${failed} failed =====`);
  process.exit(failed ? 1 : 0);
})();
