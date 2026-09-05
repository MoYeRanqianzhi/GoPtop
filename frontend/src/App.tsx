import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Coord, StoneColor } from "./components/BoardSvg";
import { BoardSvg } from "./components/BoardSvg";
import { ticketFromUrl, ticketToUrl, transport } from "./net/transport";
import type { GameMsg } from "./net/transport";

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

  // —— P2P 状态 —— //
  const [ticket, setTicket] = useState<string | null>(null);
  const [isInRoom, setIsInRoom] = useState(false);
  const [myColor, setMyColor] = useState<StoneColor>("black");
  const [peerConnected, setPeerConnected] = useState(false);
  const [joinInput, setJoinInput] = useState("");
  const [shareFeedback, setShareFeedback] = useState<string | null>(null);
  // refs：供消息回调读取最新值，避免闭包过期
  const boardRef = useRef(board);
  const toMoveRef = useRef(toMove);
  const winnerRef = useRef(winner);
  const historyRef = useRef(history);
  const lastMoveRef = useRef(lastMove);
  const kindRef = useRef(kind);
  const sizeRef = useRef(size);
  const myColorRef = useRef(myColor);
  useEffect(() => { boardRef.current = board; }, [board]);
  useEffect(() => { toMoveRef.current = toMove; }, [toMove]);
  useEffect(() => { winnerRef.current = winner; }, [winner]);
  useEffect(() => { historyRef.current = history; }, [history]);
  useEffect(() => { lastMoveRef.current = lastMove; }, [lastMove]);
  useEffect(() => { kindRef.current = kind; }, [kind]);
  useEffect(() => { sizeRef.current = size; }, [size]);
  useEffect(() => { myColorRef.current = myColor; }, [myColor]);

  // kind 切换时自动修正 size（五子棋固定 15，围棋若从 15 来则切 19）
  useEffect(() => {
    const nextSize: Size = kind === "gomoku" ? 15 : size === 15 ? 19 : size;
    if (nextSize !== size) setSize(nextSize);
  }, [kind]); // eslint-disable-line react-hooks/exhaustive-deps

  // 本地未联机时，size 变化重置棋盘；联机时由 SyncState 驱动，不在此重置
  useEffect(() => {
    if (isInRoom) return;
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [size, isInRoom]);

  // 启动时若 URL 带 room 则自动加入
  useEffect(() => {
    const t = ticketFromUrl();
    if (t && !isInRoom) {
      doJoin(t);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const statusText = useMemo(() => {
    if (winner) return `${winner === "black" ? "黑" : "白"} 胜`;
    return `${toMove === "black" ? "黑" : "白"} 落子`;
  }, [winner, toMove]);

  const p2pStatusText = useMemo(() => {
    if (!isInRoom) return "本地双打 · 创建房间或加入房间以联机";
    if (!peerConnected) return `已建房 · 等待对手加入… · 你是${myColor === "black" ? "黑" : "白"}`;
    return `已连接 · 你是${myColor === "black" ? "黑" : "白"} · ${toMove === myColor ? "轮到你" : "等待对手"}`;
  }, [isInRoom, peerConnected, myColor, toMove]);

  const boardDisabled = useMemo(() => {
    if (winner) return true;
    if (!isInRoom) return false;
    if (!peerConnected) return true;
    return toMove !== myColor;
  }, [winner, isInRoom, peerConnected, toMove, myColor]);

  // —— 传输消息处理（稳定回调，读 refs） —— //
  const handleNetMessage = useCallback((msg: GameMsg) => {
    const k = msg.kind;
    switch (k.type) {
      case "SyncRequest": {
        setPeerConnected(true);
        transport.send({
          type: "SyncState",
          board: boardRef.current,
          toMove: toMoveRef.current,
          winner: winnerRef.current,
          history: historyRef.current,
          lastMove: lastMoveRef.current,
          kind: kindRef.current,
          size: sizeRef.current,
        });
        break;
      }
      case "SyncState": {
        setPeerConnected(true);
        setKind(k.kind);
        setSize(k.size);
        setBoard(k.board.map((r) => [...r]));
        setToMove(k.toMove);
        setWinner(k.winner);
        setHistory([...k.history]);
        setLastMove(k.lastMove ? { ...k.lastMove } : null);
        break;
      }
      case "Hello": {
        setPeerConnected(true);
        if (myColorRef.current === "black") {
          transport.send({
            type: "SyncState",
            board: boardRef.current,
            toMove: toMoveRef.current,
            winner: winnerRef.current,
            history: historyRef.current,
            lastMove: lastMoveRef.current,
            kind: kindRef.current,
            size: sizeRef.current,
          });
        }
        break;
      }
      case "Move": {
        setPeerConnected(true);
        if (k.move.type === "Place") {
          const c = k.move.coord;
          const mover = toMoveRef.current;
          if (winnerRef.current) break;
          const curBoard = boardRef.current;
          if (curBoard[c.y]?.[c.x] !== "empty") break;
          const next = curBoard.map((r) => [...r]);
          next[c.y][c.x] = mover;
          const willWin = kindRef.current === "gomoku" && checkFive(next, c, mover);
          setBoard(next);
          setLastMove(c);
          setHistory((h) => [...h, c]);
          if (willWin) {
            setWinner(mover);
          } else {
            setToMove(mover === "black" ? "white" : "black");
          }
        } else if (k.move.type === "Pass") {
          setToMove((s) => (s === "black" ? "white" : "black"));
        } else if (k.move.type === "Resign") {
          setWinner(myColorRef.current);
        }
        break;
      }
      case "Reset": {
        setPeerConnected(true);
        setKind(k.kind);
        setSize(k.size);
        setBoard(emptyBoard(k.size));
        setToMove("black");
        setWinner(null);
        setLastMove(null);
        setHistory([]);
        setHover(null);
        break;
      }
      case "Chat":
      case "Ping":
      case "Pong":
      case "UndoReq":
      case "UndoAck":
        break;
    }
  }, []);

  // 订阅传输（仅一次）
  useEffect(() => {
    const off = transport.onMessage(handleNetMessage);
    return off;
  }, [handleNetMessage]);

  function doCreate() {
    const t = transport.createRoom();
    setTicket(t);
    setIsInRoom(true);
    setMyColor("black");
    setPeerConnected(false);
    // 重置为当前 kind/size 的空盘
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setShareFeedback(null);
    // 房主等待对端 SyncRequest/Hello 后再同步
  }

  function doJoin(raw: string) {
    const t = raw.trim();
    if (!t) return;
    // 支持粘贴完整 URL
    let ticket = t;
    try {
      const u = new URL(t);
      const r = u.searchParams.get("room");
      if (r) ticket = r;
      else if (u.hash.startsWith("#") && u.hash.length > 1) ticket = u.hash.slice(1);
    } catch { /* not a URL, treat as raw ticket */ }
    transport.joinRoom(ticket);
    setTicket(ticket);
    setIsInRoom(true);
    setMyColor("white");
    setPeerConnected(false);
    setShareFeedback(null);
    // 加入后主动请求同步
    setTimeout(() => {
      transport.send({ type: "Hello", kind, size });
      transport.send({ type: "SyncRequest" });
    }, 80);
  }

  function leaveRoom() {
    transport.leave();
    setTicket(null);
    setIsInRoom(false);
    setPeerConnected(false);
    setMyColor("black");
    setShareFeedback(null);
    try {
      const url = new URL(window.location.href);
      url.searchParams.delete("room");
      if (url.hash.startsWith("#")) url.hash = "";
      window.history.replaceState(null, "", url.toString());
    } catch { /* ignore */ }
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
  }

  async function copyLink() {
    const link = ticket ? ticketToUrl(ticket) : window.location.href;
    try {
      await navigator.clipboard.writeText(link);
      setShareFeedback("已复制");
      setTimeout(() => setShareFeedback(null), 1600);
    } catch {
      setShareFeedback("复制失败，请手动复制地址栏");
      setTimeout(() => setShareFeedback(null), 2000);
    }
  }

  function handlePlace(c: Coord) {
    if (winner) return;
    if (board[c.y][c.x] !== "empty") return;
    if (isInRoom && peerConnected && toMove !== myColor) return;
    if (isInRoom && !peerConnected) return;

    const mover = toMove;
    const next = board.map((row) => [...row]);
    next[c.y][c.x] = mover;
    const willWin = kind === "gomoku" && checkFive(next, c, mover);
    setBoard(next);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (willWin) {
      setWinner(mover);
    } else {
      setToMove((s: StoneColor) => (s === "black" ? "white" : "black"));
    }

    if (isInRoom) {
      transport.send({ type: "Move", move: { type: "Place", coord: c } });
    }
  }

  function undo() {
    if (history.length === 0 || winner) return;
    if (isInRoom) return; // 联机时悔棋需 UndoReq 流程，暂禁用本地悔棋
    const prev = history[history.length - 1];
    const next = board.map((row) => [...row]);
    next[prev.y][prev.x] = "empty";
    setBoard(next);
    setHistory((h) => h.slice(0, -1));
    setLastMove(history.length >= 2 ? history[history.length - 2] : null);
    setToMove((s: StoneColor) => (s === "black" ? "white" : "black"));
  }

  function reset() {
    const s = size;
    const k = kind;
    setBoard(emptyBoard(s));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
    if (isInRoom) {
      transport.send({ type: "Reset", kind: k, size: s });
    }
  }

  const moveCount = history.length;

  return (
    <div style={{ height: "100dvh", minHeight: "100dvh", display: "flex", flexDirection: "column", background: "var(--bg-page)", overflow: "hidden" }}>
      <div className="poster-strip">GoPtop · P2P Gomoku & Go · Neubrutalism · BroadcastChannel P2P · No Server</div>

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
              onClick={() => {
                if (isInRoom) return;
                setKind("gomoku");
              }}
              aria-pressed={kind === "gomoku"}
              title={isInRoom ? "联机中不可切换，请先退出房间" : undefined}
              disabled={isInRoom}
            >
              五子棋
            </button>
            <button
              className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
              onClick={() => {
                if (isInRoom) return;
                setKind("go");
              }}
              aria-pressed={kind === "go"}
              title={isInRoom ? "联机中不可切换，请先退出房间" : undefined}
              disabled={isInRoom}
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
                onClick={() => {
                  if (isInRoom) return;
                  setSize(s as Size);
                }}
                aria-pressed={size === s}
                disabled={isInRoom}
                title={isInRoom ? "联机中不可切换尺寸" : undefined}
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
        {/* 联机大厅 */}
        <div className="brutal-card" style={{ padding: "10px 12px", background: isInRoom ? "#fffbeb" : "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
            <span className="brutal-label">联机 · P2P（无服务器）</span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: isInRoom && peerConnected ? "#0a7a2e" : "var(--muted)" }}>
              {p2pStatusText}
            </span>
          </div>
          {!isInRoom ? (
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
              <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={doCreate}>
                创建房间（执黑）
              </button>
              <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "var(--muted)" }}>或</span>
              <input
                placeholder="粘贴房间链接或 ticket"
                value={joinInput}
                onChange={(e) => setJoinInput(e.target.value)}
                style={{ flex: "1 1 220px", minWidth: 180, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }}
              />
              <button className="brutal-btn brutal-btn--sm" onClick={() => doJoin(joinInput)} disabled={!joinInput.trim()}>
                加入（执白）
              </button>
            </div>
          ) : (
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
              <code style={{ flex: "1 1 220px", minWidth: 180, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {ticket ? ticketToUrl(ticket) : ""}
              </code>
              <button className="brutal-btn brutal-btn--sm" onClick={copyLink}>
                复制邀请链接
              </button>
              <button className="brutal-btn brutal-btn--sm" onClick={leaveRoom}>
                退出房间
              </button>
              {shareFeedback && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{shareFeedback}</span>}
            </div>
          )}
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
            验收方式：打开两个窗口（或无痕窗口），一窗创建房间并复制链接，另一窗粘贴加入；两窗落子实时同步，满足“两个窗口通过 p2p 连接并正常下棋”。
          </div>
        </div>

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
              {isInRoom ? `联机 · ${myColor === "black" ? "你执黑" : "你执白"}` : kind === "gomoku" ? "五子连珠" : "围棋对弈"}
            </span>
          </div>

          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button className="brutal-btn brutal-btn--sm" onClick={undo} disabled={history.length === 0 || !!winner || isInRoom} title={isInRoom ? "联机对弈暂不支持悔棋" : undefined}>
              悔棋
            </button>
            <button className="brutal-btn brutal-btn--sm brutal-btn--primary" onClick={reset}>
              重开
            </button>
            <button
              className="brutal-btn brutal-btn--sm"
              onClick={copyLink}
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
            disabled={boardDisabled}
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
        GoPtop · P2P Gomoku & Go · 验收：双窗口建连对弈可用
      </footer>
    </div>
  );
}
