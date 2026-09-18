/**
 * device.js — 跨端实机测试端点抽象（web 桌面浏览器 / web 手机浏览器 / Tauri 桌面壳 /
 * Android WebView / HarmonyOS ArkWeb）。
 *
 * 设计要点：
 * - **全部交互走真实输入事件**（mouse.click / touchscreen.tap / 键盘键入），不调
 *   `window.__session.place()` 这类内部 API 直达状态机——那是「取巧」，测不到 UI 接线。
 *   快照（`window.__session.snapshot()`）只用于**读取**与断言，不用于写入。
 * - 三类端点统一为同一套方法：
 *   - `browser`：Playwright 自起 chromium（可开手机仿真）；
 *   - `cdp`：connectOverCDP 接管已运行的 WebView（桌面壳开 remote-debugging-port；
 *     安卓经 adb forward；鸿蒙经 hdc fport + ArkWeb inspector）。
 * - 棋盘是 SVG（role="grid"），落子 = 把格点换算成 client 坐标后点真实鼠标/手指。
 *
 * 用法见 match.js；单端调试可直接 node -e 引本文件。
 */
const { chromium, devices } = require("playwright");

/** 棋盘几何常量，必须与 frontend/src/components/BoardSvg.tsx 保持一致。 */
const BOARD_PAD = 30;
const BOARD_CELL = 36;

/** 各端 UI 文案（唯一真源在 frontend/src；此处集中一处便于文案改动时同步）。 */
const UI = {
  p2pEntry: "P2P 对战",       // 菜单页入口（MenuPage.tsx）
  menuEntry: "菜单",          // 顶栏回菜单
  createInvite: "开启对战（等对手）",
  pasteInvite: "粘贴邀请链接",
  pasteSubmit: "连接",
  copyInvite: "复制",
  chatOpenTitle: "聊天 / 悔棋 / 重开 / 换棋",
  chatPlaceholder: "说点什么…",
  chatSend: "发送",
  undo: "悔棋",
  reset: "重开",
  swap: "换棋",
  approve: "同意",
  decline: "拒绝",
  inviteSpectate: "邀请观战",
  copySpecLink: "复制观战链接",
  backHome: "离开",
};

class Endpoint {
  /**
   * @param {string} name 端点名（日志用）
   * @param {import('playwright').Page} page
   * @param {{mobile?: boolean, tap?: boolean}} [opts]
   */
  constructor(name, page, opts = {}) {
    this.name = name;
    this.page = page;
    this.mobile = !!opts.mobile;
    this.tap = !!opts.tap;
    /** 页面控制台错误（断言「无 JS 异常」用）。 */
    this.errors = [];
    page.on("pageerror", (e) => this.errors.push(String(e).slice(0, 200)));
  }

  /* ---------------- 构造 ---------------- */

  /** 自起 chromium（桌面或手机仿真）。url 为空则只开空白页。 */
  static async browser(name, url, { mobile = false, device = null, viewport = null } = {}) {
    const browser = await chromium.launch({ headless: process.env.XDEV_HEADFUL !== "1" });
    const dev = device || (mobile ? devices["Pixel 5"] : null);
    const ctx = await browser.newContext(dev ? { ...dev } : { viewport: viewport || { width: 1400, height: 950 } });
    const page = await ctx.newPage();
    const ep = new Endpoint(name, page, { mobile: !!dev, tap: !!dev?.hasTouch });
    ep._browser = browser;
    if (url) await page.goto(url, { waitUntil: "domcontentloaded" });
    return ep;
  }

  /**
   * 安卓真机/模拟器：走 Playwright 的 Android 驱动（adb 直连，拿到的是真实 Page，
   * 因此与浏览器端点共用同一套定位器与输入事件）。
   * 注意：这里**不能**用 connectOverCDP——安卓 WebView 的 CDP 只有 page 域，
   * Playwright 建连时会调 Browser.setDownloadBehavior 直接失败。
   */
  static async android(name, { serial = null, pkg = "com.goptop.app" } = {}) {
    const { _android } = require("playwright");
    const devices = await _android.devices();
    const dev = serial ? devices.find((d) => d.serial() === serial) : devices[0];
    if (!dev) throw new Error(`未找到安卓设备 ${serial || "(默认)"}`);
    let target = null;
    for (const wv of await dev.webViews()) {
      if ((await wv.pkg()) === pkg) target = wv;
    }
    if (!target) throw new Error(`安卓上没有 ${pkg} 的 WebView（应用是否已启动？）`);
    const page = await target.page();
    const ep = new Endpoint(name, page, { mobile: true, tap: true });
    ep._android = dev;
    return ep;
  }

