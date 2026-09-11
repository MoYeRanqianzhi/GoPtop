/**
 * net/serverChannel — 信令服务器客户端（可选模式）。
 *
 * 与 Rust 服务器（crates/goptop-server）的 WebSocket 协议对接：
 * - 注册（hello→welcome，服务器分配 s- 短 ID）+ 25s 心跳（低于 CF 代理 100s 空闲阈值）
 * - 在线名册（peers 全量广播）
 * - 短码邀请/观战（invite-create/resolve、watch-create/resolve）
 * - SDP 与 trickle ICE 转发（offer/answer/ice）
 * - 数据兜底中转（relay）——P2P 未建立时对局消息走这里，与直连双发按 (sender,seq) 去重
 *
 * 断线自动重连（指数退避封顶 10s）；重连成功后自动重放当前名册状态。
 * 不做消息持久化：服务器内存态、连接断即清（与服务器端约定一致）。
 */
import type { GameKind, GameMsg, Size } from "./transport";
import { BUILTIN_SERVERS, loadServerSelection, loadServers } from "./transport";

export type ServerState = "off" | "connecting" | "ready" | "error";

export type ServerUserInfo = { id: string; name: string; status: "idle" | "waiting" | "in-game"; gameId: string | null };

export type ServerEvent =
  | { t: "state"; s: ServerState; detail?: string }
  | { t: "welcome"; id: string }
  | { t: "peers"; users: ServerUserInfo[] }
  | { t: "invite-created"; code: string }
  | { t: "invite-offer"; code: string; kind: GameKind; size: Size; offer: string; gameId: string; from: string; fromName: string }
  | { t: "invitee-joined"; code: string; from: string }
  | { t: "answer"; from: string; gameId: string; answer: string }
  | { t: "offer"; from: string; gameId: string; role: string; offer: string }
  | { t: "ice"; from: string; gameId: string; candidate: string }
  | { t: "watch-created"; code: string }
  | { t: "watch-accepted"; code: string; from: string; gameId: string }
  | { t: "spectator-joined"; code: string; from: string }
  | { t: "challenge"; from: string; kind: GameKind; size: Size; code: string; pwd: string }
  | { t: "challenge-rejected"; from: string }
  | { t: "relayed"; from: string; payload: GameMsg }
  | { t: "error"; msg: string };

const PING_MS = 25_000;

export class ServerChannel {
  state: ServerState = "off";
  /** 服务器分配的短 ID（welcome 后可用）。 */
  myServerId: string | null = null;
  onEvent: ((e: ServerEvent) => void) | null = null;

  private ws: WebSocket | null = null;
  private url: string | null = null;
  /** 名册状态重放：断线重连后自动重新上报。 */
  private lastStatus: { status: "idle" | "waiting" | "in-game"; gameId: string | null } = { status: "idle", gameId: null };
  private retry = 0;
  private timer: number | null = null;
  private pingTimer: number | null = null;
  private manualClose = false;

  /** 按设置选中项连接；无服务器（none）则保持 off 并断开现有连接。 */
  connectFromSettings() {
    const sel = loadServerSelection();
    if (sel === "none") {
      this.close();
      return;
    }
    const url = [...BUILTIN_SERVERS, ...loadServers()].find((s) => s.id === sel)?.url ?? null;
    if (!url) {
      this.setState("error", "选中的服务器配置不存在");
      return;
    }
    this.connect(url);
  }

  connect(url: string) {
    if (this.ws && this.url === url && (this.state === "ready" || this.state === "connecting")) return;
    this.close();
    this.url = url;
    this.manualClose = false;
    this.setState("connecting");
    this.open();
  }

