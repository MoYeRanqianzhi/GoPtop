import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { Coord, StoneColor } from "./components/BoardSvg";
import { BoardSvg } from "./components/BoardSvg";
import {
  answerToUrl,
  DirectRtcPeer,
  genGameId,
  genPwd,
  inviteToUrl,
  loadStunLines,
  myName,
  myUserId,
  nav,
  parseAnswerFromUrl,
  parseUrl,
  presence,
  saveStunLines,
  setMyName,
  transport,
  userToUrl,
  watchToUrl,
} from "./net/transport";
import type { GameKind, GameMsg, PeerInfo, Size, StunLine, UrlIntent } from "./net/transport";

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

/** 设置页持久化的默认规则/尺寸。 */
function loadDefaults(): { kind: GameKind; size: Size } {
  try {
    const raw = localStorage.getItem("goptop:defaults");
    if (raw) {
      const d = JSON.parse(raw) as { kind?: unknown; size?: unknown };
      const kind: GameKind = d.kind === "go" ? "go" : "gomoku";
      const size: Size = ([9, 13, 15, 19] as number[]).includes(Number(d.size)) ? (Number(d.size) as Size) : (kind === "go" ? 19 : 15);
      return { kind, size };
    }
  } catch { /* ignore */ }
  return { kind: "gomoku", size: 15 };
}

type Role = "idle" | "host" | "guest" | "spectator";
type Phase = "home" | "waiting" | "playing";

/* ---------------- 通用小组件 ---------------- */

