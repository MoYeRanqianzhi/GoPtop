/**
 * net/gameChannel — 对局数据通道（同源直连 BroadcastChannel）。
 *
 * 同 gameId 的页面/观战者共用此 channel：观战者只收 SyncState，不可落子；
 * 同源观战走同 game channel。跨设备由 net/rtc 的 `DirectRtc` 把收到的远端消息
 * 注入同一回调；服务器模式 P2P 未建立时另有 relay 兜底中转——多条链路送达同一
 * 消息时按 (sender, seq) 单调去重（见 dispatch）。
 */
import type { GameMsg, MsgKind } from "./protocol";
import { myUserId } from "./identity";
import { DirectRtc } from "./rtc";

export type { GameMsg };

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
    // BroadcastChannel / WebRTC 直连 / 服务器 relay 三链路会送达同一条消息：
    // 有副作用的类型（落子、协商请求与批复、头像）按 (sender, seq) 单调去重，
    // 先到者应用、后到者丢弃——否则 SwapAck 这类「翻转」操作会被执行两次翻回去。
    // SyncState/SyncRequest 不去重（幂等全量同步，重连后发送方 seq 归零，
    // 去重会错误丢弃重连同步）；Chat 允许重复（仅显示层，且须容忍 seq 归零）。
    const DEDUP = new Set(["Move", "UndoReq", "UndoAck", "ResetReq", "ResetAck", "SwapReq", "SwapAck", "Avatar"]);
    if (DEDUP.has(msg.kind.type)) {
      const seen = this.lastSeq.get(msg.sender) ?? 0;
      if (msg.seq <= seen) return;
      this.lastSeq.set(msg.sender, msg.seq);
    }
    for (const h of this.handlers) {
      try { h(msg); } catch { /* isolate */ }
    }
  }
}

/** 单例：对局数据通道（App 与各面板共用）。 */
export const transport = new GameChannel();
