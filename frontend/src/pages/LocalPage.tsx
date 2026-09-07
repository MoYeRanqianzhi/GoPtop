/**
 * 本地对战页（D1 拆分）——与 P2P 同一套棋盘 UI，只是没有邀请链接。
 * 从 App.tsx 原样搬出，禁止行为变化。规则/尺寸由顶部选择器统一控制。
 */
import { useEffect, useState } from "react";
import type { Coord, StoneColor } from "../components/BoardSvg";
import { checkFive, emptyBoard } from "../game/board";
import { BoardPanel } from "./components";
import type { GameKind, Size } from "../net/transport";

export function LocalPage(props: { kind: GameKind; size: Size }) {
  const { kind, size } = props;
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(size));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);

  // 顶部切换规则/尺寸时重置棋盘
  useEffect(() => {
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [kind, size]);

  function resetBoard() {
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }

  function place(c: Coord) {
    if (winner) return;
    if (board[c.y][c.x] !== "empty") return;
    const next = board.map((row) => [...row]);
    next[c.y][c.x] = toMove;
    const willWin = kind === "gomoku" && checkFive(next, c, toMove);
    setBoard(next);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (willWin) setWinner(toMove);
    else setToMove(toMove === "black" ? "white" : "black");
  }

  function undo() {
    if (history.length === 0 || winner) return;
    const prev = history[history.length - 1];
    const next = board.map((row) => [...row]);
    next[prev.y][prev.x] = "empty";
    setBoard(next);
    setHistory((h) => h.slice(0, -1));
    setLastMove(history.length >= 2 ? history[history.length - 2] : null);
    setToMove((s) => (s === "black" ? "white" : "black"));
  }

  const statusText = winner ? `${winner === "black" ? "黑" : "白"} 胜` : `${toMove === "black" ? "黑" : "白"} 落子`;

  return (
    <div className="play-stack">
      <BoardPanel
        kind={kind} size={size} board={board} toMove={toMove} winner={winner}
        lastMove={lastMove} hover={hover} onHover={setHover}
        disabled={!!winner} onPlace={place}
        statusText={statusText} statusNote={kind === "gomoku" ? "五子连珠 · 本地" : "围棋对弈 · 本地"}
        moveCount={history.length} history={history}
        onUndo={history.length > 0 && !winner ? undo : null} onReset={resetBoard}
      />
    </div>
  );
}
