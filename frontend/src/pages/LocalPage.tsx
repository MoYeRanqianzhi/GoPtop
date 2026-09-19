/**
 * 本地对战页（D1 拆分）——与 P2P 同一套棋盘 UI，只是没有邀请链接。
 * 落子/悔棋判定走 Rust 规则引擎（wasm，game/rules.ts），与 P2P 同一真源。
 * 规则/尺寸由顶部选择器统一控制。
 */
import { useEffect, useRef, useState } from "react";
import type { Coord, StoneColor } from "../net/protocol";
import { emptyBoard } from "../game/board";
import { RulesEngine } from "../game/rules";
import { BoardPanel } from "./components";
import type { GameKind, Size } from "../net/protocol";

export function LocalPage(props: { kind: GameKind; size: Size }) {
  const { kind, size } = props;
  const rulesRef = useRef(new RulesEngine());
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(size));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);

  // 顶部切换规则/尺寸时重建 Rust 引擎并重置棋盘
  useEffect(() => {
    rulesRef.current.newGame(kind, size);
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [kind, size]);

  function resetBoard() {
    rulesRef.current.reset();
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
    // 规则判定与棋盘更新都在 Rust（wasm）：权威棋盘直接上屏（围棋提子生效）
    const res = rulesRef.current.place(c.x, c.y);
    if (!res?.ok) return;
    setBoard(res.board);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (res.winner) setWinner(res.winner);
    else setToMove(res.toMove);
  }

  function undo() {
    if (history.length === 0 || winner) return;
    // Rust 侧弹出手并重放（围棋提子一并还原）
    const res = rulesRef.current.undo();
    if (!res?.ok) return;
    setBoard(res.board);
    setHistory((h) => h.slice(0, -1));
    setLastMove(history.length >= 2 ? history[history.length - 2] : null);
    setToMove(res.toMove);
    setWinner(res.winner);
  }

  const statusText = winner ? `${winner === "black" ? "黑" : "白"} 胜` : `${toMove === "black" ? "黑" : "白"} 落子`;

  return (
    <div className="play-stack">
      <BoardPanel
        kind={kind} size={size} board={board} toMove={toMove} winner={winner}
        lastMove={lastMove} hover={hover} onHover={setHover}
        disabled={!!winner} onPlace={place}
        statusText={statusText} statusNote=""
        moveCount={history.length}
        onUndo={history.length > 0 && !winner ? undo : null} onReset={resetBoard}
      />
    </div>
  );
}