  /** 接管已运行的 WebView（cdpUrl 形如 http://127.0.0.1:9222）。 */
  static async cdp(name, cdpUrl, { mobile = false } = {}) {
    const browser = await chromium.connectOverCDP(cdpUrl, { timeout: 30000 });
    // 取第一个真实页面（壳应用只有一个 WebView；忽略 about:blank/devtools 页）。
    let page = null;
    for (const ctx of browser.contexts()) {
      for (const p of ctx.pages()) {
        const u = p.url();
        if (u && u !== "about:blank") { page = p; break; }
      }
      if (page) break;
    }
    if (!page) throw new Error(`[${name}] CDP 已连上但找不到页面`);
    const ep = new Endpoint(name, page, { mobile });
    ep._browser = browser;
    return ep;
  }

  async close() {
    // 安卓端点：只断开设备连接，**不要**关 page——关了等于杀掉应用 WebView。
    if (this._android) {
      try { await this._android.close?.(); } catch { /* ignore */ }
      return;
    }
    try { await this._browser?.close(); } catch { /* 壳连接断开可忽略 */ }
  }

  /* ---------------- 基础读取 ---------------- */

  /** 等 wasm 会话挂载完成（壳/浏览器统一就绪信号）。 */
  async ready(timeout = 30000) {
    await this.page.waitForFunction(() => !!(window.__session && window.__session.snapshot), null, { timeout, polling: 200 });
    // 首帧快照可能仍是空壳，再等 userId 落地。
    await this.waitSnap((s) => !!s.userId, timeout, "会话就绪");
    return this;
  }

  async snap() {
    return JSON.parse(await this.page.evaluate(() => window.__session.snapshot()));
  }

  /** 轮询快照直到 pred 成立；超时抛错并带最后快照摘要。
   *  刚导航（打开链接）时 wasm 会话尚未挂载，evaluate 会读不到 __session——按未就绪继续轮询。 */
  async waitSnap(pred, timeout, label = "条件") {
    const t0 = Date.now();
    let last = null;
    while (Date.now() - t0 < timeout) {
      try {
        last = await this.snap();
      } catch {
        await this.page.waitForTimeout(200);
        continue;
      }
      if (pred(last)) return last;
      await this.page.waitForTimeout(120);
    }
    throw new Error(`[${this.name}] 等「${label}」超时(${timeout}ms)；末态 phase=${last?.phase} role=${last?.role} moves=${last?.moveCount} toMove=${last?.toMove} winner=${last?.winner} serverState=${last?.serverState} peerConnected=${last?.peerConnected}`);
  }

  async waitText(text, timeout = 15000) {
    await this.page.waitForFunction(
      (t) => document.body.innerText.includes(t),
      text,
      { timeout, polling: 200 },
    );
  }

  /* ---------------- 点击原语（真实输入） ---------------- */

  /** 点第一个可见且文案含 text 的 button。 */
  async clickButton(text, timeout = 15000) {
    const loc = this.page.locator("button:visible", { hasText: text }).first();
    await loc.waitFor({ state: "visible", timeout });
    await loc.click({ timeout });
    return this;
  }

  /** 点带指定 title 的 button（聊天入口等无文案按钮）。 */
  async clickByTitle(title, timeout = 15000) {
    const loc = this.page.locator(`button[title="${title}"]:visible`).first();
    await loc.waitFor({ state: "visible", timeout });
    await loc.click({ timeout });
    return this;
  }

  /** 点第一个可见且文案含 text 的元素（非 button，如链接/页签）。 */
  async clickText(text, timeout = 15000) {
    const loc = this.page.locator(`:visible`, { hasText: text }).first();
    await loc.waitFor({ state: "visible", timeout });
    await loc.click({ timeout });
    return this;
  }

