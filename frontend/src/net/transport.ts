/**
 * net/transport — 用户对用户 P2P 直连层（无服务器、无中转）。
 *
 * 模型（按需求）：
 * - 每个标签页即一个用户：`userId` 存 `sessionStorage`（每页不同），昵称可改。
 * - 选项页（`/`）：本地对战 / P2P 对战 / 在线用户 / 设置四个入口。
 * - 用户主页（`/<userId>`）；邀请链接（`/<inviterId>?pwd=<pwd>`）：对端带 pwd 打开即视为
 *   "带钥匙的连接请求"，邀请者校验 pwd 正确则自动同意，无需手动点接受。
 * - 无 pwd 访问某用户主页则只看到其主页，可手动发起挑战，对方弹窗同意后才进对局。
 * - pwd 只在"已开战但无对手（waiting）"时有效；两人进对局后 pwd 即失效，不存在第三人凭旧 pwd 加入。
 *   建连成功后邀请组件隐藏，转而显示观战链接（`/watch/<game>`）。
 * - 观战：观战者只收 `SyncState`，不可落子；同源观战走同 game channel。
 *   跨设备观战**尚未实现**（/watch 链接在别的设备打开收不到棋局）——待办见
 *   .agents/TODO.md，届时观战者与邀请者建立独立 WebRTC 直连。
 * - 跨设备信令：用户只传一次邀请链接。offer/answer 编码进邀请 URL 的 `&rtc=` 参数
 *   （同源页面间经 Presence 自动回传）；跨设备时受邀者把回执链接发给邀请者，邀请者在
 *   等待页点「输入回执」粘贴即可——全程弹窗粘贴，不依赖页面导航，Tauri 桌面壳同样可用。
 * - 回执/邀请链接的解析与域名无关：粘贴文本只取路径与查询参数，任意域名都能识别。
 * - 分享链接的基地址自动决定：Web 用当前站点；Tauri 等非 Web 用云端部署地址
 *   `SHARE_ORIGIN_NATIVE`（`goptop.pages.dev`）。用户不可见、不可设置。
 *
 * 直连说明（必须直连、禁用中转）：
 * - 同源页面间：`BroadcastChannel` 为同源端到端直传（浏览器内共享内存，不经过任何服务器/中转）。
 * - 跨设备：`RTCPeerConnection` 仅配 STUN（NAT 地址发现，不转发数据），**不配任何 TURN**，
 *   数据只走主机候选（host/srflx）直连。STUN 只做地址发现：默认国服 A/B 区，
 *   外服（谷歌）仅备选，用户可在设置页开关线路或添加自定义 STUN。
 */

export type StoneColor = "empty" | "black" | "white";
export type Coord = { x: number; y: number };
export type GameKind = "gomoku" | "go";
export type Size = 9 | 13 | 15 | 19;

export type Move =
  | { type: "Place"; coord: Coord }
  | { type: "Pass" }
  | { type: "Resign" };

export type MsgKind =
  | { type: "Hello"; kind: GameKind; size: Size; name?: string }
  // by：发送端声明的执子颜色。接收端（尤其观战者，没有"我的颜色"可用）据此判定，
  // 不得从本地 toMove/myColor 推断——历史 bug：任何一方认输，观战者都判白胜。
  | { type: "Move"; move: Move; by: StoneColor }
  | { type: "SyncState"; board: StoneColor[][]; toMove: StoneColor; winner: StoneColor | null; history: Coord[]; lastMove: Coord | null; kind: GameKind; size: Size }
  | { type: "SyncRequest" }
  | { type: "Chat"; text: string }
  | { type: "Ping" }
  | { type: "Pong" }
  | { type: "Reset"; kind: GameKind; size: Size }
  | { type: "UndoReq" }
  | { type: "UndoAck"; ok: boolean };

export type GameMsg = {
  seq: number;
  sender: string;
  /** 发送者的 userId，用于区分选手/观战者。 */
  userId: string;
  kind: MsgKind;
};

/* ---------------- 身份 ---------------- */

