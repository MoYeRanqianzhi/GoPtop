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
import type { SessionCtx } from "./sessionContext";

export function createNegotiation(ctx: Pick<
  SessionCtx,
  | "boardRef" | "historyRef" | "kindRef" | "lastMoveRef" | "myColorRef" | "phaseRef" | "roleRef"
  | "sizeRef" | "toMoveRef" | "winnerRef" | "syncEpochRef"
  | "setBoard" | "setHistory" | "setHover" | "setLastMove" | "setMyColor" | "setToMove" | "setWinner"
  | "showNotice"
>) {
  const {
    boardRef, historyRef, kindRef, lastMoveRef, myColorRef, phaseRef, roleRef,
    sizeRef, toMoveRef, winnerRef, syncEpochRef,
    setBoard, setHistory, setHover, setLastMove, setMyColor, setToMove, setWinner,
    showNotice,
  } = ctx;

  /* ---------- 协商：悔棋 / 重开 / 换棋（对方同意制） ---------- */

  /** 撤销最后一手（确定性操作，双方一致）；完成后补发 SyncState 对齐观战者。 */
  function applyUndoLocal() {
    const h = historyRef.current;
    if (h.length === 0) return;
    const last = h[h.length - 1];
    const next = boardRef.current.map((r) => [...r]);
    next[last.y][last.x] = "empty";
    setBoard(next);
    setHistory(h.slice(0, -1));
    setLastMove(h.length >= 2 ? h[h.length - 2] : null);
    setWinner(null);
    setToMove(h.length % 2 === 1 ? "black" : "white");
    // 回退推进纪元：否则补发的更短快照会被接收端守卫当旧快照丢弃（审计 B1）
    syncEpochRef.current += 1;
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