  /** 向输入框键入（先清空）。selector 支持 placeholder 文本。 */
  async typeInto(selector, text, { enter = false } = {}) {
    const loc = this.page.locator(`${selector}:visible`).first();
    await loc.waitFor({ state: "visible", timeout: 15000 });
    await loc.click();
    await loc.fill("");
    await loc.type(text, { delay: 15 });
    if (enter) await loc.press("Enter");
    return this;
  }

  /* ---------------- 页面导航 ---------------- */

  /** 浏览器端点：真实地址栏导航（等价用户打开链接）。 */
  async goto(url) {
    await this.page.goto(url, { waitUntil: "domcontentloaded" });
    return this;
  }

  /** 回到主页（菜单页）。对局中/等待中都能用——按「离开/取消」直至 phase=home。 */
  async home(timeout = 20000) {
    const t0 = Date.now();
    while (Date.now() - t0 < timeout) {
      const s = await this.snap();
      if (s.phase === "home" && s.role === "idle") return this;
      let clicked = false;
      for (const t of [UI.backHome, "取消", "返回"]) {
        const b = this.page.locator("button:visible", { hasText: t }).first();
        if (await b.count() && await b.isVisible().catch(() => false)) {
          await b.click().catch(() => {});
          clicked = true;
          break;
        }
      }
      if (!clicked) {
        // 已在菜单页但没有「离开」按钮：点顶栏「菜单」兜底。
        await this.clickButton(UI.menuEntry).catch(() => {});
      }
      await this.page.waitForTimeout(400);
    }
    return this;
  }

  /** 进对战大厅（P2P 页）。浏览器端点已直达；壳端点走「菜单 → P2P 对战」真实点击。 */
  async enterP2P() {
    const path = await this.page.evaluate(() => location.pathname);
    if (path !== "/p2p") {
      const entry = this.page.locator("button", { hasText: UI.p2pEntry }).first();
      if (await entry.count() && await entry.isVisible().catch(() => false)) {
        await entry.click();
      } else {
        await this.clickButton(UI.menuEntry);
        await this.page.waitForTimeout(300);
        await this.clickButton(UI.p2pEntry);
      }
      await this.page.waitForFunction(() => location.pathname === "/p2p", null, { timeout: 15000, polling: 150 });
    }
    return this;
  }

  /** 服务器模式下等信令连接就绪（开局的前置条件）。 */
  async waitServerReady(timeout = 40000) {
    const s = await this.snap();
    if (!s.serverMode) return this;
    await this.waitSnap((x) => x.serverState === "ready", timeout, "信令服务器已连接");
    return this;
  }

  /* ---------------- 大厅/邀请/接入 ---------------- */

  /** 开局等对手；返回邀请链接（服务器模式免回执，链接由 userId 构成）。 */
  async createInvite() {
    await this.clickButton(UI.createInvite);
    const s = await this.waitSnap((x) => (x.inviteUrl || x.specUrl), 40000, "邀请链接生成");
    return s.inviteUrl || "";
  }

  /** 粘贴链接加入（壳应用与浏览器通用的「人类路径」）。 */
  async joinByPaste(link) {
    await this.clickButton(UI.pasteInvite);
    await this.typeInto('input[placeholder*="粘贴邀请链接"]', link);
    await this.clickButton(UI.pasteSubmit);
    return this;
  }

  /** 读当前观战链接（无服务器模式才有 specrtc；服务器模式为 watch 链接）。 */
  async specLink() {
    const s = await this.snap();
    return s.specUrl || s.watchUrl || "";
  }

  /** 邀请观战：点「邀请观战」把观战链接写入剪贴板钩子，再从快照取链接文本。 */
  async inviteSpectate() {
    await this.clickButton(UI.inviteSpectate);
    const s = await this.waitSnap((x) => !!(x.specUrl || x.watchUrl), 30000, "观战链接");
    return s.specUrl || s.watchUrl || "";
  }

  /* ---------------- 对局操作 ---------------- */

