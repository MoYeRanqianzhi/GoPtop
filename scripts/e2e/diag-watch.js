/**
 * diag-watch.js — 三端观战诊断：逐手落子并打印 A/B/C 手数，定位镜像转发丢失。
 * 用法: node diag-watch.js [A规格] [B规格] [C规格]
 */
const { Endpoint, playOneMove } = require("./device.js");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function open(spec, name) {
  let ep;
  if (spec === "android") ep = await Endpoint.android(name, {});
  else if (spec.startsWith("cdp:")) ep = await Endpoint.cdp(name, spec.slice(4));
  else if (spec === "mob") ep = await Endpoint.browser(name, "http://localhost:5173/", { mobile: true });
  else ep = await Endpoint.browser(name, "http://localhost:5173/");
  await ep.ready(40000);
  return ep;
}

(async () => {
  const [specA = "web", specB = "web", specC = "web"] = process.argv.slice(2);
  const A = await open(specA, "A");
  const B = await open(specB, "B");
  const C = await open(specC, "C");
  for (const ep of [A, B]) { await ep.home().catch(() => {}); await ep.enterP2P(); await ep.waitServerReady(); }
  const link = await A.createInvite();
  await B.joinByPaste(link);
  await A.waitSnap((s) => s.phase === "playing", 60000, "A playing");
  await B.waitSnap((s) => s.phase === "playing", 60000, "B playing");

  const spec = await A.inviteSpectate();
  console.log("观战链接:", spec);
  await C.goto(spec);
  await C.waitSnap((s) => s.role === "spectator", 60000, "C 成为观众");
  await C.waitSnap((s) => s.peerConnected, 60000, "C 直连建立");
  console.log("C 已接入观战");

  const counts = async () => {
    const [a, b, c] = await Promise.all([A.snap(), B.snap(), C.snap()]);
    const diff = [];
    for (let y = 0; y < a.size; y++) for (let x = 0; x < a.size; x++) {
      if (a.board[y][x] !== c.board[y][x]) diff.push(`(${x},${y})A=${a.board[y][x]}/C=${c.board[y][x]}`);
    }
    return `A=${a.moveCount} B=${b.moveCount} C=${c.moveCount}${diff.length ? " 差异[" + diff.slice(0, 6).join(" ") + "]" : ""}`;
  };

  const paced = Number(process.env.DIAG_PACED ?? 3);
  for (let i = 0; i < 10; i++) {
    const s = await A.snap();
    const mover = s.toMove === s.myColor ? A : B;
    const mv = await playOneMove(mover);
    if (!mv) { await sleep(300); i--; continue; }
    if (i < paced) { await sleep(1500); console.log(`第${i + 1}手 ${mover.name} 落(${mv.x},${mv.y}) [慢] → ${await counts()}`); }
    else console.log(`第${i + 1}手 ${mover.name} 落(${mv.x},${mv.y}) [快] → ${await counts()}`);
  }
  for (const wait of [3000, 10000, 20000]) {
    await sleep(wait);
    console.log(`等待累计 ${wait / 1000}s 后 → ${await counts()}`);
  }
  await A.close(); await B.close(); await C.close();
  process.exit(0);
})().catch((e) => { console.error("DIAG ERR", e.message); process.exit(1); });
