/**
 * negotiation —— 协商：悔棋 / 重开 / 换棋（对方同意制）。
 *
 * 从 useGameSession 原样整体搬出（禁止行为变化）：函数体逐字保留，只把原本
 * 引用 hook 内 state/refs/函数的地方改为工厂顶部从 ctx 一次解构。
 * 「发起方发请求 → 对方弹窗 → UndoAck/ResetAck/SwapAck」的弹窗续体挂在
 * useGameSession 的 handleNetMessage 里，本地执行统一走本模块。
 */
import { emptyBoard } from "../game/board";
import { transport } from "../net/gameChannel";
import type { Coord } from "../net/protocol";
import type { SessionCtx } from "./sessionContext";

export function createNegotiation(ctx: Pick<
  SessionCtx,
  | "boardRef" | "historyRef" | "kindRef" | "lastMoveRef" | "myColorRef" | "phaseRef" | "roleRef"
  | "sizeRef" | "toMoveRef" | "winnerRef" | "syncEpochRef" | "rulesRef"
  | "setBoard" | "setHistory" | "setHover" | "setLastMove" | "setMyColor" | "setToMove" | "setWinner"
  | "showNotice"
>) {
  const {
    boardRef, historyRef, kindRef, lastMoveRef, myColorRef, phaseRef, roleRef,
    sizeRef, toMoveRef, winnerRef, syncEpochRef, rulesRef,
    setBoard, setHistory, setHover, setLastMove, setMyColor, setToMove, setWinner,
    showNotice,
  } = ctx;

  /* ---------- 协商：悔棋 / 重开 / 换棋（对方同意制） ---------- */

  /** 撤销最后一手：Rust 侧弹出手并重放（围棋提子一并还原），TS 只做
   *  history/lastMove 的镜像裁剪；完成后推进纪元并补发 SyncState 对齐观战者。 */
  function applyUndoLocal() {
    const h = historyRef.current;
    if (h.length === 0) return;
    const res = rulesRef.current.undo();
    if (!res?.ok) return;
    syncEpochRef.current += 1;
    setBoard(res.board);
    const rest = h.slice(0, -1);
    setHistory(rest);
    // lastMove 语义是最后一颗落子：截断后末项可能是 "pass"，向前找最近坐标
    let last: Coord | null = null;
    for (let i = rest.length - 1; i >= 0; i--) {
      const m = rest[i];
      if (m !== "pass") { last = m; break; }
    }
    setLastMove(last);
    setWinner(res.winner);
    setToMove(res.toMove);
    setTimeout(() => pushSyncState(), 60);
  }

  function requestUndo() {
    if (phaseRef.current !== "playing" || historyRef.current.length === 0) return;
    transport.send({ type: "UndoReq" });
    showNotice("已请求悔棋，等待对方同意…", 6000);
  }

  function requestReset() {
    if (phaseRef.current !== "playing") return;
    transport.send({ type: "ResetReq" });
    showNotice("已请求重开，等待对方同意…", 6000);
  }

  function requestSwap() {
    if (phaseRef.current !== "playing") return;
    transport.send({ type: "SwapReq" });
    showNotice("已请求换棋（交换黑白并重开），等待对方同意…", 6000);
  }

  function pushSyncState() {
    transport.send({
      type: "SyncState", sv: syncEpochRef.current,
      board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
      history: historyRef.current, lastMove: lastMoveRef.current,
      kind: kindRef.current, size: sizeRef.current,
    });
  }

  /** 协商重开的本地执行（双方各自执行 + SyncState 对齐观战者）。 */
  function doResetLocal() {
    rulesRef.current.reset();
    setBoard(emptyBoard(sizeRef.current));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
    // 回退推进纪元：理由同 applyUndoLocal（审计 B1）
    syncEpochRef.current += 1;
    setTimeout(() => pushSyncState(), 60);
  }

  /** 协商换棋的本地执行：黑白互换并重开（观战者无需变色，颜色以消息 by 为准）。 */
  function doSwapLocal() {
    if (roleRef.current === "inviter" || roleRef.current === "invitee") {
      const flipped = myColorRef.current === "black" ? "white" : "black";
      setMyColor(flipped);
      myColorRef.current = flipped;
    }
    doResetLocal();
  }

  return { applyUndoLocal, requestUndo, requestReset, requestSwap, doResetLocal, doSwapLocal };
}