  /**
   * 落子：把格点换算成 client 坐标，发真实鼠标/触摸事件。
   * 坐标换算与 BoardSvg.coordFromEvent 严格互逆（同一套 pad/cell/viewBox）。
   */
  async place(gx, gy) {
    // 窄屏聊天弹窗若还开着会盖住棋盘，点击会被遮罩吃掉——先收起（等价用户点空白处）。
    await this.closeChat();
    // 先把棋盘滚进视口：mouse.click 用的是视口坐标，棋盘在折叠线下会点空
    //（短窗口下加了计分条后踩到）。等价用户先把棋盘滑到眼前。
    await this.page.locator('svg[role="grid"]').first().scrollIntoViewIfNeeded().catch(() => {});
    const info = await this.page.evaluate(([x, y]) => {
      const svg = document.querySelector('svg[role="grid"]');
      if (!svg) return null;
      const vbArr = (svg.getAttribute("viewBox") || "0 0 600 600").split(/\s+/).map(Number);
      const vb = vbArr[2];
      const r = svg.getBoundingClientRect();
      const scale = vb / r.width;
      const pad = 30, cell = 36;
      return {
        cx: r.left + (pad + x * cell) / scale,
        cy: r.top + (pad + y * cell) / scale,
        size: Number((svg.getAttribute("aria-label") || "").match(/(\d+)x\1/)?.[1] || 15),
        rect: { w: r.width, h: r.height },
      };
    }, [gx, gy]);
    if (!info) throw new Error(`[${this.name}] 找不到棋盘 SVG`);
    if (!info.rect.w || !info.rect.h) throw new Error(`[${this.name}] 棋盘尺寸为 0（未渲染）`);
    if (gx >= info.size || gy >= info.size) throw new Error(`[${this.name}] 落子越界 (${gx},${gy}) 盘=${info.size}`);
    if (this.tap) {
      // 移动端优先真手指（touch 事件）；宿主上下文没开 hasTouch 时退回鼠标。
      try { await this.page.touchscreen.tap(info.cx, info.cy); }
      catch { await this.page.mouse.click(info.cx, info.cy); }
    } else {
      await this.page.mouse.click(info.cx, info.cy);
    }
    return this;
  }

  /** 打开聊天面板（协商动作也在里面）。 */
  async openChat() {
    const s = await this.snap();
    if (s.phase !== "playing") return this;
    // 幂等：窄屏聊天是弹窗，弹窗一开就会把入口按钮盖住——再点一次必然被遮罩拦截。
    const modal = this.page.locator(".chat-modal-bg").first();
    if (await modal.count() && await modal.isVisible().catch(() => false)) return this;
    // 窄屏（手机）聊天是弹窗；宽屏是侧栏——两种情况都点同一个入口按钮。
    const btn = this.page.locator(`button[title="${UI.chatOpenTitle}"]:visible`).first();
    if (await btn.count() && await btn.isVisible().catch(() => false)) {
      await btn.click();
      await this.page.waitForTimeout(250);
    }
    return this;
  }

  /**
   * 聊天面板作用域：放得下是右侧停靠栏（`.chat-dock`），放不下是 `.chat-modal-bg` 弹窗。
   * 两者互斥渲染（2026-09-19 起），但作用域仍按「有弹窗就只在弹窗内找」：
   * 弹窗覆盖在页面上，全局第一个匹配可能落到被遮罩挡住的元素上
   * （Playwright 报「intercepts pointer events」）。
   */
  async chatScope() {
    const modal = this.page.locator(".chat-modal-bg").first();
    if (await modal.count() && await modal.isVisible().catch(() => false)) return modal;
    return this.page;
  }

  /** 在聊天面板作用域内点按钮。 */
  async clickInChat(text, timeout = 15000) {
    const scope = await this.chatScope();
    const loc = scope.locator("button:visible", { hasText: text }).first();
    await loc.waitFor({ state: "visible", timeout });
    await loc.click({ timeout });
    return this;
  }

  /** 发聊天消息（真实键入 + 点发送）。 */
  async sendChat(text) {
    await this.openChat();
    const scope = await this.chatScope();
    const input = scope.locator(`input[placeholder="${UI.chatPlaceholder}"]`).first();
    await input.waitFor({ state: "visible", timeout: 10000 });
    await input.click();
    await input.type(text, { delay: 15 });
    await this.clickInChat(UI.chatSend);
    return this;
  }

