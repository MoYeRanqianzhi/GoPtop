/**
 * matrix.js — 跨端对局矩阵批跑：逐对执行场景并汇总结果。
 *
 * 用法：
 *   node matrix.js                       # 跑内置全矩阵（device 场景）
 *   node matrix.js chat                  # 用 chat 场景跑全矩阵
 *   node matrix.js game "web|mob" "web|cdp:http://127.0.0.1:9222"
 *
 * 端规格同 match.js：web / mob / android / cdp:<url>。
 * 每对独立进程执行（fail 不影响后续），最终打印汇总表并以退出码反映成败。
 */
const { spawnSync } = require("child_process");
const path = require("path");

/** 内置矩阵：5 个端两两组合 + 同端组合。 */
const ENDS = {
  "web-d": "web",
  "web-m": "mob",
  desktop: "cdp:http://127.0.0.1:9222",
  desktop2: "cdp:http://127.0.0.1:9223",
  android: "android",
  harmony: "cdp:http://127.0.0.1:9444",
};

const DEFAULT_PAIRS = [
  ["web-d", "web-m"],
  ["web-d", "desktop"],
  ["web-d", "android"],
  ["web-d", "harmony"],
  ["web-m", "desktop"],
  ["web-m", "android"],
  ["web-m", "harmony"],
  ["desktop", "desktop2"],
  ["desktop", "android"],
  ["desktop", "harmony"],
  ["android", "harmony"],
];

(async () => {
  const args = process.argv.slice(2);
  const scenario = args[0] && ["game", "chat", "watch"].includes(args[0]) ? args[0] : "game";
  const rest = args[0] && ["game", "chat", "watch"].includes(args[0]) ? args.slice(1) : args;
  let pairs = DEFAULT_PAIRS;
  if (rest.length) {
    // 形如 "web-d|web-m"：显式指定端名对
    pairs = rest.map((s) => s.split("|"));
  }

  const results = [];
  for (const [a, b] of pairs) {
    const specA = ENDS[a] || a;
    const specB = ENDS[b] || b;
    const label = `${a} ↔ ${b}`;
    console.log(`\n########## ${label} (${scenario}) ##########`);
    const t0 = Date.now();
    const r = spawnSync(process.execPath, [path.join(__dirname, "match.js"), scenario, specA, specB], {
      stdio: "inherit",
      timeout: 12 * 60 * 1000,
      env: process.env,
    });
    const ok = r.status === 0;
    results.push({ label, ok, secs: Math.round((Date.now() - t0) / 1000) });
    console.log(`########## ${label} → ${ok ? "PASS" : "FAIL"} (${Math.round((Date.now() - t0) / 1000)}s) ##########`);
  }

  console.log("\n================ 矩阵汇总 ================");
  for (const r of results) console.log(`${r.ok ? "PASS" : "FAIL"}  ${r.label.padEnd(22)} ${r.secs}s`);
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n合计 ${results.length} 对：${results.length - failed} 通过，${failed} 失败`);
  process.exit(failed ? 1 : 0);
})();