/** 本页的用户 id（sessionStorage，每页不同）。 */
export function myUserId(): string {
  try {
    let id = sessionStorage.getItem("goptop:tabUser");
    if (!id) {
      id = `u-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
      sessionStorage.setItem("goptop:tabUser", id);
    }
    return id;
  } catch {
    return `u-${Math.random().toString(36).slice(2, 10)}`;
  }
}

export function myName(): string {
  try {
    return localStorage.getItem("goptop:name") || "";
  } catch {
    return "";
  }
}

export function setMyName(n: string) {
  try {
    localStorage.setItem("goptop:name", n);
  } catch { /* ignore */ }
}

/** 每局轮换的一次性 pwd（邀请钥匙）。CSPRNG 生成，固定 6 位 base36。
 *  pwd 同时是信令编码密钥（encodeRtcPayload），必须不可预测且长度稳定。 */
export function genPwd(): string {
  const buf = new Uint32Array(1);
  crypto.getRandomValues(buf);
  return (buf[0] % 2176782336).toString(36).padStart(6, "0"); // 36^6，取模偏差 ~2.7% 可接受
}

/** 每局轮换的 gameId（观战 channel 后缀）。 */
export function genGameId(): string {
  return `g-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

/* ---------------- STUN 线路（仅 NAT 地址发现，不转发数据） ----------------
 *
 * - 国服 A 区：`stun:stun.miwifi.com:3478`（默认启用）
 * - 国服 B 区：`stun:stun.chat.bilibili.com:3478`（默认启用）
 * - 外服（谷歌）：`stun:stun.l.google.com:19302`（默认关闭，国内不可用时可手动打开）
 * - 用户可在设置页开关各线路、添加自定义 STUN，保存在 localStorage。
 */

export type StunLine = { id: string; label: string; urls: string; builtin: boolean; enabled: boolean };

export const BUILTIN_STUN: Omit<StunLine, "enabled">[] = [
  { id: "cn-a", label: "国服A区", urls: "stun:stun.miwifi.com:3478", builtin: true },
  { id: "cn-b", label: "国服B区", urls: "stun:stun.chat.bilibili.com:3478", builtin: true },
  { id: "foreign", label: "外服", urls: "stun:stun.l.google.com:19302", builtin: true },
];

const STUN_KEY = "goptop:stun";

function defaultStunLines(): StunLine[] {
  return BUILTIN_STUN.map((b) => ({ ...b, enabled: b.id !== "foreign" }));
}

/** 当前启用的 STUN 线路（含用户自定义）。 */
export function loadStunLines(): StunLine[] {
  try {
    const raw = localStorage.getItem(STUN_KEY);
    if (!raw) return defaultStunLines();
    const arr = JSON.parse(raw) as StunLine[];
    if (!Array.isArray(arr) || arr.length === 0) return defaultStunLines();
    // 与内置线路合并：保留用户新增，补齐新增内置项
    const byId = new Map(arr.filter((l) => l && typeof l.urls === "string").map((l) => [l.id, l]));
    for (const b of BUILTIN_STUN) {
      if (!byId.has(b.id)) byId.set(b.id, { ...b, enabled: b.id !== "foreign" });
    }
    return [...byId.values()];
  } catch {
    return defaultStunLines();
  }
}

export function saveStunLines(lines: StunLine[]) {
  try {
    localStorage.setItem(STUN_KEY, JSON.stringify(lines));
  } catch { /* ignore */ }
}

/** 组装 RTCPeerConnection 的 iceServers（只用启用的线路；全关则走纯 host 候选局域网直连）。 */
export function stunServers(): { urls: string }[] {
  return loadStunLines().filter((l) => l.enabled).map((l) => ({ urls: l.urls }));
}

/* ---------------- URL 模型（路径风格） ----------------
 *
 * - `/`              选项页（本地对战 / P2P 对战 / 在线用户 / 设置）
 * - `/local`         本地对战
 * - `/p2p`           P2P 对战大厅
 * - `/users`         在线用户
 * - `/settings`      设置（含 STUN 线路：国服A区/国服B区/外服/自定义）
 * - `/<userId>`      用户主页；邀请链接自动含信令：
 *   `?pwd=&kind=&size=`（同源直传）或 `?pwd=&kind=&size=&rtc=<offer>`（跨设备一键直连）
 * - `/watch/<game>`  观战
 * - 兼容旧链接：`?room=` 视为观战旧房间；`?u=` 视为用户主页；`?watch=` 视为观战。
 */

export type UrlIntent =
  | { mode: "menu" }
  | { mode: "local" }
  | { mode: "p2p" }
  | { mode: "users" }
  | { mode: "settings" }
  | { mode: "user"; userId: string; pwd: string | null; kind: GameKind; size: Size; rtc: string | null }
  | { mode: "watch"; gameId: string };

function kindSizeFromParams(sp: URLSearchParams): { kind: GameKind; size: Size } {
  const kind: GameKind = sp.get("kind") === "go" ? "go" : "gomoku";
  const sizeRaw = Number(sp.get("size"));
  const size = ([9, 13, 15, 19] as number[]).includes(sizeRaw) ? (sizeRaw as Size) : undefined;
  return { kind, size: size ?? (kind === "go" ? 19 : 15) };
}

/** 解析当前 URL 为路由意图（路径优先，旧 query 兼容）。 */
export function parseUrl(): UrlIntent {
  try {
    const url = new URL(window.location.href);
    const sp = url.searchParams;
    const segs = url.pathname.split("/").filter(Boolean);
    if (segs.length === 0) {
      // 旧链接兼容
      const room = sp.get("room");
      if (room) return { mode: "watch", gameId: room };
      const u = sp.get("u");
      if (u) {
        const { kind, size } = kindSizeFromParams(sp);
        return { mode: "user", userId: u, pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc") };
      }
      const watch = sp.get("watch");
      if (watch) return { mode: "watch", gameId: watch };
      return { mode: "menu" };
    }
    const [first, second] = segs;
    if (first === "local" && segs.length === 1) return { mode: "local" };
    if (first === "p2p" && segs.length === 1) return { mode: "p2p" };
    if (first === "users" && segs.length === 1) return { mode: "users" };
    if (first === "settings" && segs.length === 1) return { mode: "settings" };
    if (first === "watch" && second) return { mode: "watch", gameId: decodeURIComponent(second) };
    if (segs.length === 1) {
      const { kind, size } = kindSizeFromParams(sp);
      return { mode: "user", userId: decodeURIComponent(first), pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc") };
    }
    return { mode: "menu" };
  } catch {
    return { mode: "menu" };
  }
}

/** 邀请链接：`<分享域名>/<host>?pwd=<pwd>&kind=&size=[&rtc=<inviteOffer>]`。
 *  `rtc` 为邀请者预生成的直连 offer（跨设备一键直连用）；同源页面间不需要它，
 *  邀请者建邀请时后台自动生成、生成后自动补进链接，无需用户手动复制 offer。 */
export function inviteToUrl(inviterId: string, pwd: string, kind: GameKind, size: Size, rtcOffer?: string | null): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(inviterId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("kind", kind);
  url.searchParams.set("size", String(size));
  if (rtcOffer) url.searchParams.set("rtc", rtcOffer);
  return url.toString();
}

/** 回执链接：受邀者把自动生成的 answer 编进 URL 发回邀请者，邀请者在等待页点「输入回执」
 *  粘贴即可完成直连（不再依赖邀请者用浏览器打开链接——Tauri 里没有地址栏可粘贴）。
 *  同源页面间 answer 经 Presence 自动回传，此链接仅跨设备时需要复制一次。
 *  game/kind/size 一并编入：邀请者凭回执即可加入受邀者的 game channel 并对齐规则。 */
export function answerToUrl(inviterId: string, pwd: string, rtcAns: string, gameId?: string | null, kind?: GameKind | null, size?: Size | null): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(inviterId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("rtcAns", rtcAns);
  if (gameId) url.searchParams.set("game", gameId);
  if (kind) url.searchParams.set("kind", kind);
  if (size) url.searchParams.set("size", String(size));
  return url.toString();
}

/** 用户主页链接：`<分享域名>/<userId>`（无 pwd，只看到主页、可手动挑战）。 */
export function userToUrl(userId: string): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(userId)}`;
  return url.toString();
}

/** 观战链接：`<分享域名>/watch/<gameId>`。 */
export function watchToUrl(gameId: string): string {
  const url = new URL(shareOrigin());
  url.pathname = `/watch/${encodeURIComponent(gameId)}`;
  return url.toString();
}

/** 回选项页。 */
export function homeUrl(): string {
  return `${window.location.origin}/`;
}

/* ---------------- 分享基地址（自动决定，用户不可见不可设） ----------------
 *
 * 分享类链接（邀请/主页/观战/回执）发给对方后要在浏览器打开，所以基地址必须是
 * 公网可访问的站点。两处自动决定，无需任何用户设置：
 * - Web（http/https）：自动用当前站点 origin；
 * - 非 Web（Tauri 桌面壳等）：自动用云端部署地址 SHARE_ORIGIN_NATIVE。
 * 换部署域名 = 改 SHARE_ORIGIN_NATIVE 一处常量，全局生效。
 */

/** 判断当前是否运行在 Tauri 桌面壳（非 Web）。 */
export function isTauri(): boolean {
  try {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  } catch {
    return false;
  }
}

/** 非 Web 环境分享链接的基地址（云端部署地址）：换域名只改这一处。 */
export const SHARE_ORIGIN_NATIVE = "https://goptop.pages.dev";

/** 分享基地址（自动）：Web 用当前站点；Tauri/非 Web 用 SHARE_ORIGIN_NATIVE。 */
export function shareOrigin(): string {
  return isTauri() ? SHARE_ORIGIN_NATIVE : window.location.origin;
}

/** 从粘贴的任意 URL 中提取站内意图（路径 + 查询参数），与域名无关。
 *  兼容旧 query 风格（`?u=` `?room=` `?watch=`）与路径风格（`/<userId>` `/watch/<id>`）。
 *  返回 null 表示这段文本不是可识别的 GoPtop 链接。 */
export function parsePastedLink(text: string): UrlIntent | null {
  try {
    const raw = text.trim();
    if (!raw) return null;
    // 有协议就当 URL 解析；无协议（如 "goptop.pages.dev/u-1a2b?pwd=xx"）补上再解析
    const withProto = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw) ? raw : `https://${raw}`;
    const url = new URL(withProto);
    const sp = url.searchParams;
    const segs = url.pathname.split("/").filter(Boolean);
    // 旧 query 风格优先（?u= / ?room= / ?watch=）
    const room = sp.get("room");
    if (room) return { mode: "watch", gameId: room };
    const u = sp.get("u");
    if (u) {
      const { kind, size } = kindSizeFromParams(sp);
      return { mode: "user", userId: decodeURIComponent(u), pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc") };
    }
    const watch = sp.get("watch");
    if (watch) return { mode: "watch", gameId: watch };
    // 路径风格
    const [first, second] = segs;
    if (first === "local" && segs.length === 1) return { mode: "local" };
    if (first === "p2p" && segs.length === 1) return { mode: "p2p" };
    if (first === "users" && segs.length === 1) return { mode: "users" };
    if (first === "settings" && segs.length === 1) return { mode: "settings" };
    if (first === "watch" && second) return { mode: "watch", gameId: decodeURIComponent(second) };
    if (segs.length === 1) {
      // 收紧识别：单段路径必须像用户链接（用户 ID 均为 u- 前缀，或带 pwd/rtc 邀请参数），
      // 否则视为普通文本/陌生网址——避免粘贴任意内容被误判成"用户主页"发起挑战。
      if (!first.startsWith("u-") && !sp.has("pwd") && !sp.has("rtc")) return null;
      const { kind, size } = kindSizeFromParams(sp);
      return { mode: "user", userId: decodeURIComponent(first), pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc") };
    }
    return null;
  } catch {
    return null;
  }
}

/** 从粘贴的任意 URL 中提取邀请者回执参数（inviterId + pwd + rtcAns + game/kind/size），与域名无关。 */
export function parsePastedAnswer(text: string): { inviterId: string; pwd: string; rtcAns: string; spectator: boolean; gameId: string | null; kind: GameKind | null; size: Size | null } | null {
  try {
    const raw = text.trim();
    if (!raw) return null;
    const withProto = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw) ? raw : `https://${raw}`;
    const url = new URL(withProto);
    const rtcAns = url.searchParams.get("rtcAns");
    if (!rtcAns) return null;
    const segs = url.pathname.split("/").filter(Boolean);
    if (segs.length !== 1) return null;
    const kindRaw = url.searchParams.get("kind");
    const sizeRaw = Number(url.searchParams.get("size"));
    return {
      inviterId: decodeURIComponent(segs[0]),
      pwd: url.searchParams.get("pwd") ?? "",
      rtcAns,
      // spec=1 标记观战回执（回执类型由链接属性自动判断，用户拍板）；对局回执不带
      spectator: url.searchParams.get("spec") === "1",
      gameId: url.searchParams.get("game"),
      kind: kindRaw === "go" || kindRaw === "gomoku" ? kindRaw : null,
      size: ([9, 13, 15, 19] as number[]).includes(sizeRaw) ? (sizeRaw as Size) : null,
    };
  } catch {
    return null;
  }
}

/** 站内导航（SPA，不刷新）。 */
export function nav(path: string) {
  window.history.pushState(null, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

/* ---------------- 在线发现与挑战（同源 BroadcastChannel，端到端直传） ---------------- */

export type PeerStatus = "idle" | "waiting" | "in-game";
export type PeerInfo = { id: string; name: string; status: PeerStatus; gameId: string | null; ts: number };

export type PresenceEvent =
  | { type: "peers"; peers: PeerInfo[] }
  | { type: "challenge"; from: string; fromName: string; pwd: string | null; kind: GameKind; size: Size; gameId: string; rtcAns: string | null }
  | { type: "accept"; from: string; gameId: string }
  | { type: "reject"; from: string; gameId: string };

type PresenceWire =
  | { t: "announce"; user: Omit<PeerInfo, "ts"> }
  | { t: "challenge"; from: string; fromName: string; to: string; pwd: string | null; kind: GameKind; size: Size; gameId: string; rtcAns?: string | null }
  | { t: "accept"; from: string; to: string; gameId: string }
  | { t: "reject"; from: string; to: string; gameId: string }
  | { t: "bye"; id: string };

const PRESENCE_CH = "goptop-presence-v1";

export class Presence {
  readonly me: string;
  private bc: BroadcastChannel | null = null;
  private peers = new Map<string, PeerInfo>();
  private handlers = new Set<(e: PresenceEvent) => void>();
  private timer: number | null = null;
  private myStatus: PeerStatus = "idle";
  private myGame: string | null = null;
  /** beforeunload 处理器作为实例字段：start 只注册一次、stop 注销，
   *  避免闭包引用旧 bc 且 start→stop→start 循环累积监听器（审查 D6）。 */
  private readonly onUnload = () => {
    try { this.bc?.postMessage({ t: "bye", id: this.me } satisfies PresenceWire); } catch { /* ignore */ }
  };

  constructor() {
    this.me = myUserId();
  }

  start() {
    if (this.bc) return;
    try {
      this.bc = new BroadcastChannel(PRESENCE_CH);
    } catch {
      this.bc = null;
      return;
    }
    this.bc.onmessage = (ev) => this.onWire(ev.data as PresenceWire);
    this.announce();
    this.timer = window.setInterval(() => {
      this.announce();
      this.prune();
    }, 2000);
    window.addEventListener("beforeunload", this.onUnload);
  }

  stop() {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
    try { this.bc?.close(); } catch { /* ignore */ }
    this.bc = null;
    window.removeEventListener("beforeunload", this.onUnload);
  }

  setStatus(status: PeerStatus, gameId: string | null) {
    this.myStatus = status;
    this.myGame = gameId;
    this.announce();
  }

  myName(): string {
    return myName();
  }

  onEvent(h: (e: PresenceEvent) => void): () => void {
    this.handlers.add(h);
    return () => this.handlers.delete(h);
  }

  private emit(e: PresenceEvent) {
    for (const h of this.handlers) {
      try { h(e); } catch { /* isolate */ }
    }
  }

  private announce() {
    if (!this.bc) return;
    const name = myName() || this.me.slice(0, 8);
    try {
      this.bc.postMessage({
        t: "announce",
        user: { id: this.me, name, status: this.myStatus, gameId: this.myGame },
      } satisfies PresenceWire);
    } catch { /* ignore */ }
  }

  /** 向某用户发起挑战（pwd 为空表示手动挑战，需对方点接受；带正确 pwd 则对方自动同意）。
   *  `rtcAns` 为跨设备回执：同源页面间 answer 经此字段自动回传，跨设备时编进回执链接。 */
  challenge(to: string, pwd: string | null, kind: GameKind, size: Size, gameId: string, rtcAns?: string | null) {
    if (!this.bc) return;
    try {
      this.bc.postMessage({
        t: "challenge", from: this.me, fromName: myName() || this.me.slice(0, 8),
        to, pwd, kind, size, gameId, rtcAns: rtcAns ?? null,
      } satisfies PresenceWire);
    } catch { /* ignore */ }
  }

  accept(to: string, gameId: string) {
    try {
      this.bc?.postMessage({ t: "accept", from: this.me, to, gameId } satisfies PresenceWire);
    } catch { /* ignore */ }
  }

  reject(to: string, gameId: string) {
    try {
      this.bc?.postMessage({ t: "reject", from: this.me, to, gameId } satisfies PresenceWire);
    } catch { /* ignore */ }
  }

  private onWire(m: PresenceWire) {
    if (!m || typeof (m as { t: string }).t !== "string") return;
    switch (m.t) {
      case "announce": {
        if (m.user.id === this.me) return;
        this.peers.set(m.user.id, { ...m.user, ts: Date.now() });
        this.emitPeers();
        break;
      }
      case "challenge": {
        if (m.to !== this.me) return;
        this.emit({ type: "challenge", from: m.from, fromName: m.fromName, pwd: m.pwd, kind: m.kind, size: m.size, gameId: m.gameId, rtcAns: m.rtcAns ?? null });
        break;
      }
      case "accept": {
        if (m.to !== this.me) return;
        this.emit({ type: "accept", from: m.from, gameId: m.gameId });
        break;
      }
      case "reject": {
        if (m.to !== this.me) return;
        this.emit({ type: "reject", from: m.from, gameId: m.gameId });
        break;
      }
      case "bye": {
        if (this.peers.delete(m.id)) this.emitPeers();
        break;
      }
    }
  }

  private prune() {
    const now = Date.now();
    let changed = false;
    for (const [id, p] of this.peers) {
      if (now - p.ts > 7000) {
        this.peers.delete(id);
        changed = true;
      }
    }
    if (changed) this.emitPeers();
  }

  private emitPeers() {
    this.emit({ type: "peers", peers: [...this.peers.values()].sort((a, b) => a.id.localeCompare(b.id)) });
  }
}

/* ---------------- 对局数据通道（同源直连 BroadcastChannel） ---------------- */

type GameHandler = (msg: GameMsg) => void;

/**
 * 同 gameId 的直连数据通道。同源页面/观战共用此 channel；
 * 跨设备时由 `DirectRtc` 把收到的远端消息注入同一回调，保证上层只订阅一处。
 */
export class GameChannel {
  gameId: string | null = null;
  readonly myId: string;
  readonly userId: string;
  private seq = 0;
  private bc: BroadcastChannel | null = null;
  private handlers = new Set<GameHandler>();
  /** 每个远端 sender 已应用的最大 seq：BroadcastChannel 与 WebRTC 双链路
   *  会送达同一消息，靠 (sender, seq) 单调去重，保证每条消息只应用一次。 */
  private lastSeq = new Map<string, number>();

  constructor() {
    this.myId =
      typeof crypto !== "undefined" && typeof (crypto as Crypto).randomUUID === "function"
        ? (crypto as Crypto).randomUUID()
        : `peer-${Math.random().toString(36).slice(2, 10)}`;
    this.userId = myUserId();
  }

  join(gameId: string) {
    this.leave();
    this.gameId = gameId;
    try {
      this.bc = new BroadcastChannel(`goptop-game-${gameId}`);
      this.bc.onmessage = (ev) => this.dispatch(ev.data as GameMsg);
    } catch {
      this.bc = null;
    }
  }

  leave() {
    try { this.bc?.close(); } catch { /* ignore */ }
    this.bc = null;
    this.gameId = null;
  }

  send(kind: MsgKind) {
    const msg: GameMsg = { seq: ++this.seq, sender: this.myId, userId: this.userId, kind };
    try { this.bc?.postMessage(msg); } catch { /* ignore */ }
    // 同时经由已建立的 WebRTC 直连广播（跨设备/观战远端）
    DirectRtc.broadcast(msg);
  }

  /** 供 WebRTC 远端消息注入（跨设备直连到达后的统一入口）。 */
  injectRemote(msg: GameMsg) {
    this.dispatch(msg);
  }

  onMessage(h: GameHandler): () => void {
    this.handlers.add(h);
    return () => this.handlers.delete(h);
  }

  private dispatch(msg: GameMsg) {
    if (!msg || typeof msg.seq !== "number" || !msg.kind) return;
    if (msg.sender === this.myId) return;
    // BroadcastChannel 与 WebRTC 双链路会送达同一消息：对会改变对局状态的 Move
    // 按 (sender, seq) 单调去重，先到者应用、后到者丢弃。
    // SyncState/SyncRequest 不去重——它们是幂等全量同步，且重连后发送方 seq 归零，
    // 去重会错误丢弃重连同步。
    if (msg.kind.type === "Move") {
      const seen = this.lastSeq.get(msg.sender) ?? 0;
      if (msg.seq <= seen) return;
      this.lastSeq.set(msg.sender, msg.seq);
    }
    for (const h of this.handlers) {
      try { h(msg); } catch { /* isolate */ }
    }
  }
}

/* ---------------- 跨设备直连（WebRTC DataChannel，STUN-only，禁用 TURN 中转） ---------------- */

/* offer/answer 的 URL 编码：压缩 + pwd 派生 XOR + URL 安全 base64。
 * - 压缩：`CompressionStream("deflate-raw")`，SDP 是高度重复的文本，压缩比可观，
 *   链接显著变短。兼容面：Chromium 103+ / Firefox 113+ / Safari 16.4+ / Tauri
 *   WebView2（Chromium）均可用；**无降级路径**——极旧内核会在 encode/decode 抛错，
 *   上层以「直连建立失败」提示（不误报为线路问题）。
 * - 加密：与本局钥匙 pwd 派生的密钥流逐字节 XOR——链接里不再出现可读 SDP
 *   （SDP 含本机 IP 候选）。这是混淆级而非密码学级（pwd 本就在同一链接里）；
 *   真正的传输安全由 WebRTC 自带的 DTLS 端到端加密保证，此层只为不裸奔。
 * - base64：URL 安全字母表 + 去填充，`%` 编码后无 `%2B` `%2F` `%3D` 转义。 */

const RTC_ENC_MAGIC = "G1"; // 版本头：未来换编码格式时可平滑迁移

/** deflate-raw 压缩。 */
async function deflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const cs = new CompressionStream("deflate-raw");
  const writer = cs.writable.getWriter();
  void writer.write(bytes);
  void writer.close();
  const buf = await new Response(cs.readable).arrayBuffer();
  return new Uint8Array(buf);
}

/** deflate-raw 解压。 */
async function inflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const ds = new DecompressionStream("deflate-raw");
  const writer = ds.writable.getWriter();
  void writer.write(bytes);
  void writer.close();
  const buf = await new Response(ds.readable).arrayBuffer();
  return new Uint8Array(buf);
}

/** 由短钥匙派生重复密钥流：fnv1a 双散列扩展成 4 字节步进，避免同钥周期性。 */
function keyStream(pwd: string, len: number): Uint8Array {
  const enc = new TextEncoder();
  const p = enc.encode(pwd);
  const out = new Uint8Array(len);
  let h1 = 0x811c9dc5, h2 = 0x1b873593;
  for (const b of p) {
    h1 = Math.imul(h1 ^ b, 0x01000193) >>> 0;
    h2 = Math.imul(h2 + b, 0x85ebca6b) >>> 0;
  }
  for (let i = 0; i < len; i++) {
    h1 = (Math.imul(h1, 0x01000193) ^ (h1 >>> 15)) >>> 0;
    h2 = (Math.imul(h2, 0x85ebca6b) ^ (h2 >>> 13)) >>> 0;
    out[i] = (h1 ^ h2) & 0xff;
  }
  return out;
}

/** XOR 加密/解密（同一函数）。 */
function xorBytes(bytes: Uint8Array, pwd: string): Uint8Array {
  const ks = keyStream(pwd, bytes.length);
  const out = new Uint8Array(bytes.length);
  for (let i = 0; i < bytes.length; i++) out[i] = bytes[i] ^ ks[i];
  return out;
}

/** URL 安全 base64（无填充）。 */
function b64urlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** URL 安全 base64 解码。 */
function b64urlDecode(s: string): Uint8Array {
  const t = s.replace(/-/g, "+").replace(/_/g, "/");
  const pad = t.length % 4 === 0 ? "" : "=".repeat(4 - (t.length % 4));
  const bin = atob(t + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** 编码 offer/answer：JSON 短键 → deflate → pwd XOR → URL 安全 base64。
 *  `pwd` 为本局钥匙（编解码两端必须一致）。 */
async function encodeRtcPayload(payload: unknown, pwd: string): Promise<string> {
  const json = JSON.stringify(payload);
  const raw = new TextEncoder().encode(json);
  const deflated = await deflateRaw(raw);
  const encrypted = xorBytes(deflated, pwd);
  return RTC_ENC_MAGIC + b64urlEncode(encrypted);
}

/** 解码 offer/answer（`encodeRtcPayload` 的逆）。 */
async function decodeRtcPayload(token: string, pwd: string): Promise<unknown> {
  const token2 = token.trim();
  if (!token2.startsWith(RTC_ENC_MAGIC)) throw new Error("unknown rtc token");
  const encrypted = b64urlDecode(token2.slice(RTC_ENC_MAGIC.length));
  const deflated = xorBytes(encrypted, pwd);
  const raw = await inflateRaw(deflated);
  return JSON.parse(new TextDecoder().decode(raw));
}

export type RtcState = "idle" | "making-invite" | "waiting-invitee" | "joining" | "open" | "closed" | "error";

/**
 * URL 邀请式点对点直连。约束：
 * - `iceServers` 仅用户启用的 STUN 线路（默认国服 A/B 区；外服默认关闭；可自定义），
 *   只做 NAT 地址发现、不转发数据；**不配置任何 TURN**，杜绝中转。
 * - 信令随邀请走：邀请者 offer 压缩混淆后编进邀请 URL `&rtc=`；受邀者 answer 经同源
 *   Presence 自动回传，跨设备时编进回执链接 `?rtcAns=` 由邀请者弹窗粘贴完成。
 *   编码见 `encodeRtcPayload`（deflate 压缩 + pwd XOR + URL 安全 base64）。
 * - 建连后 DataChannel 标签 `goptop` 直接传 `GameMsg` JSON；观战者同样以独立连接接入邀请者广播。
 */
export class DirectRtcPeer {
  state: RtcState = "idle";
  isInviter: boolean;
  role: "player" | "spectator";
  onRemote: ((msg: GameMsg) => void) | null = null;
  onState: ((s: RtcState) => void) | null = null;
  private pc: RTCPeerConnection | null = null;
  private dc: RTCDataChannel | null = null;
  /** answer 是否已成功应用（setRemoteDescription 成功后才置位）。
   *  回执可能经弹窗与 Presence 双路径到达，本位防重复应用；App 层也读它判断
   *  「失败是否真的失败」（另一路径成功时 catch 不算失败）。 */
  answered = false;
  private static all = new Set<DirectRtcPeer>();

  constructor(opts: { isInviter: boolean; role: "player" | "spectator" }) {
    this.isInviter = opts.isInviter;
    this.role = opts.role;
  }

  private setState(s: RtcState) {
    this.state = s;
    try { this.onState?.(s); } catch { /* ignore */ }
  }

  private makePc(): RTCPeerConnection {
    const pc = new RTCPeerConnection({
      // 仅用户启用的 STUN 线路：解析自身公网地址以便直连；数据不经任何服务器中转。
      iceServers: stunServers(),
    });
    pc.ondatachannel = (ev) => {
      this.attachChannel(ev.channel);
    };
    pc.onconnectionstatechange = () => {
      const st = pc.connectionState;
      if (st === "connected") this.setState("open");
      // disconnected 常可自愈（网络抖动、ICE 切换候选），不判死；真断会转 failed
      else if (st === "failed") this.setState("error");
      else if (st === "closed") this.setState("closed");
    };
    this.pc = pc;
    return pc;
  }

  private attachChannel(ch: RTCDataChannel) {
    this.dc = ch;
    ch.onopen = () => this.setState("open");
    ch.onclose = () => this.setState("closed");
    ch.onerror = () => this.setState("error");
    ch.onmessage = (ev) => {
      try {
        const msg = JSON.parse(String(ev.data)) as GameMsg;
        this.onRemote?.(msg);
      } catch { /* ignore */ }
    };
  }

  private waitGathering(pc: RTCPeerConnection, timeoutMs = 8000): Promise<void> {
    if (pc.iceGatheringState === "complete") return Promise.resolve();
    return new Promise((resolve) => {
      let done = false;
      const finish = () => {
        if (done) return;
        done = true;
        resolve();
      };
      const timer = window.setTimeout(finish, timeoutMs);
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === "complete") {
          window.clearTimeout(timer);
          finish();
        }
      };
      pc.onicecandidate = (ev) => {
        if (!ev.candidate) {
          window.clearTimeout(timer);
          finish();
        }
      };
    });
  }

  /** 邀请者：预生成邀请用 offer（role 固定 player），编进邀请 URL 的 `&rtc=` 参数。
   *  `pwd` 为本局钥匙，同时充当信令编码密钥（两端一致即可解码）。 */
  async createOffer(pwd: string): Promise<string> {
    this.close();
    this.setState("making-invite");
    const pc = this.makePc();
    const ch = pc.createDataChannel("goptop");
    this.attachChannel(ch);
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await this.waitGathering(pc);
    const desc = pc.localDescription!;
    DirectRtcPeer.all.add(this);
    this.setState("waiting-invitee");
    return encodeRtcPayload({ s: desc.sdp, t: desc.type, r: this.role }, pwd);
  }

  /** 受邀者：用邀请 URL 的 offer 生成 answer 返回（随后自动回传邀请者，无需用户操作）。 */
  async acceptOffer(token: string, pwd: string): Promise<string> {
    this.close();
    this.setState("joining");
    const pc = this.makePc();
    const payload = await decodeRtcPayload(token, pwd) as { s: string; t: RTCSdpType; r?: string };
    if (payload.r === "spectator") this.role = "spectator";
    await pc.setRemoteDescription({ type: payload.t, sdp: payload.s });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    await this.waitGathering(pc);
    const desc = pc.localDescription!;
    DirectRtcPeer.all.add(this);
    return encodeRtcPayload({ s: desc.sdp, t: desc.type, r: this.role }, pwd);
  }

  /** 邀请者：用受邀者回传的 answer 完成直连（同源经 Presence 自动回传；跨设备由弹窗粘贴回执触发）。
   *  幂等位在「应用成功」后才置位：坏回执解码失败不锁死，邀请者可再贴正确回执。 */
  async acceptAnswer(token: string, pwd: string): Promise<void> {
    if (!this.pc) throw new Error("no pending offer");
    if (this.answered) return;
    const payload = await decodeRtcPayload(token, pwd) as { s: string; t: RTCSdpType };
    try {
      await this.pc.setRemoteDescription({ type: payload.t, sdp: payload.s });
      this.answered = true;
    } catch (err) {
      // 双路径竞态：另一路径（Presence/弹窗）可能已成功应用同一 answer
      if (this.answered || this.pc.remoteDescription) return;
      throw err;
    }
  }

  send(msg: GameMsg) {
    try {
      if (this.dc && this.dc.readyState === "open") {
        this.dc.send(JSON.stringify(msg));
      }
    } catch { /* ignore */ }
  }

  close() {
    DirectRtcPeer.all.delete(this);
    try { this.dc?.close(); } catch { /* ignore */ }
    try { this.pc?.close(); } catch { /* ignore */ }
    this.dc = null;
    this.pc = null;
    this.answered = false;
    if (this.state !== "idle") this.setState("closed");
  }
}

/** 邀请者侧广播辅助：向所有 open 的直连发送（选手+观战）。
 *  App 启动时经 `wireRtcBroadcast` 注入实现；GameChannel.send 单入口调用。
 *  关键约束：注入的消息与 BroadcastChannel 发出的是同一个对象（同 seq/sender），
 *  两条链路送达同一消息时 GameChannel 按 sender+seq 去重，绝不重复应用。 */
let rtcBroadcast: ((msg: GameMsg) => void) | null = null;

export function wireRtcBroadcast(fn: ((msg: GameMsg) => void) | null) {
  rtcBroadcast = fn;
}

export const DirectRtc = {
  broadcast(msg: GameMsg) {
    rtcBroadcast?.(msg);
  },
};

/** 单例：对局数据通道（App 与各面板共用）。 */
export const transport = new GameChannel();

/** 单例：在线发现与挑战。 */
export const presence = new Presence();
