import { useEffect, useMemo, useState } from "react";
import type { Coord, StoneColor } from "./components/BoardSvg";
import { BoardSvg } from "./components/BoardSvg";

type GameKind = "gomoku" | "go";
type Size = 9 | 13 | 15 | 19;

const DIRS: [number, number][] = [[1, 0], [0, 1], [1, 1], [1, -1]];

function checkFive(board: StoneColor[][], last: Coord, color: StoneColor): boolean {
  if (color === "empty") return false;
  const n = board.length;
  for (const [dx, dy] of DIRS) {
    let count = 1;
    for (const dir of [1, -1]) {
      let x = last.x + dx * dir;
      let y = last.y + dy * dir;
      while (x >= 0 && y >= 0 && x < n && y < n && board[y][x] === color) {
        count++;
        x += dx * dir;
        y += dy * dir;
      }
    }
    if (count >= 5) return true;
  }
  return false;
}

function emptyBoard(size: number): StoneColor[][] {
  return Array.from({ length: size }, () => Array.from({ length: size }, () => "empty" as StoneColor));
}

export default function App() {
  const [kind, setKind] = useState<GameKind>("gomoku");
  const [size, setSize] = useState<Size>(15);
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(15));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);

  useEffect(() => {
    const nextSize: Size = kind === "gomoku" ? 15 : size === 15 ? 19 : size;
    if (nextSize !== size) setSize(nextSize);
  }, [kind]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [size]);

  const statusText = useMemo(() => {
    if (winner) return `${winner === "black" ? "黑" : "白"} 胜`;
    return `${toMove === "black" ? "黑" : "白"} 落子`;
  }, [winner, toMove]);

  function handlePlace(c: Coord) {
    if (winner) return;
    if (board[c.y][c.x] !== "empty") return;
    const next = board.map((row) => [...row]);
    next[c.y][c.x] = toMove;
    if (kind === "gomoku" && checkFive(next, c, toMove)) {
      setBoard(next);
      setLastMove(c);
      setHistory((h) => [...h, c]);
      setWinner(toMove);
      return;
    }
    setBoard(next);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    setToMove((s: StoneColor) => (s === "black" ? "white" : "black"));
  }

  function undo() {
    if (history.length === 0 || winner) return;
    const prev = history[history.length - 1];
    const next = board.map((row) => [...row]);
    next[prev.y][prev.x] = "empty";
    setBoard(next);
    setHistory((h) => h.slice(0, -1));
    setLastMove(history.length >= 2 ? history[history.length - 2] : null);
    setToMove((s: StoneColor) => (s === "black" ? "white" : "black"));
  }

  function reset() {
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }

  const moveCount = history.length;

  return (
    <div style={{ height: "100dvh", minHeight: "100dvh", display: "flex", flexDirection: "column", background: "var(--bg-page)", overflow: "hidden" }}>
      <div className="poster-strip">GoPtop · P2P Gomoku & Go · Neubrutalism · Official Relay · No Server</div>

      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 12,
          padding: "14px 16px",
          borderBottom: "4px solid var(--ink)",
          background: "#fff",
          flexWrap: "wrap",
        }}
      >
        <div style={{ display: "flex", alignItems: "baseline", gap: 10, flexWrap: "wrap" }}>
          <div className="brutal-title" style={{ fontSize: 30, lineHeight: 1 }}>
            GoPtop
          </div>
          <div
            style={{
              fontFamily: "var(--font-mono)",
              fontWeight: 700,
              fontSize: 11,
              letterSpacing: "0.08em",
              textTransform: "uppercase",
              border: "2px solid var(--ink)",
              padding: "3px 8px",
              background: "#fff",
            }}
          >
            {kind === "gomoku" ? "Gomoku" : "Go"} · {size}×{size}
          </div>
        </div>

        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              className={`brutal-btn brutal-btn--sm ${kind === "gomoku" ? "brutal-btn--active" : ""}`}
              onClick={() => setKind("gomoku")}
              aria-pressed={kind === "gomoku"}
            >
              五子棋
            </button>
            <button
              className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
              onClick={() => setKind("go")}
              aria-pressed={kind === "go"}
            >
              围棋
            </button>
          </div>
          <div style={{ width: 1, height: 26, background: "var(--ink)", opacity: 0.18 }} />
          <div style={{ display: "flex", gap: 6 }}>
            {(kind === "gomoku" ? [15] : [9, 13, 19]).map((s) => (
              <button
                key={s}
                className={`brutal-btn brutal-btn--sm ${size === s ? "brutal-btn--active" : ""}`}
                onClick={() => setSize(s as Size)}
                aria-pressed={size === s}
              >
                {s}×{s}
              </button>
            ))}
          </div>
        </div>
      </header>

      <main
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "flex-start",
          padding: "clamp(6px, 1.2vh, 12px) 12px clamp(6px, 1vh, 10px)",
          width: "100%",
          maxWidth: 760,
          margin: "0 auto",
          overflow: "hidden",
        }}
      >
        <div className="play-stack">
        <div
          className="brutal-card"
          style={{
            flexShrink: 0,
            width: "100%",
            maxWidth: "100%",
            padding: "12px 14px",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 12,
            flexWrap: "wrap",
            background: winner ? "var(--bg-2)" : "#fff",
            color: winner ? "#fff" : "var(--ink)",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
            <span
              style={{
                width: 16,
                height: 16,
                borderRadius: 999,
                background: winner ? (winner === "black" ? "#0A0A0A" : "#fff") : toMove === "black" ? "#0A0A0A" : "#fff",
                border: winner === "white" ? "2px solid var(--ink)" : winner ? "2px solid #fff" : "2px solid var(--ink)",
                display: "inline-block",
                flexShrink: 0,
              }}
            />
            <span className="brutal-title" style={{ fontSize: 18, letterSpacing: "0.04em" }}>
              {statusText}
            </span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, opacity: 0.9 }}>
              手数 {moveCount}
            </span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase", opacity: 0.6 }}>
              {kind === "gomoku" ? "五子连珠" : "围棋对弈"}
            </span>
          </div>

          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button className="brutal-btn brutal-btn--sm" onClick={undo} disabled={history.length === 0 || !!winner}>
              悔棋
            </button>
            <button className="brutal-btn brutal-btn--sm brutal-btn--primary" onClick={reset}>
              重开
            </button>
            <button
              className="brutal-btn brutal-btn--sm"
              onClick={() => navigator.clipboard?.writeText(window.location.href).catch(() => {})}
            >
              复制链接
            </button>
          </div>
        </div>

        <div className="board-wrap">
          <BoardSvg
            size={size}
            board={board}
            onPlace={handlePlace}
            lastMove={lastMove}
            hover={hover}
            onHover={setHover}
            disabled={!!winner}
            kind={kind}
          />
        </div>

        <div
          style={{
            flexShrink: 0,
            width: "100%",
            maxWidth: "100%",
            display: "grid",
            gridTemplateColumns: "1fr 1fr",
            gap: 12,
          }}
          className="bottom-grid"
        >
          <div className="brutal-card brutal-card--paper" style={{ padding: 14 }}>
            <div className="brutal-label">规则</div>
            <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 13, lineHeight: 1.6, fontWeight: 600 }}>
              {kind === "gomoku" ? (
                <>
                  <li>黑先，轮流落子，先连五者胜。</li>
                  <li>落子于交点，已有棋子处不可落子。</li>
                  <li>胜负以最后一手的四方向连珠判定。</li>
                </>
              ) : (
                <>
                  <li>黑先，轮流落子于交点。</li>
                  <li>提子、气与自杀判定由规则引擎执行。</li>
                  <li>同点不可重复落子，终局计目待后续版本。</li>
                </>
              )}
            </ul>
          </div>

          <div className="brutal-card" style={{ padding: 14, background: "#fff" }}>
            <div className="brutal-label">对局</div>
            <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 6, fontSize: 13, lineHeight: 1.6, fontWeight: 600 }}>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
                <span style={{ color: "var(--muted)" }}>手数</span>
                <span style={{ fontFamily: "var(--font-mono)" }}>{moveCount}</span>
              </div>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
                <span style={{ color: "var(--muted)" }}>轮到</span>
                <span style={{ fontFamily: "var(--font-mono)" }}>{winner ? "—" : toMove === "black" ? "黑" : "白"}</span>
              </div>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
                <span style={{ color: "var(--muted)" }}>棋盘</span>
                <span style={{ fontFamily: "var(--font-mono)" }}>{size}×{size}</span>
              </div>
              {winner && (
                <div style={{ marginTop: 4, padding: "6px 8px", border: "2px solid var(--ink)", background: "var(--bg-2)", color: "#fff", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, textAlign: "center" }}>
                  {winner === "black" ? "黑" : "白"} 胜 — 点击重开开始新对局
                </div>
              )}
            </div>
          </div>
        </div>
        </div>
      </main>

      <style>{`
        @media (max-width: 640px) {
          .bottom-grid { grid-template-columns: 1fr !important; }
        }
      `}</style>

      <footer
        style={{
          flexShrink: 0,
          padding: "10px 16px",
          borderTop: "3px solid var(--ink)",
          background: "#fff",
          fontFamily: "var(--font-mono)",
          fontSize: 11,
          fontWeight: 700,
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          color: "var(--muted)",
          textAlign: "center",
        }}
      >
        GoPtop · P2P Gomoku & Go
      </footer>
    </div>
  );
}
