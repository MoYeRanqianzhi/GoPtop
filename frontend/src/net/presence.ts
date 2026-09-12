/**
 * net/presence — 在线发现与挑战（同源 BroadcastChannel，端到端直传）。
 *
 * BroadcastChannel 为同源端到端直传（浏览器内共享内存，不经过任何服务器/中转）；
 * 同源页面间的 answer 回执也经此通道自动回传。跨设备走 WebRTC 直连 + 回执链接，
 * 服务器模式走服务器信令（relay 兜底中转仅用于对局数据，默认直连优先）。
 */
import { myName, myUserId } from "./identity";
import type { GameKind, Size } from "./protocol";

export type PeerStatus = "idle" | "waiting" | "in-game";
export type PeerInfo = { id: string; name: string; status: PeerStatus; gameId: string | null; ts: number };

export type PresenceEvent =
  | { type: "peers"; peers: PeerInfo[] }
  | { type: "challenge"; from: string; fromName: string; pwd: string | null; kind: GameKind; size: Size; gameId: string; rtcAns: string | null }
  | { type: "accept"; from: string; gameId: string }
  | { type: "reject"; from: string; gameId: string };

export type PresenceWire =
  | { t: "announce"; user: Omit<PeerInfo, "ts"> }
  | { t: "challenge"; from: string; fromName: string; to: string; pwd: string | null; kind: GameKind; size: Size; gameId: string; rtcAns?: string | null }
  | { t: "accept"; from: string; to: string; gameId: string }
  | { t: "reject"; from: string; to: string; gameId: string }
  | { t: "bye"; id: string };

export const PRESENCE_CH = "goptop-presence-v1";

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

/** 单例：在线发现与挑战。 */
export const presence = new Presence();