  private open() {
    if (!this.url) return;
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.url);
    } catch (e) {
      this.setState("error", String(e));
      this.scheduleRetry();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      ws.send(JSON.stringify({ t: "hello", name: this.displayName() }));
      // 心跳：25s（CF 代理 WebSocket 空闲上限 100s，必须低于它）
      this.pingTimer = window.setInterval(() => {
        try { ws.send(JSON.stringify({ t: "ping" })); } catch { /* ignore */ }
      }, PING_MS);
    };
    ws.onmessage = (ev) => {
      let m: ServerEvent | { t: string } | null = null;
      try { m = JSON.parse(String(ev.data)); } catch { return; }
      if (!m || typeof m.t !== "string") return;
      if (m.t === "welcome") {
        this.retry = 0;
        this.myServerId = (m as { id: string }).id;
        this.setState("ready");
        // 重连恢复：重新上报名册状态
        this.announce(this.lastStatus.status, this.lastStatus.gameId);
      } else if (m.t === "pong") {
        // 心跳回应，无需处理
      } else {
        this.onEvent?.(m as ServerEvent);
      }
    };
    ws.onclose = () => {
      this.teardownTimers();
      if (this.manualClose) {
        this.setState("off");
        return;
      }
      this.setState("connecting");
      this.scheduleRetry();
    };
    ws.onerror = () => { /* onclose 会跟着来，状态在 onclose 统一推进 */ };
  }

  private displayName(): string {
    try { return localStorage.getItem("goptop:name") || ""; } catch { return ""; }
  }

  private scheduleRetry() {
    if (this.manualClose || !this.url) return;
    // 指数退避 1s→2s→4s→…封顶 10s
    const delay = Math.min(1000 * 2 ** this.retry, 10_000);
    this.retry += 1;
    this.timer = window.setTimeout(() => this.open(), delay);
  }

  private teardownTimers() {
    if (this.pingTimer !== null) { window.clearInterval(this.pingTimer); this.pingTimer = null; }
    if (this.timer !== null) { window.clearTimeout(this.timer); this.timer = null; }
  }

  private setState(s: ServerState, detail?: string) {
    this.state = s;
    this.onEvent?.({ t: "state", s, detail });
  }

  close() {
    this.manualClose = true;
    this.teardownTimers();
    try { this.ws?.close(); } catch { /* ignore */ }
    this.ws = null;
    this.url = null;
    this.myServerId = null;
    this.retry = 0;
    if (this.state !== "off") this.setState("off");
  }

  get connected(): boolean {
    return this.state === "ready" && this.ws?.readyState === WebSocket.OPEN;
  }

  private send(o: unknown): boolean {
    if (!this.connected) return false;
    try {
      this.ws!.send(JSON.stringify(o));
      return true;
    } catch {
      return false;
    }
  }

  /** 名册状态上报（对局状态变化与重连恢复共用）。 */
  announce(status: "idle" | "waiting" | "in-game", gameId: string | null) {
    this.lastStatus = { status, gameId };
    this.send({ t: "announce", status, gameId });
  }

  /** 建短码邀请：kind/size/pwd/offer 存服务器换短码。 */
  inviteCreate(kind: GameKind, size: Size, pwd: string, offer: string, gameId: string) {
    this.send({ t: "invite-create", kind, size, pwd, offer, gameId });
  }

  inviteResolve(code: string, pwd: string) {
    this.send({ t: "invite-resolve", code, pwd });
  }

  answer(to: string, gameId: string, answer: string) {
    this.send({ t: "answer", to, gameId, answer });
  }

  offer(to: string, gameId: string, role: "player" | "spectator", offer: string) {
    this.send({ t: "offer", to, gameId, role, offer });
  }

  ice(to: string, gameId: string, candidate: string) {
    this.send({ t: "ice", to, gameId, candidate });
  }

  watchCreate(gameId: string) {
    this.send({ t: "watch-create", gameId });
  }

  watchResolve(code: string) {
    this.send({ t: "watch-resolve", code });
  }

  /** 大厅挑战：先建短码邀请，再把 code/pwd 随挑战信发给对方（对方同意后 resolve）。 */
  challenge(to: string, kind: GameKind, size: Size, code: string, pwd: string) {
    this.send({ t: "challenge", to, kind, size, code, pwd });
  }

  challengeReject(to: string) {
    this.send({ t: "challenge-reject", to });
  }

  /** 数据兜底中转：把一条 GameMsg 转给指定对端。 */
  relay(to: string, msg: GameMsg) {
    this.send({ t: "relay", to, payload: msg });
  }
}

/** 单例：服务器信令通道。 */
export const serverChannel = new ServerChannel();
