/**
 * 观战页 `/watch/<game>`（原 App.tsx JSX 原样搬出，禁止行为变化）：只读棋盘 + 状态卡。
 */
import type { GameSession } from "../state/useGameSession";
import { BoardPanel } from "./components";
import { NoticeLine } from "../components/NoticeLine";

export function WatchPage(props: { s: GameSession }) {
  const {
    kind, size, board, toMove, winner, lastMove, hover,
    peerConnected, p2pStatusText, notice, statusText, moveCount,
    setHover, backHome,
  } = props.s;
  return (
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
        <NoticeLine text={notice} />
      </div>
      <BoardPanel
        kind={kind} size={size} board={board} toMove={toMove} winner={winner}
        lastMove={lastMove} hover={hover} onHover={setHover}
        disabled onPlace={() => undefined}
        statusText={statusText} statusNote="观战 · 只读"
        moveCount={moveCount}
        onUndo={null} onReset={null}
      />
    </div>
  );
}
