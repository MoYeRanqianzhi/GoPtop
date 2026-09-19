/**
 * net/protocol — TS 侧类型定义（Coord/StoneColor/GameKind/Size 等 UI 共用类型在此统一定义）。
 *
 * 线格式（GameMsg/MsgKind）的真源是 crates/goptop-net/src/protocol.rs：所有链路
 * （同源 BroadcastChannel、WebRTC DataChannel、服务器 relay 兜底中转）传的 JSON 以那边为准。
 * 本文件 MsgKind 已滞后（缺 ScoreMark/ScoreConfirmReq/ScoreConfirmAck，见 protocol.rs:86-89），只作阅读参考。
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
  // history 可表达停一手（"pass"）：含 Pass 的对局若快照丢失该信息，
  // adopt 后 undo 重放的行棋方必然漂移（审查 #5 P1-2）。
  | { type: "SyncState"; sv?: number; board: StoneColor[][]; toMove: StoneColor; winner: StoneColor | null; history: (Coord | "pass")[]; lastMove: Coord | null; kind: GameKind; size: Size }
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
  // 头像：圆形裁剪后的 dataURL（128px PNG，几 KB）；服务器模式下会经服务器 relay 转发给观战者（matchplay.rs:41-51）
  | { type: "Avatar"; dataUrl: string };

export type GameMsg = {
  seq: number;
  sender: string;
  /** 发送者的 userId，用于区分选手/观战者。 */
  userId: string;
  kind: MsgKind;
};