  /**
   * 收起窄屏聊天弹窗（点遮罩空白处，等价用户点一下面板外）。
   * 发起协商的一方发完请求后弹窗仍开着，会把棋盘整个盖住——后续落子点击全被遮罩吃掉。
   */
  async closeChat() {
    const modal = this.page.locator(".chat-modal-bg").first();
    if (!(await modal.count()) || !(await modal.isVisible().catch(() => false))) return this;
    const box = await modal.boundingBox();
    if (box) {
      // 面板居中且有 16px padding，四角必是遮罩而非面板。
      const pt = [box.x + 4, box.y + 4];
      if (this.tap) {
        try { await this.page.touchscreen.tap(pt[0], pt[1]); }
        catch { await this.page.mouse.click(pt[0], pt[1]); }
      } else {
        await this.page.mouse.click(pt[0], pt[1]);
      }
    }
    await this.page.waitForTimeout(300);
    return this;
  }

  /** 协商请求（悔棋/重开/换棋）——在聊天面板内；发完即收起面板，回到棋盘。 */
  async negotiate(kind) {
    await this.openChat();
    await this.clickInChat(UI[kind]);
    await this.closeChat();
    return this;
  }

  /** 同意/拒绝弹窗（ConfirmBanner，主页与对局页均在顶层显示）。 */
  async approve() {
    await this.clickButton(UI.approve);
    return this;
  }

  async decline() {
    await this.clickButton(UI.decline);
    return this;
  }

  /** 等轮到自己落子（对局中且 toMove === myColor 且已连接）。 */
  async waitMyTurn(timeout = 60000) {
    return this.waitSnap(
      (s) => s.phase === "playing" && !s.winner && s.toMove === s.myColor && s.peerConnected,
      timeout,
      "轮到我落子",
    );
  }

  /** 等对局结束并返回胜者。 */
  async waitWinner(timeout = 600000) {
    const s = await this.waitSnap((x) => !!x.winner, timeout, "决出胜负");
    return s.winner;
  }

  /** 一行摘要（日志）。 */
  async brief() {
    const s = await this.snap();
    return `${this.name}[${s.role}/${s.phase} moves=${s.moveCount} toMove=${s.toMove} winner=${s.winner ?? "-"} peer=${s.peerConnected ? "on" : "off"}]`;
  }
}

/* ================= 五子棋对局策略（真实落子，非取巧） =================
 * 双方都用同一套启发式：能赢就赢 → 挡对方成五 → 否则按「己方进攻分 + 对方威胁分」
 * 选点。这样下出来是**真实且有来有回的对局**，通常数十手后才因双威胁分出胜负。 */

const DIRS = [[1, 0], [0, 1], [1, 1], [1, -1]];

/**
 * 假定在该点落 color 子后，分析四方向形态。
 * 返回 {score, five, fours, openThrees}——关键在于**双威胁**识别：
 * 「四 + 活三」「双四」「双活三」都是对手挡不住的必胜形，必须给远高于单威胁的分。
 */
function analyze(board, x, y, color, size) {
  let five = false, fours = 0, openThrees = 0, best = 0;
  for (const [dx, dy] of DIRS) {
    let cnt = 1, openA = 0, openB = 0;
    for (let i = 1; i < 5; i++) {
      const nx = x + dx * i, ny = y + dy * i;
      if (nx < 0 || ny < 0 || nx >= size || ny >= size) break;
      if (board[ny][nx] === color) cnt++;
      else { if (board[ny][nx] === "empty") openA = 1; break; }
    }
    for (let i = 1; i < 5; i++) {
      const nx = x - dx * i, ny = y - dy * i;
      if (nx < 0 || ny < 0 || nx >= size || ny >= size) break;
      if (board[ny][nx] === color) cnt++;
      else { if (board[ny][nx] === "empty") openB = 1; break; }
    }
    const open = openA + openB;
    if (cnt >= 5) five = true;
    else if (cnt === 4) { if (open >= 1) { fours++; best = Math.max(best, open === 2 ? 1e7 : 1e6); } }
    else if (cnt === 3) { if (open === 2) { openThrees++; best = Math.max(best, 1e4); } else if (open === 1) best = Math.max(best, 1e3); }
    else if (cnt === 2) best = Math.max(best, open === 2 ? 200 : 20);
    else best = Math.max(best, open === 2 ? 5 : 1);
  }
  let score;
  if (five) score = 1e9;                                   // 直接成五
  else if (fours >= 1 && openThrees >= 1) score = 5e8;      // 四三 —— 挡不住的必胜
  else if (fours >= 2) score = 5e8;                         // 双四
  else if (openThrees >= 2) score = 1e8;                    // 双活三
  else if (fours >= 1) score = 1e6;                         // 单四（对手必须挡）
  else if (openThrees >= 1) score = 1e4;                    // 活三（对手应当挡）
  else score = best;
  return { score, five, fours, openThrees };
}

