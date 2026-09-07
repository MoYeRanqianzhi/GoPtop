/**
 * App 拆分（D1 两刀完成后）——跨页面复用的展示组件与纯函数。
 *
 * 从 App.tsx 原样搬出（禁止行为变化）：PeerList / StunSettings / RtcStatusLine /
 * BoardPanel / loadDefaults / Role/Phase 类型。
 * 页面级 JSX 在 pages/ 下；对局状态机与信令编排已抽到 state/useGameSession.tsx。
 */
import { useState } from "react";
import type { ReactNode } from "react";
import type { Coord, StoneColor } from "../components/BoardSvg";
import { BoardSvg } from "../components/BoardSvg";
import { loadStunLines, saveStunLines } from "../net/transport";
import type { GameKind, PeerInfo, Size, StunLine } from "../net/transport";

export type Role = "idle" | "inviter" | "invitee" | "spectator";
export type Phase = "home" | "waiting" | "playing";

/** 设置页持久化的默认规则/尺寸。 */
export function loadDefaults(): { kind: GameKind; size: Size } {
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

/* ---------------- 通用小组件 ---------------- */

export function PeerList(props: {
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

export function StunSettings() {
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
      <div className="brutal-label" style={{ marginBottom: 6 }}>直连线路（STUN）</div>
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
        至少保留一条启用线路。
      </div>
    </div>
  );
}

export function RtcStatusLine(props: { directState: string }) {
  const { directState } = props;
  const text = directState === "open" ? "直连已建立"
    : directState === "making-invite" ? "正在准备直连邀请…"
    : directState === "waiting-invitee" ? "邀请已就绪 · 等待对方打开邀请链接"
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

export function BoardPanel(props: {
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
              <><li>黑先，轮流落子于交点。</li><li>同点不可重复落子。</li><li>提子、禁自杀、终局数目为后续版本（见 docs/已知限制与路线图.md）。</li></>
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
