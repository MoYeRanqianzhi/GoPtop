/**
 * diag-pair.js — 配对诊断：两端配对后持续打印快照关键字段与 ICE 状态。
 * 用法: node diag-pair.js <A规格> <B规格> [秒数]
 */
const { Endpoint } = require("./device.js");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function open(spec, name) {
  let ep;
  if (spec === "android" || spec.startsWith("android:")) {
    ep = await Endpoint.android(name, { serial: spec.includes(":") ? spec.slice(8) : null });
  } else if (spec.startsWith("cdp:")) {
    ep = await Endpoint.cdp(name, spec.slice(4));
  } else {
    ep = await Endpoint.browser(name, "http://localhost:5173/");
  }
  await ep.ready(40000);
  return ep;
}

const brief = (s) => `role=${s.role} phase=${s.phase} my=${s.myColor} peer=${s.peerConnected ? "on" : "off"} server=${s.serverState} moves=${s.moveCount} confirm=${s.confirmReq ? s.confirmReq.kind : "-"} win=${s.winner ?? "-"}`;

(async () => {
  const [specA, specB, secs] = process.argv.slice(2);
  const A = await open(specA, "A");
  const B = await open(specB, "B");
  for (const ep of [A, B]) {
    await ep.home().catch(() => {});
    await ep.enterP2P();
    await ep.waitServerReady();
  }
  const link = await A.createInvite();
  console.log("A 邀请链接:", link);
  console.log("A userId:", (await A.snap()).userId, " B userId:", (await B.snap()).userId);
  // B 粘贴链接（壳端点路径）
  await B.joinByPaste(link);
  const total = Number(secs || 30);
  for (let i = 0; i <= total; i += 3) {
    const [sa, sb] = [await A.snap(), await B.snap()];
    const ia = JSON.parse(await A.page.evaluate(() => window.__session.ice_debug())).map((p) => `${p.tag}:${p.ice}/dc=${p.dcState}`);
    const ib = JSON.parse(await B.page.evaluate(() => window.__session.ice_debug())).map((p) => `${p.tag}:${p.ice}/dc=${p.dcState}`);
    console.log(`t=${i}s | A ${brief(sa)} ice=[${ia.join(",")}]`);
    console.log(`       | B ${brief(sb)} ice=[${ib.join(",")}]`);
    await sleep(3000);
  }
  await A.close();
  await B.close();
  process.exit(0);
})().catch((e) => { console.error("DIAG ERR", e.message); process.exit(1); });