/**
 * 选点：返回 {x,y}。
 * 决策分层：能成五就成五 → 挡对方成五 → 造自己的双威胁 → 挡对方双威胁 → 单威胁 → 形态分。
 *
 * `level` 是**棋力档位**，模拟两名水平不同的真人：
 * - `"sharp"`：看得见双威胁（四三 / 双四 / 双活三），能主动做杀；
 * - `"solid"`：只挡成五、四、单活三，看不见双威胁——稳健但会被杀棋击穿。
 * 实测同等棋力互相封死会一路下到满盘和棋（225 手），无法「决出胜负」；
 * 两档不同棋力才是真实对局形态，也才能收敛出胜者。
 */
function pickMove(board, me, size, level = "sharp") {
  const opp = me === "black" ? "white" : "black";
  const casual = level === "casual";
  const stones = board.reduce((n, row) => n + row.reduce((m, c) => m + (c === "empty" ? 0 : 1), 0), 0);
  /** 业余棋手看漏活三的确定性伪随机（同一局面可复现，便于失败复跑）。 */
  const miss = (x, y) => (((x * 73856093) ^ (y * 19349663) ^ (stones * 83492791)) >>> 0) % 100 < 45;
  /** 把形态分析压成决策分；casual 档不认双威胁、且会看漏活三。 */
  const value = (a, x, y, asThreat) => {
    if (a.five) return 1e9;
    if (casual) {
      if (a.fours >= 1) return 1e6;
      if (a.openThrees >= 1) return asThreat && miss(x, y) ? 0 : 1e4;
      return Math.min(a.score, 200);
    }
    return a.score; // sharp：四三 / 双四 / 双活三都能看见（5e8 / 1e8）
  };
  let best = null, bestScore = -Infinity;
  const center = (size - 1) / 2;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      if (board[y][x] !== "empty") continue;
      // 邻近性剪枝：附近 2 格内有子才考虑（远点无意义且拖慢）
      let near = false;
      for (let dy = -2; dy <= 2 && !near; dy++) {
        for (let dx = -2; dx <= 2; dx++) {
          const nx = x + dx, ny = y + dy;
          if (nx >= 0 && ny >= 0 && nx < size && ny < size && board[ny][nx] !== "empty") { near = true; break; }
        }
      }
      if (!near && stones > 0) continue;
      const mine = value(analyze(board, x, y, me, size), x, y, false);
      const theirs = value(analyze(board, x, y, opp, size), x, y, true);
      // 对手威胁不打折（挡不住就输）；自己进攻略加权，促成棋局收束。
      let sc = Math.max(mine * 1.05, theirs);
      // 同分时靠天元，避免棋子散乱导致对局无限拉长
      sc -= (Math.abs(x - center) + Math.abs(y - center)) * 0.5;
      if (sc > bestScore) { bestScore = sc; best = { x, y }; }
    }
  }
  return best;
}

/**
 * 在端点上走一手真实棋（轮到它时调用）。
 * 返回落子坐标；无处可下返回 null。
 */
async function playOneMove(ep, level = "sharp") {
  const s = await ep.snap();
  if (s.phase !== "playing" || s.winner || s.toMove !== s.myColor) return null;
  const mv = pickMove(s.board, s.myColor, s.size, level);
  if (!mv) return null;
  await ep.place(mv.x, mv.y);
  return mv;
}

module.exports = { Endpoint, UI, playOneMove, pickMove, analyze, BOARD_PAD, BOARD_CELL };
