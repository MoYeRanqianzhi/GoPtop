/**
 * net/serverChannel — 信令服务器客户端（可选模式）。
 *
 * 与 Rust 服务器（crates/goptop-server）的 WebSocket 协议对接。服务器是纯转发管道：
 * - hello→welcome（服务器分配 s- 短 ID）+ 25s 心跳（低于 CF 代理 100s 空闲阈值）
 * - 在线名册（peers 全量广播）
 * - `signal`：任意点对点信令原样转发（服务器不解析不存储）——邀请 offer/answer、
 *   观战 join/offer/answer、大厅挑战、观战房间控制全走这里，语义由 useGameSession 解释
 * - `relay`：数据兜底中转（P2P 未建立时对局消息走这里，与直连双发按 sender+seq 去重）
 *
 * 断线自动重连（指数退避封顶 10s）；重连成功后自动重放当前名册状态。
 */
import type { GameMsg } from "./protocol";
import { myUserId } from "./identity";
import { BUILTIN_SERVERS, loadServerSelection, loadServers } from "./servers";

export type ServerState = "off" | "connecting" | "ready" | "error";

export type ServerUserInfo = { id: string; name: string; status: "idle" | "waiting" | "in-game"; gameId: string | null };

export type ServerEvent =
  | { t: "state"; s: ServerState; detail?: string }
  | { t: "welcome"; id: string }
  | { t: "peers"; users: ServerUserInfo[] }
  | { t: "signal"; from: string; kind: string; payload: Record<string, unknown> }
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
      // 注册名册 ID：与邀请链接的 userId 一致（对端凭 URL 找人），重连同 ID 顶替旧连接。
      // 注意服务器字段是 camelCase（rename_all_fields）。
      ws.send(JSON.stringify({ t: "hello", name: this.displayName(), userId: myUserId() }));
      // 心跳：25s（CF 代理 WebSocket 空闲上限 100s，必须低于它）
      this.pingTimer = window.setInterval(() => {
        try { ws.send(JSON.stringify({ t: "ping" })); } catch { /* ignore */ }
      }, PING_MS);
    };
    ws.onmessage = (ev) => {
      if (import.meta.env.DEV) console.log("[server<<]", String(ev.data).slice(0, 160));
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

  /** 通用点对点信令：kind 标注语义，payload 原样转发（服务器不解析不存储）。 */
  signal(to: string, kind: string, payload: Record<string, unknown>) {
    if (import.meta.env.DEV) console.log("[signal>>out]", to, kind);
    this.send({ t: "signal", to, kind, payload });
  }

  /** 数据兜底中转：把一条 GameMsg 转给指定对端。 */
  relay(to: string, msg: GameMsg) {
    this.send({ t: "relay", to, payload: msg });
  }
}

/** 单例：服务器信令通道。 */
export const serverChannel = new ServerChannel();
