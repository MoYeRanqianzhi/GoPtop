/**
 * net/protocol — 对局线格式类型唯一真源。
 *
 * 所有链路（同源 BroadcastChannel、WebRTC DataChannel、服务器 relay 兜底中转）
 * 传的都是这里定义的 JSON 结构；BoardSvg 与 game/board 的 StoneColor/Coord 也统一
 * 从这里取，避免结构相同却各自漂移。
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
  | { type: "Hello"; kind: GameKind; size: Size }
  // by：发送端声明的执子颜色。接收端（尤其观战者，没有"我的颜色"可用）据此判定，
  // 不得从本地 toMove/myColor 推断——历史 bug：任何一方认输，观战者都判白胜。
  | { type: "Move"; move: Move; by: StoneColor }
  // sv：回退纪元（可选）。悔棋/重开让 history 变短是合法回退，旧守卫只比
  // history.length 会把回退快照当旧快照丢掉（观战者永远看不到回退）；
  // 发送端在每次本地回退时 +1 并随快照广播，接收端按 (sv, history.length)
  // 双键比较——sv 更旧或同 sv 但更短才丢弃。
  | { type: "SyncState"; sv?: number; board: StoneColor[][]; toMove: StoneColor; winner: StoneColor | null; history: Coord[]; lastMove: Coord | null; kind: GameKind; size: Size }
  | { type: "SyncRequest" }
  | { type: "Chat"; text: string }
  | { type: "Ping" }
  | { type: "Pong" }
  | { type: "Reset"; kind: GameKind; size: Size }
  // —— 协商类：请求 → 对方弹窗 → Ack；同意后双方各自执行确定性操作，
  //    并补发 SyncState 让观战者对齐（协商消息观战者只忽略不应用）——
  | { type: "UndoReq" }
  | { type: "UndoAck"; ok: boolean }
  | { type: "ResetReq" }
  | { type: "ResetAck"; ok: boolean }
  | { type: "SwapReq" }
  | { type: "SwapAck"; ok: boolean }
  // 头像：圆形裁剪后的 dataURL（≤128px jpeg，几 KB），走 P2P 不经服务器
  | { type: "Avatar"; dataUrl: string };

export type GameMsg = {
  seq: number;
  sender: string;
  /** 发送者的 userId，用于区分选手/观战者。 */
  userId: string;
  kind: MsgKind;
};