function PeerList(props: {
  peers: PeerInfo[];
  emptyHint: string;
  actionLabel?: (p: PeerInfo) => string;
  onAction?: (p: PeerInfo) => void;
  extraAction?: (p: PeerInfo) => ReactNode;
}) {
  const { peers, emptyHint, actionLabel, onAction, extraAction } = props;
  if (peers.length === 0) {
    return (
      <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 600, color: "var(--muted)" }}>
        {emptyHint}
      </div>
    );
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      {peers.map((p) => (
        <div key={p.id} style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", border: "3px solid var(--ink)", padding: "6px 8px", background: "#fffbeb" }}>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>{p.name}</span>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>
            {p.status === "idle" ? "空闲" : p.status === "waiting" ? "等待对手中" : "对局中"}
          </span>
          <span style={{ flex: 1 }} />
          {extraAction?.(p)}
          {onAction && actionLabel && (
            <button className="brutal-btn brutal-btn--sm" onClick={() => onAction(p)}
              disabled={p.status === "in-game"} title={p.status === "in-game" ? "对方对局中，不可挑战" : "向其发起对局"}>
              {actionLabel(p)}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

function StunSettings() {
  const [lines, setLines] = useState<StunLine[]>(() => loadStunLines());
  const [customUrl, setCustomUrl] = useState("");
  const [customName, setCustomName] = useState("");

  function persist(next: StunLine[]) {
    setLines(next);
    saveStunLines(next);
  }

  function toggle(id: string) {
    persist(lines.map((l) => (l.id === id ? { ...l, enabled: !l.enabled } : l)));
  }

  function remove(id: string) {
    persist(lines.filter((l) => l.id !== id));
  }

  function addCustom() {
    const urls = customUrl.trim();
    if (!urls) return;
    const id = `custom-${Date.now().toString(36)}`;
    persist([...lines, { id, label: customName.trim() || "自定义", urls, builtin: false, enabled: true }]);
    setCustomUrl("");
    setCustomName("");
  }

  return (
    <div>
      <div className="brutal-label" style={{ marginBottom: 6 }}>直连线路（STUN，仅地址发现、不转发数据）</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {lines.map((l) => (
          <div key={l.id} style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", border: "3px solid var(--ink)", padding: "6px 8px", background: l.enabled ? "#fffbeb" : "#fff" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>{l.label}</span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", overflowWrap: "anywhere" }}>{l.urls}</span>
            <span style={{ flex: 1 }} />
            {!l.builtin && (
              <button className="brutal-btn brutal-btn--sm" onClick={() => remove(l.id)}>删除</button>
            )}
            <button className={`brutal-btn brutal-btn--sm ${l.enabled ? "brutal-btn--active" : ""}`}
              onClick={() => toggle(l.id)} aria-pressed={l.enabled}>
              {l.enabled ? "启用中" : "已关闭"}
            </button>
          </div>
        ))}
      </div>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginTop: 8 }}>
        <input placeholder="名称（可选，如 公司STUN）" value={customName} onChange={(e) => setCustomName(e.target.value)}
          style={{ flex: "1 1 120px", minWidth: 120, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
        <input placeholder="stun:host:port" value={customUrl} onChange={(e) => setCustomUrl(e.target.value)}
          style={{ flex: "2 1 200px", minWidth: 180, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
        <button className="brutal-btn brutal-btn--sm" onClick={addCustom} disabled={!customUrl.trim()}>添加线路</button>
      </div>
      <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5, marginTop: 6 }}>
        默认国服A/B区开启、外服关闭。至少保留一条启用线路；全关则仅局域网 host 候选直连。本局邀请链接在开局时按当前线路生成。
      </div>
    </div>
  );
}

function RtcStatusLine(props: { directState: string }) {
  const { directState } = props;
  const text = directState === "open" ? "直连已建立"
    : directState === "making-invite" ? "正在准备直连邀请…"
    : directState === "waiting-guest" ? "邀请已就绪 · 等待对方打开邀请链接"
    : directState === "joining" ? "正在通过邀请链接直连…"
    : directState === "error" ? "直连失败（可检查设置页线路）"
    : directState === "closed" ? "直连已关闭" : "同源直传中";
  return (
    <div className="brutal-card" style={{ padding: "8px 10px", background: "#fff", display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <span className="brutal-label">P2P 直连</span>
      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: directState === "open" ? "#0a7a2e" : directState === "error" ? "#b00020" : "var(--muted)" }}>{text}</span>
    </div>
  );
}

function BoardPanel(props: {
  kind: GameKind; size: Size;
  board: StoneColor[][]; toMove: StoneColor; winner: StoneColor | null;
  lastMove: Coord | null; hover: Coord | null; onHover: (c: Coord | null) => void;
  disabled: boolean; onPlace: (c: Coord) => void;
  statusText: string; statusNote: string; moveCount: number; history: Coord[];
  onUndo: (() => void) | null; onReset: () => void;
  actions?: ReactNode;
}) {
  const { kind, size, board, toMove, winner, lastMove, hover, onHover, disabled, onPlace } = props;
  return (
    <>
      <div className="brutal-card" style={{ flexShrink: 0, width: "100%", maxWidth: "100%", padding: "12px 14px", display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, flexWrap: "wrap", background: winner ? "var(--bg-2)" : "#fff", color: winner ? "#fff" : "var(--ink)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <span style={{ width: 16, height: 16, borderRadius: 999, background: winner ? (winner === "black" ? "#0A0A0A" : "#fff") : toMove === "black" ? "#0A0A0A" : "#fff", border: winner === "white" ? "2px solid var(--ink)" : winner ? "2px solid #fff" : "2px solid var(--ink)", display: "inline-block", flexShrink: 0 }} />
          <span className="brutal-title" style={{ fontSize: 18, letterSpacing: "0.04em" }}>{props.statusText}</span>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, opacity: 0.9 }}>手数 {props.moveCount}</span>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase", opacity: 0.6 }}>
            {props.statusNote}
          </span>
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          {props.onUndo && <button className="brutal-btn brutal-btn--sm" onClick={props.onUndo}>悔棋</button>}
          <button className="brutal-btn brutal-btn--sm brutal-btn--primary" onClick={props.onReset}>重开</button>
          {props.actions}
        </div>
      </div>

      <div className="board-wrap">
        <BoardSvg size={size} board={board} onPlace={onPlace} lastMove={lastMove} hover={hover} onHover={onHover} disabled={disabled} kind={kind} />
      </div>

      <div style={{ flexShrink: 0, width: "100%", maxWidth: "100%", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }} className="bottom-grid">
        <div className="brutal-card brutal-card--paper" style={{ padding: 14 }}>
          <div className="brutal-label">规则</div>
          <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 13, lineHeight: 1.6, fontWeight: 600 }}>
            {kind === "gomoku" ? (
              <><li>黑先，轮流落子，先连五者胜。</li><li>落子于交点，已有棋子处不可落子。</li><li>胜负以最后一手的四方向连珠判定。</li></>
            ) : (
              <><li>黑先，轮流落子于交点。</li><li>提子、气与自杀判定由规则引擎执行。</li><li>同点不可重复落子，终局计目待后续版本。</li></>
            )}
          </ul>
        </div>
        <div className="brutal-card" style={{ padding: 14, background: "#fff" }}>
          <div className="brutal-label">对局</div>
          <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 6, fontSize: 13, lineHeight: 1.6, fontWeight: 600 }}>
            <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
              <span style={{ color: "var(--muted)" }}>手数</span>
              <span style={{ fontFamily: "var(--font-mono)" }}>{props.moveCount}</span>
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
    </>
  );
}

/* ---------------- 本地对战页（与 P2P 同一套棋盘 UI，只是没有邀请链接） ----------------
 * 规则/尺寸由顶部选择器统一控制（与 P2P 共用同一套 kind/size）。 */

function LocalPage(props: { kind: GameKind; size: Size }) {
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

/* ---------------- 主应用 ---------------- */

export default function App() {
  const defs = useMemo(loadDefaults, []);
  const [kind, setKind] = useState<GameKind>(defs.kind);
  const [size, setSize] = useState<Size>(defs.size);
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(defs.size));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);

  // —— 路由 —— //
  const [intent, setIntent] = useState<UrlIntent>(() => parseUrl());

  // —— 身份与联机 —— //
  const [tabUser] = useState(() => myUserId());
  const [name, setName] = useState(() => myName());
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [role, setRole] = useState<Role>("idle");
  const [phase, setPhase] = useState<Phase>("home");
  const [gameId, setGameId] = useState<string | null>(null);
  const [myColor, setMyColor] = useState<StoneColor>("black");
  const [peerConnected, setPeerConnected] = useState(false);
  const [pwd, setPwd] = useState<string | null>(null);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [watchUrl, setWatchUrl] = useState<string | null>(null);
  const [incoming, setIncoming] = useState<{ from: string; fromName: string; kind: GameKind; size: Size; gameId: string } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [directState, setDirectState] = useState<string>("idle");
  const [answerBackUrl, setAnswerBackUrl] = useState<string | null>(null);
  const [copyFb, setCopyFb] = useState<string | null>(null);

  // refs：供消息回调读取最新值
  const boardRef = useRef(board);
  const toMoveRef = useRef(toMove);
  const winnerRef = useRef(winner);
  const historyRef = useRef(history);
  const lastMoveRef = useRef(lastMove);
  const kindRef = useRef(kind);
  const sizeRef = useRef(size);
  const myColorRef = useRef(myColor);
  const roleRef = useRef(role);
  const gameIdRef = useRef<string | null>(null);
  const pwdRef = useRef<string | null>(null);
  const phaseRef = useRef(phase);
  const rtcPeersRef = useRef<DirectRtcPeer[]>([]);
  const hostRtcRef = useRef<DirectRtcPeer | null>(null);
  const bootRef = useRef(false);
  const inviteDoneRef = useRef<string | null>(null);
  useEffect(() => { boardRef.current = board; }, [board]);
  useEffect(() => { toMoveRef.current = toMove; }, [toMove]);
  useEffect(() => { winnerRef.current = winner; }, [winner]);
  useEffect(() => { historyRef.current = history; }, [history]);
  useEffect(() => { lastMoveRef.current = lastMove; }, [lastMove]);
  useEffect(() => { kindRef.current = kind; }, [kind]);
  useEffect(() => { sizeRef.current = size; }, [size]);
  useEffect(() => { myColorRef.current = myColor; }, [myColor]);
  useEffect(() => { roleRef.current = role; }, [role]);
  useEffect(() => { gameIdRef.current = gameId; }, [gameId]);
  useEffect(() => { pwdRef.current = pwd; }, [pwd]);
  useEffect(() => { phaseRef.current = phase; }, [phase]);

  // kind 切换时自动修正 size
  useEffect(() => {
    const nextSize: Size = kind === "gomoku" ? 15 : size === 15 ? 19 : size;
    if (nextSize !== size) setSize(nextSize);
  }, [kind]); // eslint-disable-line react-hooks/exhaustive-deps

  // 主页 size 变化重置棋盘；对局中由 SyncState 驱动
  useEffect(() => {
    if (phase !== "home") return;
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [size, phase]);

  /* ---------- 对局数据消息 ---------- */
  const handleNetMessage = useCallback((msg: GameMsg) => {
    const k = msg.kind;
    switch (k.type) {
      case "SyncRequest": {
        setPeerConnected(true);
        transport.send({
          type: "SyncState",
          board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
          history: historyRef.current, lastMove: lastMoveRef.current,
          kind: kindRef.current, size: sizeRef.current,
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
            board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
            history: historyRef.current, lastMove: lastMoveRef.current,
            kind: kindRef.current, size: sizeRef.current,
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
          if (c.x < 0 || c.y < 0 || c.x >= sizeRef.current || c.y >= sizeRef.current) break;
          if (curBoard[c.y]?.[c.x] !== "empty") break;
          const next = curBoard.map((r) => [...r]);
          next[c.y][c.x] = mover;
          const willWin = kindRef.current === "gomoku" && checkFive(next, c, mover);
          setBoard(next);
          setLastMove(c);
          setHistory((h) => [...h, c]);
          if (willWin) setWinner(mover);
          else setToMove(mover === "black" ? "white" : "black");
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

  useEffect(() => {
    const off = transport.onMessage(handleNetMessage);
    return off;
  }, [handleNetMessage]);

  /* ---------- 在线发现与挑战 ---------- */
  useEffect(() => {
    presence.start();
    const off = presence.onEvent((e) => {
      if (e.type === "peers") {
        setPeers(e.peers.filter((p) => p.id !== tabUser));
      } else if (e.type === "challenge") {
        // 只处理发给我的挑战
        if (phaseRef.current === "playing") return; // 对局中：pwd 已失效，不接受第三人
        if (phaseRef.current === "waiting" && pwdRef.current && e.pwd === pwdRef.current) {
          // 带正确 pwd 的连接请求：自动同意（邀请钥匙语义）
          // 同源页面间：客人 answer 已随该消息自动回传，直接完成直连
          acceptChallenge(e.from, e.kind, e.size, e.gameId, true, e.rtcAns ?? null);
        } else if (phaseRef.current === "home") {
          // 主页：无 pwd 或 pwd 不对 → 弹窗手动确认
          setIncoming({ from: e.from, fromName: e.fromName, kind: e.kind, size: e.size, gameId: e.gameId });
        }
        // waiting 但 pwd 对不上：忽略（第三人拿旧 pwd 无法加入）
      } else if (e.type === "accept") {
        // 我是 guest，主机同意了
        if (roleRef.current === "guest" && gameIdRef.current === e.gameId) {
          enterPlayingAsGuest();
        }
      } else if (e.type === "reject") {
        if (roleRef.current === "guest" && gameIdRef.current === e.gameId) {
          setNotice("对方拒绝了对局");
          backHome();
        }
      }
    });
    return () => { off(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabUser]);

  // presence 状态上报
  useEffect(() => {
    if (phase === "home") presence.setStatus("idle", null);
    else if (phase === "waiting") presence.setStatus("waiting", gameId);
    else presence.setStatus("in-game", gameId);
  }, [phase, gameId]);

  // 路由变化监听（站内 nav / 前进后退）
  useEffect(() => {
    const onPop = () => setIntent(parseUrl());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  /** 按 URL 意图行动：观战加入 / 邀请自动挑战 / 客人回执自动完成。 */
  function processIntent(it: UrlIntent) {
    if (it.mode === "watch" && phaseRef.current === "home") {
      joinAsSpectator(it.gameId);
    } else if (it.mode === "user" && phaseRef.current === "home") {
      if (it.userId === tabUser) {
        // 房主打开客人回执链接（`?rtcAns=`）：自动完成直连，无需任何操作
        const ans = parseAnswerFromUrl();
        if (ans) finishHostRtc(ans);
        return;
      }
      if (!it.pwd) return; // 无 pwd：仅展示对方主页，由用户手动挑战
      const href = window.location.href;
      if (inviteDoneRef.current === href) return;
      inviteDoneRef.current = href;
      // 带 pwd 打开某用户主页：自动发起带钥匙的连接请求；主机校验 pwd 自动同意
      guestChallenge(it.userId, it.pwd, it.kind, it.size, it.rtc ?? null);
    }
  }

  // 启动意图（StrictMode 下守卫只执行一次）
  useEffect(() => {
    if (bootRef.current) return;
    bootRef.current = true;
    processIntent(parseUrl());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 站内路由变化时处理新意图
  useEffect(() => {
    if (!bootRef.current) return;
    processIntent(intent);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intent]);

  function resetBoardFor(k: GameKind, s: Size) {
    setKind(k);
    setSize(s);
    setBoard(emptyBoard(s));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }

  /** 主机：开启对战（waiting），生成每局轮换 pwd + gameId，后台预生成直连 offer 编进邀请链接。 */
  function hostCreate() {
    const g = genGameId();
    const p = genPwd();
    setGameId(g);
    gameIdRef.current = g;
    setPwd(p);
    setRole("host");
    setPhase("waiting");
    setMyColor("black");
    setPeerConnected(false);
    resetBoardFor(kind, size);
    transport.join(g);
    setInviteUrl(inviteToUrl(tabUser, p, kind, size));
    setWatchUrl(null);
    setNotice(null);
    setDirectState("making-invite");
    // 后台预生成 offer：用户只需复制最终邀请链接，无需触碰 offer 文本
    void (async () => {
      const peer = new DirectRtcPeer({ isHost: true, role: "player" });
      attachPeer(peer);
      try {
        const offer = await peer.createOffer();
        hostRtcRef.current = peer;
        setInviteUrl(inviteToUrl(tabUser, p, kind, size, offer));
        setDirectState("waiting-guest");
      } catch {
        hostRtcRef.current = null;
        try { peer.close(); } catch { /* ignore */ }
        // offer 生成失败也不阻塞：同源页面间仍可经 BroadcastChannel 直传对弈
        setDirectState("idle");
      }
    })();
  }

  /** 客人：向某用户发起挑战（pwd 可空）。发起后统一到 P2P 页等待/对战。 */
  function guestChallenge(hostId: string, pwdOrNull: string | null, k: GameKind, s: Size, hostOffer?: string | null) {
    const g = genGameId();
    setGameId(g);
    gameIdRef.current = g;
    setRole("guest");
    setMyColor("white");
    setPeerConnected(false);
    resetBoardFor(k, s);
    transport.join(g);
    setPhase("waiting");
    setPwd(null);
    setInviteUrl(null);
    setWatchUrl(null);
    setAnswerBackUrl(null);
    if (hostOffer) {
      // 跨设备一键直连：邀请链接自带主机 offer，客人后台自动生成 answer。
      // 同源页面间 answer 经 Presence 自动回传；跨设备时生成回执链接由客人发回房主。
      setNotice("邀请已受理，正在建立 P2P 直连…");
      setDirectState("joining");
      nav("/p2p");
      void (async () => {
        const peer = new DirectRtcPeer({ isHost: false, role: "player" });
        attachPeer(peer);
        try {
          const ans = await peer.acceptOffer(hostOffer);
          // guest 的 gameId + answer 发给主机：pwd 校验通过后主机自动完成直连并开始对局
          presence.challenge(hostId, pwdOrNull, k, s, g, ans);
          setAnswerBackUrl(answerToUrl(hostId, pwdOrNull ?? "", ans));
          setNotice("直连应答已生成：同源会自动送达；跨设备请把回执链接发给房主打开");
        } catch {
          setDirectState("error");
          setNotice("直连建立失败，可检查设置页线路后重试");
        }
      })();
      return;
    }
    // guest 的 gameId 发给主机，主机接受后双方用 guest 的 gameId 建 channel
    presence.challenge(hostId, pwdOrNull, k, s, g);
    setNotice(pwdOrNull ? "已带邀请钥匙请求连接，等待主机自动确认…" : "已发送挑战，等待对方同意…");
    nav("/p2p");
  }

  /** 房主用客人回传的 answer 完成直连（同源自动；跨设备由回执链接自动触发）。 */
  async function finishHostRtc(ans: string) {
    const peer = hostRtcRef.current;
    if (!peer || peer.state !== "waiting-guest") return;
    try {
      await peer.acceptAnswer(ans);
      setNotice("对方已加入，直连建立中…");
    } catch {
      setDirectState("error");
      setNotice("直连建立失败，可检查设置页线路后重试");
    }
  }

  /** 主机接受挑战：pwd 失效（两人满员），进入 playing。 */
  function acceptChallenge(from: string, k: GameKind, s: Size, guestGameId: string, auto: boolean, guestAns?: string | null) {
    if (guestAns) void finishHostRtc(guestAns);
    // 切换到 guest 的 game channel（两人同一 channel）
    transport.join(guestGameId);
    setGameId(guestGameId);
    setRole("host");
    setPhase("playing");
    setMyColor("black");
    setPeerConnected(false);
    resetBoardFor(k, s);
    // pwd 失效：两人已满，不再接受第三人
    setPwd(null);
    setInviteUrl(null);
    setWatchUrl(watchToUrl(guestGameId));
    presence.accept(from, guestGameId);
    setIncoming(null);
    setNotice(auto ? "邀请钥匙校验通过，已自动开始对局" : "已接受挑战，对局开始");
    nav("/p2p");
    setTimeout(() => {
      transport.send({ type: "Hello", kind: k, size: s });
      transport.send({ type: "SyncRequest" });
    }, 80);
  }

  function rejectChallenge(from: string, guestGameId: string) {
    presence.reject(from, guestGameId);
    setIncoming(null);
    setNotice("已拒绝该挑战");
  }

  function enterPlayingAsGuest() {
    setPhase("playing");
    setPwd(null);
    setWatchUrl(watchToUrl(gameIdRef.current ?? ""));
    setNotice("对方已同意，对局开始（你执白）");
    setTimeout(() => {
      transport.send({ type: "Hello", kind: kindRef.current, size: sizeRef.current });
      transport.send({ type: "SyncRequest" });
    }, 80);
  }

  function joinAsSpectator(g: string) {
    setGameId(g);
    gameIdRef.current = g;
    setRole("spectator");
    setPhase("playing");
    setMyColor("white");
    setPeerConnected(false);
    transport.join(g);
    setPwd(null);
    setInviteUrl(null);
    setWatchUrl(null);
    setNotice("观战模式：只读同步，不可落子");
    setTimeout(() => transport.send({ type: "SyncRequest" }), 120);
  }

  function backHome() {
    transport.leave();
    for (const p of rtcPeersRef.current) { try { p.close(); } catch { /* ignore */ } }
    rtcPeersRef.current = [];
    hostRtcRef.current = null;
    setRole("idle");
    setPhase("home");
    setGameId(null);
    gameIdRef.current = null;
    setPwd(null);
    setInviteUrl(null);
    setWatchUrl(null);
    setIncoming(null);
    setPeerConnected(false);
    setMyColor("black");
    setNotice(null);
    setAnswerBackUrl(null);
    setDirectState("idle");
    nav("/");
    setBoard(emptyBoard(sizeRef.current));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
  }

  async function copyText(t: string, okMsg: string) {
    try {
      await navigator.clipboard.writeText(t);
      setCopyFb(okMsg);
      setTimeout(() => setCopyFb(null), 1600);
    } catch {
      setCopyFb("复制失败，请手动复制");
      setTimeout(() => setCopyFb(null), 2000);
    }
  }

  /* ---------- P2P 直连（邀请链接自动信令，无服务器、无手动输入） ---------- */

  function attachPeer(p: DirectRtcPeer) {
    p.onRemote = (msg) => transport.injectRemote(msg);
    p.onState = (s) => {
      setDirectState(s);
      if (s === "open") {
        setPeerConnected(true);
        // 直连建立后主动同步一次
        setTimeout(() => {
          const sync = {
            type: "SyncState",
            board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
            history: historyRef.current, lastMove: lastMoveRef.current,
            kind: kindRef.current, size: sizeRef.current,
          } as const;
          transport.send(sync);
          for (const q of rtcPeersRef.current) {
            try {
              q.send({ seq: Date.now(), sender: transport.myId, userId: tabUser, kind: { ...sync } });
            } catch { /* ignore */ }
          }
        }, 200);
      }
    };
    rtcPeersRef.current.push(p);
  }

  /* ---------- 落子 ---------- */
  function handlePlace(c: Coord) {
    if (winner) return;
    if (board[c.y][c.x] !== "empty") return;
    if (role === "spectator") return;
    if (phase === "waiting") return;
    if (phase === "playing" && peerConnected && toMove !== myColor) return;
    if (phase === "playing" && !peerConnected) return;

    const mover = toMove;
    const next = board.map((row) => [...row]);
    next[c.y][c.x] = mover;
    const willWin = kind === "gomoku" && checkFive(next, c, mover);
    setBoard(next);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (willWin) setWinner(mover);
    else setToMove(mover === "black" ? "white" : "black");

    if (phase === "playing") {
      const msg = { type: "Move", move: { type: "Place", coord: c } } as const;
      transport.send(msg);
      for (const q of rtcPeersRef.current) {
        try {
          q.send({ seq: Date.now(), sender: transport.myId, userId: tabUser, kind: { ...msg } });
        } catch { /* ignore */ }
      }
    }
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
    if (phase === "playing") {
      const msg = { type: "Reset", kind: k, size: s } as const;
      transport.send(msg);
      for (const q of rtcPeersRef.current) {
        try {
          q.send({ seq: Date.now(), sender: transport.myId, userId: tabUser, kind: { ...msg } });
        } catch { /* ignore */ }
      }
    }
  }

  function saveName() {
    setMyName(name.trim());
    setNotice(name.trim() ? `昵称已保存：${name.trim()}` : "昵称已清空");
    setTimeout(() => setNotice(null), 1600);
  }

  const moveCount = history.length;
  const myHomeUrl = userToUrl(tabUser);

  const statusText = useMemo(() => {
    if (winner) return `${winner === "black" ? "黑" : "白"} 胜`;
    return `${toMove === "black" ? "黑" : "白"} 落子`;
  }, [winner, toMove]);

  const p2pStatusText = useMemo(() => {
    if (phase === "home") return role === "spectator" ? "观战中" : "主页 · 选择对手或等待被挑战";
    if (phase === "waiting") return `等待对手 · 你是${myColor === "black" ? "黑" : "白"} · pwd 本局有效`;
    if (!peerConnected) return `连接中 · 你是${myColor === "black" ? "黑" : "白"}`;
    return `已直连 · 你是${myColor === "black" ? "黑" : "白"}${role === "spectator" ? "（观战）" : toMove === myColor ? " · 轮到你" : " · 等待对手"}`;
  }, [phase, role, peerConnected, myColor, toMove]);

  const boardDisabled = useMemo(() => {
    if (winner) return true;
    if (role === "spectator") return true;
    if (phase !== "playing") return phase === "waiting";
    if (!peerConnected) return true;
    return toMove !== myColor;
  }, [winner, role, phase, peerConnected, toMove, myColor]);

  const rtcStatus = <RtcStatusLine directState={directState} />;

  const incomingBanner = incoming && (
    <div className="brutal-card" style={{ padding: "8px 10px", background: "#fff", display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>
        {incoming.fromName} 向你发起对局（{incoming.kind === "gomoku" ? "五子棋" : "围棋"} {incoming.size}×{incoming.size}）
      </span>
      <button className="brutal-btn brutal-btn--sm brutal-btn--accent"
        onClick={() => acceptChallenge(incoming.from, incoming.kind, incoming.size, incoming.gameId, false)}>同意</button>
      <button className="brutal-btn brutal-btn--sm" onClick={() => rejectChallenge(incoming.from, incoming.gameId)}>拒绝</button>
    </div>
  );

  const mode = intent.mode;
  const viewedUserId = mode === "user" ? intent.userId : null;
  const viewedPeer = viewedUserId && viewedUserId !== tabUser ? peers.find((p) => p.id === viewedUserId) ?? null : null;
  const isSelfPage = viewedUserId === tabUser;

  // 顶部选择器：恢复 bb452d9 的五子棋/围棋 + 尺寸布局，并在左下保留一个"选项"入口。
  // 对局中（非主页）禁用切换，与旧版"联机中不可切换"一致。
  const topLocked = phase !== "home";
  const topLockedTitle = topLocked ? "对局/等待中不可切换，请先取消或离开" : undefined;

  function pickKind(k: GameKind) {
    if (topLocked) return;
    setKind(k);
  }

  function pickSize(s: Size) {
    if (topLocked) return;
    setSize(s);
  }

  return (
    <div style={{ height: "100dvh", minHeight: "100dvh", display: "flex", flexDirection: "column", background: "var(--bg-page)", overflow: "hidden" }}>
      <div className="poster-strip">GoPtop · P2P Gomoku & Go · Neubrutalism · 用户直连 · 无服务器无中转</div>

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
              onClick={() => pickKind("gomoku")}
              aria-pressed={kind === "gomoku"}
              title={topLockedTitle}
              disabled={topLocked}
            >
              五子棋
            </button>
            <button
              className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
              onClick={() => pickKind("go")}
              aria-pressed={kind === "go"}
              title={topLockedTitle}
              disabled={topLocked}
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
                onClick={() => pickSize(s as Size)}
                aria-pressed={size === s}
                disabled={topLocked}
                title={topLockedTitle ?? undefined}
              >
                {s}×{s}
              </button>
            ))}
          </div>
          <div style={{ width: 1, height: 26, background: "var(--ink)", opacity: 0.18 }} />
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")} title="回选项页（本地对战 / P2P 对战 / 在线用户 / 设置）">
            选项
          </button>
        </div>
      </header>

      <main style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "flex-start", padding: "clamp(6px, 1.2vh, 12px) 12px clamp(6px, 1vh, 10px)", width: "100%", maxWidth: 760, margin: "0 auto", overflow: "auto" }}>
        <div className="play-stack">
          {incomingBanner}

          {/* —— 选项页 `/` —— */}
          {mode === "menu" && (
            <div className="brutal-card" style={{ padding: "16px 14px", background: "#fff", display: "flex", flexDirection: "column", gap: 12 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">开始 · 选一个玩法</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
              </div>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
                <button className="brutal-btn" style={{ padding: "16px 8px" }} onClick={() => nav("/local")}>本地对战</button>
                <button className="brutal-btn brutal-btn--accent" style={{ padding: "16px 8px" }} onClick={() => nav("/p2p")}>P2P 对战</button>
                <button className="brutal-btn" style={{ padding: "16px 8px" }} onClick={() => nav("/users")}>在线用户（{peers.length}）</button>
                <button className="brutal-btn" style={{ padding: "16px 8px" }} onClick={() => nav("/settings")}>设置</button>
              </div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(tabUser)}`)}>我的主页</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </div>
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                用户对用户直连：每人一个主页（/:userId），邀请链接带本局钥匙 pwd（自动同意）；无 pwd 则手动挑战。两人进对局后 pwd 失效，只剩观战链接。
              </div>
            </div>
          )}

          {/* —— 本地对战 `/local`：与 P2P 同一套棋盘 UI，只是没有邀请链接 —— */}
          {mode === "local" && <LocalPage kind={kind} size={size} />}

          {/* —— P2P 对战大厅 `/p2p` —— */}
          {mode === "p2p" && phase === "home" && (
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">P2P 对战大厅</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
              </div>
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                规则/尺寸用顶部选择器统一设置（与本地对战共用）。
              </div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={hostCreate}>开启对战（等对手）</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </div>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap", marginTop: 4 }}>
                <span className="brutal-label">在线用户（{peers.length}）</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>点挑战即发起对局，对方同意后进入</span>
              </div>
              <PeerList
                peers={peers}
                emptyHint="暂无其他在线用户。把你的主页链接发给对方，对方打开即可向你发起挑战。"
                actionLabel={() => "挑战"}
                onAction={(p) => guestChallenge(p.id, null, kind, size)}
                extraAction={(p) => (
                  <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(p.id)}`)}>主页</button>
                )}
              />
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
            </div>
          )}

          {mode === "p2p" && phase === "waiting" && (
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fffbeb", display: "flex", flexDirection: "column", gap: 8 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">等待对手 · 邀请（本局有效）</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{p2pStatusText}</span>
              </div>
              {role === "host" && inviteUrl ? (
                <>
                  <code style={{ border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{inviteUrl}</code>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制邀请链接</button>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消等待</button>
                    {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                  </div>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                    对方只需打开此链接即自动加入并建立直连（pwd 为本局钥匙，每局轮换；直连信息已编进链接）。
                    跨设备若直连未自动建立，对方页会出现一条回执链接，发回给你打开即可。
                    两人进对局后 pwd 失效，不可再加入第三人；此邀请区将隐藏。
                  </div>
                  {rtcStatus}
                </>
              ) : (
                <>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>{notice ?? "等待对方确认…"}</div>
                  {answerBackUrl && (
                    <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                      <span className="brutal-label">跨设备回执（对方房主打不开自动通道时，把这条链接发给房主打开）</span>
                      <code style={{ border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{answerBackUrl}</code>
                      <div><button className="brutal-btn brutal-btn--sm" onClick={() => copyText(answerBackUrl, "回执链接已复制")}>复制回执链接</button></div>
                    </div>
                  )}
                  <div style={{ display: "flex", gap: 8 }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消</button>
                  </div>
                </>
              )}
            </div>
          )}

          {mode === "p2p" && phase === "playing" && (
            <>
              <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                  <span className="brutal-label">对局 · 直连</span>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: peerConnected ? "#0a7a2e" : "var(--muted)" }}>{p2pStatusText}</span>
                </div>
                {watchUrl && role !== "spectator" && (
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                    <code style={{ flex: "1 1 220px", minWidth: 180, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{watchUrl}</code>
                    <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(watchUrl, "观战链接已复制")}>邀请观战</button>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>离开对局</button>
                    {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                  </div>
                )}
                {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              </div>
              <BoardPanel
                kind={kind} size={size} board={board} toMove={toMove} winner={winner}
                lastMove={lastMove} hover={hover} onHover={setHover}
                disabled={boardDisabled} onPlace={handlePlace}
                statusText={statusText} statusNote={`联机 · ${myColor === "black" ? "你执黑" : "你执白"}`}
                moveCount={moveCount} history={history}
                onUndo={null} onReset={reset}
                actions={watchUrl
                  ? <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(watchUrl, "观战链接已复制")}>复制观战链接</button>
                  : undefined}
              />
              {rtcStatus}
            </>
          )}

          {/* —— 在线用户 `/users` —— */}
          {mode === "users" && (
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">在线用户（{peers.length}）</span>
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回选项页</button>
              </div>
              {phase !== "home" && (
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
                  你当前{phase === "waiting" ? "正在等待对手" : "正在对局中"}（规则/尺寸用顶部选择器，对局中已锁定）。
                  <button className="brutal-btn brutal-btn--sm" style={{ marginLeft: 8 }} onClick={() => nav("/p2p")}>前往 P2P 页</button>
                </div>
              )}
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                规则/尺寸用顶部选择器统一设置（与本地对战共用）。
              </div>
              <PeerList
                peers={peers}
                emptyHint="暂无其他在线用户。把你的主页链接发给对方，对方打开即可向你发起挑战。"
                actionLabel={() => "挑战"}
                onAction={(p) => { if (phase === "home") guestChallenge(p.id, null, kind, size); }}
                extraAction={(p) => (
                  <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(p.id)}`)}>主页</button>
                )}
              />
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
            </div>
          )}

          {/* —— 设置 `/settings` —— */}
          {mode === "settings" && (
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 10 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">设置</span>
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回选项页</button>
              </div>
              <div>
                <div className="brutal-label" style={{ marginBottom: 6 }}>昵称（在线用户列表中显示）</div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <input placeholder="给自己起个昵称" value={name} onChange={(e) => setName(e.target.value)}
                    style={{ flex: "1 1 160px", minWidth: 140, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
                  <button className="brutal-btn brutal-btn--sm" onClick={saveName}>保存昵称</button>
                </div>
              </div>
              <div>
                <div className="brutal-label" style={{ marginBottom: 6 }}>默认规则与尺寸（新对局/开页时使用，也可直接用顶部选择器切换）</div>
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
                  当前：{kind === "gomoku" ? "五子棋" : "围棋"} {size}×{size}
                  {phase !== "home" && "（对局/等待中，顶部已锁定）"}
                </div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginTop: 6 }}>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => {
                    try { localStorage.setItem("goptop:defaults", JSON.stringify({ kind, size })); } catch { /* ignore */ }
                    setNotice("已保存当前顶部选择为默认值");
                    setTimeout(() => setNotice(null), 1600);
                  }} disabled={phase !== "home"} title={phase !== "home" ? "对局/等待中不可保存" : undefined}>
                    保存当前选择为默认
                  </button>
                </div>
              </div>
              <div>
                <div className="brutal-label" style={{ marginBottom: 6 }}>我的身份</div>
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)", overflowWrap: "anywhere" }}>{tabUser}</div>
                <div style={{ display: "flex", gap: 8, marginTop: 6, flexWrap: "wrap" }}>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(tabUser)}`)}>打开我的主页</button>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                  {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                </div>
              </div>
              <StunSettings />
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                直连约束：同源页面间走 BroadcastChannel 同源直传；跨设备走 WebRTC DataChannel（仅上面启用的 STUN、无 TURN 中转），信令随邀请链接自动走，无信令服务器、无手动输入。
              </div>
            </div>
          )}

          {/* —— 用户主页 `/<userId>` —— */}
          {mode === "user" && viewedUserId && (
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
              {isSelfPage ? (
                <>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                    <span className="brutal-label">我的主页</span>
                    <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
                  </div>
                  {phase === "waiting" && role === "host" && inviteUrl ? (
                    <>
                      <span className="brutal-label">等待对手 · 邀请（本局有效）</span>
                      <code style={{ border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{inviteUrl}</code>
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制邀请链接</button>
                        <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/p2p")}>前往 P2P 页</button>
                        <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消等待</button>
                      </div>
                    </>
                  ) : phase === "playing" ? (
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>对局中（pwd 已失效，不可再加入）。</span>
                      <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => nav("/p2p")}>回到对局</button>
                      {watchUrl && <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(watchUrl, "观战链接已复制")}>复制观战链接</button>}
                    </div>
                  ) : (
                    <>
                      <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                        规则/尺寸用顶部选择器统一设置。
                      </div>
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={hostCreate}>开启对战（等对手）</button>
                        <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                        {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                      </div>
                    </>
                  )}
                </>
              ) : (
                <>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                    <span className="brutal-label">用户主页</span>
                    <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)", overflowWrap: "anywhere" }}>{viewedUserId}</span>
                  </div>
                  {phase !== "home" ? (
                    <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
                      {intent.mode === "user" && intent.pwd ? "邀请已受理，正在进入对局…" : "你当前不在空闲状态，无法发起新的挑战。"}
                      <button className="brutal-btn brutal-btn--sm" style={{ marginLeft: 8 }} onClick={() => nav("/p2p")}>前往 P2P 页</button>
                    </div>
                  ) : viewedPeer ? (
                    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", border: "3px solid var(--ink)", padding: "8px 10px", background: "#fffbeb" }}>
                      <span style={{ fontFamily: "var(--font-mono)", fontSize: 13, fontWeight: 800 }}>{viewedPeer.name}</span>
                      <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>
                        {viewedPeer.status === "idle" ? "空闲" : viewedPeer.status === "waiting" ? "等待对手中" : "对局中"}
                      </span>
                      <span style={{ flex: 1 }} />
                      <button className="brutal-btn brutal-btn--sm brutal-btn--accent"
                        onClick={() => guestChallenge(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null)}
                        disabled={viewedPeer.status === "in-game"}
                        title={viewedPeer.status === "in-game" ? "对方对局中，不可挑战" : "向其发起对局"}>
                        {intent.mode === "user" && intent.pwd ? "接受邀请进入对局" : "挑战"}
                      </button>
                    </div>
                  ) : (
                    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                      <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                        在在线用户列表中暂未发现该用户（对方可能已离线）。仍可尝试发起挑战。
                      </div>
                      <div>
                        <button className="brutal-btn brutal-btn--sm brutal-btn--accent"
                          onClick={() => guestChallenge(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null)}>
                          {intent.mode === "user" && intent.pwd ? "接受邀请进入对局" : "挑战"}
                        </button>
                      </div>
                    </div>
                  )}
                  {phase === "home" && (
                    <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                      规则/尺寸用顶部选择器统一设置。
                    </div>
                  )}
                </>
              )}
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              {copyFb && <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</div>}
            </div>
          )}

          {/* —— 观战 `/watch/<game>` —— */}
          {mode === "watch" && (
            <div className="play-stack">
              <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                  <span className="brutal-label">观战 · 只读</span>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: peerConnected ? "#0a7a2e" : "var(--muted)" }}>{p2pStatusText}</span>
                </div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>观战中（只读，不可落子）</span>
                  <button className="brutal-btn brutal-btn--sm" onClick={backHome}>离开</button>
                </div>
                {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              </div>
              <BoardPanel
                kind={kind} size={size} board={board} toMove={toMove} winner={winner}
                lastMove={lastMove} hover={hover} onHover={setHover}
                disabled onPlace={() => undefined}
                statusText={statusText} statusNote="观战 · 只读"
                moveCount={moveCount} history={history}
                onUndo={null} onReset={() => undefined}
              />
              {rtcStatus}
            </div>
          )}
        </div>
      </main>

      <style>{`
        @media (max-width: 640px) {
          .bottom-grid { grid-template-columns: 1fr !important; }
        }
      `}</style>

      <footer style={{ flexShrink: 0, padding: "10px 16px", borderTop: "3px solid var(--ink)", background: "#fff", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.06em", textTransform: "uppercase", color: "var(--muted)", textAlign: "center" }}>
        GoPtop · P2P Gomoku & Go · 用户直连 · 无服务器无中转
      </footer>
    </div>
  );
}
