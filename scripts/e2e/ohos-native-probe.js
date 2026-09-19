/**
 * ohos-native-probe.js —— 鸿蒙壳「原生宿主是否真的接上了」的最小探针。
 *
 * 只回答一个问题：**壳里的规则/AI 走的是原生 Rust，还是回落到了 wasm？**
 * 面（UI 能不能下棋）由 ai.js / match.js 压，这里只验桥本身——桥不通时那些脚本
 * 会以为「功能没反应」，排查起来要绕一大圈。
 *
 * 用法：node ohos-native-probe.js [cdpUrl]     默认 http://127.0.0.1:9444
 * 前置：hdc fport tcp:9444 localabstract:webview_devtools_remote_<主进程 pid>
 */
const { Endpoint } = require("./device.js");

const CDP = process.argv[2] || "http://127.0.0.1:9444";

let pass = 0, fail = 0;
function check(name, ok, extra = "") {
  if (ok) { pass++; console.log(`PASS ${name}${extra ? " — " + extra : ""}`); }
  else { fail++; console.log(`FAIL ${name}${extra ? " — " + extra : ""}`); }
}

(async () => {
  const ep = await Endpoint.cdp("ohos", CDP);
  const page = ep.page;
  page.on("pageerror", (e) => console.log("[pageerror]", String(e).slice(0, 200)));
  console.log("页面:", page.url());

  // 1) 代理对象存在且三个方法都在
  const shape = await page.evaluate(() => {
    const g = window.goptopHost;
    return g ? { call: typeof g.call, post: typeof g.aiPost, poll: typeof g.aiPoll } : null;
  });
  check("window.goptopHost 已注入", !!shape, JSON.stringify(shape));
  if (!shape) { console.log("\n0 passed, 1 failed"); await ep.close(); process.exit(1); }
  check("call / aiPost / aiPoll 都是函数", shape.call === "function" && shape.post === "function" && shape.poll === "function");

  // 2) 规则命令真的算出了东西（不是空串、不是错误回执）
  const created = JSON.parse(await page.evaluate(() => window.goptopHost.call("game_new", JSON.stringify({ kindJson: JSON.stringify({ Gomoku: { size: 15 } }) }))));
  check("game_new 返回局号", typeof created === "number" && created > 0, `id=${created}`);
  const id = created;

  const placed = await page.evaluate((gid) => window.goptopHost.call("game_place", JSON.stringify({ id: gid, x: 7, y: 7 })), id);
  const p = JSON.parse(placed);
  check("game_place 判定合法并回权威棋盘", p.ok === true && p.board?.[7]?.[7] === "black", placed.slice(0, 80));

  const bad = JSON.parse(await page.evaluate((gid) => window.goptopHost.call("game_place", JSON.stringify({ id: gid, x: 7, y: 7 })), id));
  check("同点重下给稳定错误码", bad.ok === false && !!bad.error, JSON.stringify(bad));

  const size = JSON.parse(await page.evaluate((gid) => window.goptopHost.call("game_board_size", JSON.stringify({ id: gid })), id));
  check("game_board_size = 15", size === 15, String(size));

  // 3) 未知命令有可读回执（设备上静默＝「功能没反应」）
  const unknown = JSON.parse(await page.evaluate(() => window.goptopHost.call("no_such_cmd", "{}")));
  check("未知命令报错而非静默", typeof unknown.error === "string" && unknown.error.includes("unknown command"), JSON.stringify(unknown));

  // 4) AI：post/poll 拿到真结果（这条最慢，也最能证明原生引擎在跑）
  const ai = await page.evaluate(async (gid) => {
    const state = JSON.parse(window.goptopHost.call("game_state_json", JSON.stringify({ id: gid })));
    const ticket = window.goptopHost.aiPost(JSON.stringify({ state, myColor: "Black", budgetMs: 400, wantMove: true }));
    const t0 = Date.now();
    for (;;) {
      const raw = window.goptopHost.aiPoll(ticket);
      if (raw) return { ticket, ms: Date.now() - t0, result: JSON.parse(raw) };
      if (Date.now() - t0 > 12000) return { ticket, ms: Date.now() - t0, result: null };
      await new Promise((r) => setTimeout(r, 50));
    }
  }, id);
  const r = ai.result;
  check("aiPost/aiPoll 拿到分析结果", !!r && !r.error, `票号=${ai.ticket} 耗时=${ai.ms}ms ${JSON.stringify(r).slice(0, 100)}`);
  if (r && !r.error) {
    check("结果含 bestMove 与 0..1 的 winRate", Array.isArray(r.bestMove) && r.bestMove.length === 2 && r.winRate >= 0 && r.winRate <= 1, JSON.stringify({ bm: r.bestMove, wr: r.winRate, nodes: r.nodes }));
    // 分析确实花了时间＝真的在算，而不是立刻返回一个常数
    check("分析耗时与预算量级相符（真在算）", ai.ms >= 200, `${ai.ms}ms`);
  }

  // 5) 规则引擎门面是否挑了原生后端（而非回落 wasm）
  const backend = await page.evaluate(() => ({ hasNative: !!window.goptopHost, hasTauri: "__TAURI_INTERNALS__" in window }));
  check("壳内不具备 Tauri 注入（确认走的是鸿蒙分支）", backend.hasTauri === false, JSON.stringify(backend));

  await page.evaluate((gid) => window.goptopHost.call("game_drop", JSON.stringify({ id: gid })), id);
  console.log(`\n${pass} passed, ${fail} failed`);
  await ep.close();
  process.exit(fail ? 1 : 0);
})().catch((e) => { console.error("PROBE CRASH:", e); process.exit(2); });
