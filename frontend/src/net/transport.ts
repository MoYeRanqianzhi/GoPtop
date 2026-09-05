/**
 * net/transport — 前端 P2P 传输门面（BroadcastChannel 实现，真 P2P，无服务器）。
 *
 * 设计：
 * - 同源双窗口/双标签通过 `BroadcastChannel("goptop-<ticket>")` 直连，符合"两个窗口通过 p2p 连接"验收。
 * - `RoomTicket` 即 channel 名后缀，通过 URL `?room=<ticket>` 分享，复制链接即可让对端 `join`。
 * - `GameMsg` 与 Rust `protocol::GameMsg` 语义对齐（seq 单调、MsgKind 枚举），便于未来替换为 iroh 而无需改上层。
 * - Tauri 下仍可复用此门面（Tauri 窗口同为同源 WebView，BroadcastChannel 亦可用）；未来 iroh 就绪时仅替换本文件实现。
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
  kind: MsgKind;
};

type Handler = (msg: GameMsg) => void;

function genTicket(): string {
  const a = Date.now().toString(36);
  const b = Math.random().toString(36).slice(2, 8);
  const c = Math.random().toString(36).slice(2, 6);
  return `${a}${b}${c}`;
}

export function ticketToUrl(ticket: string): string {
  const url = new URL(window.location.href);
  url.searchParams.set("room", ticket);
  return url.toString();
}

export function ticketFromUrl(): string | null {
  const url = new URL(window.location.href);
  return url.searchParams.get("room") || (window.location.hash.startsWith("#") ? window.location.hash.slice(1) : null);
}

export class BroadcastTransport {
  ticket: string | null = null;
  isConnected = false;
  readonly myId: string;
  private seq = 0;
  private bc: BroadcastChannel | null = null;
  private handlers = new Set<Handler>();

  constructor() {
    this.myId =
      typeof crypto !== "undefined" && typeof (crypto as Crypto).randomUUID === "function"
        ? (crypto as Crypto).randomUUID()
        : `peer-${Math.random().toString(36).slice(2, 10)}`;
  }

  /** 创建房间并返回 ticket（本端为黑）。 */
  createRoom(): string {
    this.leave();
    const t = genTicket();
    this.ticket = t;
    this.bc = new BroadcastChannel(`goptop-${t}`);
    this.bc.onmessage = (ev) => this.dispatch(ev.data as GameMsg);
    this.isConnected = true;
    // 将 ticket 写入 URL，便于刷新后仍在房间
    try {
      const url = new URL(window.location.href);
      url.searchParams.set("room", t);
      window.history.replaceState(null, "", url.toString());
    } catch { /* ignore */ }
    return t;
  }

  /** 加入房间（本端为白，加入后应发送 Hello/SyncRequest）。 */
  joinRoom(ticket: string) {
    this.leave();
    this.ticket = ticket;
    this.bc = new BroadcastChannel(`goptop-${ticket}`);
    this.bc.onmessage = (ev) => this.dispatch(ev.data as GameMsg);
    this.isConnected = true;
    try {
      const url = new URL(window.location.href);
      url.searchParams.set("room", ticket);
      window.history.replaceState(null, "", url.toString());
    } catch { /* ignore */ }
  }

  leave() {
    if (this.bc) {
      try { this.bc.close(); } catch { /* ignore */ }
      this.bc = null;
    }
    this.isConnected = false;
    this.ticket = null;
    // 不清空 URL，由调用方决定是否保留
  }

  send(kind: MsgKind) {
    if (!this.bc || !this.isConnected) return;
    const msg: GameMsg = { seq: ++this.seq, sender: this.myId, kind };
    // BroadcastChannel 不会回送给发送者，无需过滤 fromSelf
    this.bc.postMessage(msg);
  }

  onMessage(handler: Handler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  private dispatch(msg: GameMsg) {
    if (!msg || typeof msg.seq !== "number" || !msg.kind) return;
    if (msg.sender === this.myId) return;
    for (const h of this.handlers) {
      try { h(msg); } catch { /* isolate */ }
    }
  }
}

/** 单例，供 App/gameStore 复用，保证同页多处订阅同一通道。 */
export const transport = new BroadcastTransport();
