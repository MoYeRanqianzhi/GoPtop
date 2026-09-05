/**
 * net/transport — 用户对用户 P2P 直连层（无服务器、无中转）。
 *
 * 模型（按需求）：
 * - 每个标签页即一个用户：`userId` 存 `sessionStorage`（每页不同），昵称可改。
 * - 选项页（`/`）：本地对战 / P2P 对战 / 在线用户 / 设置四个入口。
 * - 用户主页（`/<userId>`）；邀请链接（`/<hostId>?pwd=<pwd>`）：对端带 pwd 打开即视为
 *   "带钥匙的连接请求"，主机校验 pwd 正确则自动同意，无需手动点接受。
 * - 无 pwd 访问某用户主页则只看到其主页，可手动发起挑战，对方弹窗同意后才进对局。
 * - pwd 只在"已开战但无对手（waiting）"时有效；两人进对局后 pwd 即失效，不存在第三人凭旧 pwd 加入。
 *   建连成功后邀请组件隐藏，转而显示观战链接（`/watch/<game>`）。
 * - 观战：观战者只收 `SyncState`，不可落子；同源观战走同 game channel，
 *   跨设备观战走与主机之间的独立 WebRTC 直连。
 * - 跨设备信令：用户只传一次邀请链接。offer/answer 编码进邀请 URL 的 `&rtc=` 参数
 *   （同源页面间经 Presence 自动回传），全程无手动复制粘贴面板。
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
  | { type: "Move"; move: Move }
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

/** 每局轮换的一次性 pwd（邀请钥匙）。 */
export function genPwd(): string {
  return Math.random().toString(36).slice(2, 8);
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

/** 房主打开客人回执链接时解析其中的 answer（`?rtcAns=`），供自动完成直连。 */
export function parseAnswerFromUrl(): string | null {
  try {
    const url = new URL(window.location.href);
    const segs = url.pathname.split("/").filter(Boolean);
    if (segs.length !== 1) return null;
    return url.searchParams.get("rtcAns");
  } catch {
    return null;
  }
}

/** 邀请链接：`/<host>?pwd=<pwd>&kind=&size=[&rtc=<hostOffer>]`。
 *  `rtc` 为主机预生成的直连 offer（跨设备一键直连用）；同源页面间不需要它，
 *  主机建邀请时后台自动生成、生成后自动补进链接，无需用户手动复制 offer。 */
export function inviteToUrl(hostId: string, pwd: string, kind: GameKind, size: Size, rtcOffer?: string | null): string {
  const url = new URL(window.location.origin);
  url.pathname = `/${encodeURIComponent(hostId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("kind", kind);
  url.searchParams.set("size", String(size));
  if (rtcOffer) url.searchParams.set("rtc", rtcOffer);
  return url.toString();
}

/** 回执链接：客人把自动生成的 answer 编进 URL 发回房主，房主打开此链接即完成直连。
 *  同源页面间 answer 经 Presence 自动回传，此链接仅跨设备时需要复制一次。 */
export function answerToUrl(hostId: string, pwd: string, rtcAns: string): string {
  const url = new URL(window.location.origin);
  url.pathname = `/${encodeURIComponent(hostId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("rtcAns", rtcAns);
  return url.toString();
}

/** 用户主页链接：`/<userId>`（无 pwd，只看到主页、可手动挑战）。 */
export function userToUrl(userId: string): string {
  const url = new URL(window.location.origin);
  url.pathname = `/${encodeURIComponent(userId)}`;
  return url.toString();
}

/** 观战链接：`/watch/<gameId>`。 */
export function watchToUrl(gameId: string): string {
  const url = new URL(window.location.origin);
  url.pathname = `/watch/${encodeURIComponent(gameId)}`;
  return url.toString();
}

/** 回选项页。 */
export function homeUrl(): string {
  return `${window.location.origin}/`;
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
    window.addEventListener("beforeunload", () => {
      try { this.bc?.postMessage({ t: "bye", id: this.me } satisfies PresenceWire); } catch { /* ignore */ }
    });
  }

  stop() {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
    try { this.bc?.close(); } catch { /* ignore */ }
    this.bc = null;
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
    for (const h of this.handlers) {
      try { h(msg); } catch { /* isolate */ }
    }
  }
}

/* ---------------- 跨设备直连（WebRTC DataChannel，STUN-only，禁用 TURN 中转） ---------------- */

export type RtcState = "idle" | "making-invite" | "waiting-guest" | "joining" | "open" | "closed" | "error";

/**
 * URL 邀请式点对点直连。约束：
 * - `iceServers` 仅用户启用的 STUN 线路（默认国服 A/B 区；外服默认关闭；可自定义），
 *   只做 NAT 地址发现、不转发数据；**不配置任何 TURN**，杜绝中转。
 * - 信令随邀请走：主机 offer 编进邀请 URL `&rtc=`；客人 answer 经同源 Presence 自动回传，
 *   跨设备时编进回执链接 `?rtcAns=` 由房主打开完成。用户全程只传链接，不碰 offer/answer 文本。
 * - 建连后 DataChannel 标签 `goptop` 直接传 `GameMsg` JSON；观战者同样以独立连接接入主机广播。
 */
export class DirectRtcPeer {
  state: RtcState = "idle";
  isHost: boolean;
  role: "player" | "spectator";
  onRemote: ((msg: GameMsg) => void) | null = null;
  onState: ((s: RtcState) => void) | null = null;
  private pc: RTCPeerConnection | null = null;
  private dc: RTCDataChannel | null = null;
  private static all = new Set<DirectRtcPeer>();

  constructor(opts: { isHost: boolean; role: "player" | "spectator" }) {
    this.isHost = opts.isHost;
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
      else if (st === "failed" || st === "disconnected") this.setState("error");
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

  /** 主机：预生成邀请用 offer（role 固定 player），编进邀请 URL 的 `&rtc=` 参数。 */
  async createOffer(): Promise<string> {
    this.close();
    this.setState("making-invite");
    const pc = this.makePc();
    const ch = pc.createDataChannel("goptop");
    this.attachChannel(ch);
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await this.waitGathering(pc);
    const payload = { sdp: pc.localDescription, role: this.role };
    DirectRtcPeer.all.add(this);
    this.setState("waiting-guest");
    return btoa(unescape(encodeURIComponent(JSON.stringify(payload))));
  }

  /** 客人：用邀请 URL 的 offer 生成 answer 返回（随后自动回传房主，无需用户操作）。 */
  async acceptOffer(offerB64: string): Promise<string> {
    this.close();
    this.setState("joining");
    const pc = this.makePc();
    const payload = JSON.parse(decodeURIComponent(escape(atob(offerB64.trim())))) as { sdp: RTCSessionDescriptionInit; role?: string };
    if (payload.role === "spectator") this.role = "spectator";
    await pc.setRemoteDescription(payload.sdp);
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    await this.waitGathering(pc);
    const out = { sdp: pc.localDescription, role: this.role };
    DirectRtcPeer.all.add(this);
    return btoa(unescape(encodeURIComponent(JSON.stringify(out))));
  }

  /** 房主：用客人回传的 answer 完成直连（同源自动调用；跨设备由回执链接自动触发）。 */
  async acceptAnswer(answerB64: string): Promise<void> {
    if (!this.pc) throw new Error("no pending offer");
    const payload = JSON.parse(decodeURIComponent(escape(atob(answerB64.trim())))) as { sdp: RTCSessionDescriptionInit };
    await this.pc.setRemoteDescription(payload.sdp);
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
    if (this.state !== "idle") this.setState("closed");
  }
}

/** 主机侧广播辅助：向所有 open 的直连发送（选手+观战）。 */
export const DirectRtc = {
  broadcast(msg: GameMsg) {
    // 占位：实际广播由 App 持有 peer 列表完成；此处保留门面以便 gameChannel.send 单入口。
    void msg;
  },
};

/** 单例：对局数据通道（App 与各面板共用）。 */
export const transport = new GameChannel();

/** 单例：在线发现与挑战。 */
export const presence = new Presence();
