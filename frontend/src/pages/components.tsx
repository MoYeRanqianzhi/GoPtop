/**
 * App 拆分（D1 两刀完成后）——跨页面复用的展示组件与纯函数。
 *
 * 从 App.tsx 原样搬出（禁止行为变化）：PeerList / StunSettings /
 * BoardPanel / loadDefaults / Role/Phase 类型。
 * 页面级 JSX 在 pages/ 下；对局状态机与信令编排已抽到 state/useGameSession.tsx。
 */
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import { BoardSvg } from "../components/BoardSvg";
import { loadStunLines, saveStunLines } from "../net/stun";
import type { StunLine } from "../net/stun";
import {
  BUILTIN_SERVERS, SERVER_NONE, loadServerSelection, loadServers,
  saveServerSelection, saveServers,
} from "../net/servers";
import type { ServerEntry } from "../net/servers";
import type { PeerInfo } from "../net/presence";

/* ---------------- 头像（圆形，本地存储，P2P 交换） ---------------- */

/** 圆形头像：有图显示图，无图显示首字。dataUrl 由调用方保证已圆形裁剪。 */
export function Avatar(props: { dataUrl: string | null; name: string; size: number }) {
  const { dataUrl, name, size } = props;
  if (dataUrl) {
    return <img src={dataUrl} alt={name} style={{ width: size, height: size, borderRadius: "50%", border: "2px solid var(--ink)", objectFit: "cover", flexShrink: 0, background: "#fff" }} />;
  }
  return (
    <span style={{ width: size, height: size, borderRadius: "50%", border: "2px solid var(--ink)", display: "inline-flex", alignItems: "center", justifyContent: "center", fontFamily: "var(--font-mono)", fontWeight: 800, fontSize: size * 0.42, flexShrink: 0, background: "#fff" }}>
      {(name || "?").slice(0, 1).toUpperCase()}
    </span>
  );
}

/** 选图 → 居中方形裁剪 → 圆形 mask → 128px PNG dataURL。 */
export async function cropAvatarToCircle(file: File): Promise<string> {
  const bitmap = await createImageBitmap(file);
  const side = Math.min(bitmap.width, bitmap.height);
  const sx = (bitmap.width - side) / 2;
  const sy = (bitmap.height - side) / 2;
  const OUT = 128;
  const canvas = document.createElement("canvas");
  canvas.width = OUT;
  canvas.height = OUT;
  const ctx = canvas.getContext("2d")!;
  ctx.beginPath();
  ctx.arc(OUT / 2, OUT / 2, OUT / 2, 0, Math.PI * 2);
  ctx.clip();
  ctx.drawImage(bitmap, sx, sy, side, side, 0, 0, OUT, OUT);
  bitmap.close();
  return canvas.toDataURL("image/png");
}

/** 头像设置块（我的主页）：上传即裁剪保存，可清除。 */
export function AvatarSettings(props: { dataUrl: string | null; onSave: (dataUrl: string | null) => void }) {
  const { dataUrl, onSave } = props;
  const fileRef = useRef<HTMLInputElement | null>(null);
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
      <Avatar dataUrl={dataUrl} name="我" size={56} />
      <input ref={fileRef} type="file" accept="image/*" style={{ display: "none" }}
        onChange={async (e) => {
          const f = e.target.files?.[0];
          if (f) onSave(await cropAvatarToCircle(f));
          e.target.value = "";
        }} />
      <button className="brutal-btn brutal-btn--sm" onClick={() => fileRef.current?.click()}>上传头像</button>
      {dataUrl && <button className="brutal-btn brutal-btn--sm" onClick={() => onSave(null)}>清除</button>}
      <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>圆形裁剪 · 存本地 · 对局开始后自动发给对手</span>
    </div>
  );
}

/* ---------------- 聊天面板（对局侧栏 / 窄屏弹窗共用） ---------------- */

export type ChatEntry = { userId: string; name: string; text: string; ts: number; self: boolean };

/** 单局聊天面板：消息流 + 输入 + 协商动作 + 观战房间管理。 */
export function ChatPanel(props: {
  role: "idle" | "inviter" | "invitee" | "spectator";
  chatLog: ChatEntry[];
  peerAvatars: Record<string, string>;
  spectators: { id: string; name: string; host: string; muted: boolean }[];
  specRequests: { from: string; fromName: string }[];
  specCanChat: boolean;
  spectateEnabled: boolean;
  onSend: (text: string) => void;
  onUndo: () => void;
  onReset: () => void;
  onSwap: () => void;
  onKick: (id: string) => void;
  onMute: (id: string, muted: boolean) => void;
  onDisableSpectate: () => void;
  onRequestSpecChat: () => void;
  onApproveSpec: (from: string) => void;
  onRejectSpec: (from: string) => void;
}) {
  const { role, chatLog, peerAvatars, spectators, specRequests, specCanChat, spectateEnabled } = props;
  const [text, setText] = useState("");
  const isPlayer = role === "inviter" || role === "invitee";
  const listRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [chatLog.length]);

  function send() {
    const t = text.trim();
    if (!t) return;
    props.onSend(t);
    setText("");
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, height: "100%", minHeight: 0 }}>
      {/* 协商动作（仅对局者） */}
      {isPlayer && (
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
          <button className="brutal-btn brutal-btn--sm" onClick={props.onUndo}>悔棋</button>
          <button className="brutal-btn brutal-btn--sm" onClick={props.onReset}>重开</button>
          <button className="brutal-btn brutal-btn--sm" onClick={props.onSwap}>换棋</button>
        </div>
      )}
      {/* 观战者：申请发言 / 已批准可发言 */}
      {role === "spectator" && (
        <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>
          {specCanChat ? "双方已同意，可参与聊天" : "观战模式：只读，可申请发言（一次机会，需双方同意）"}
        </div>
      )}
      {role === "spectator" && !specCanChat && (
        <button className="brutal-btn brutal-btn--sm" onClick={props.onRequestSpecChat}>申请发言</button>
      )}
      {/* 消息流 */}
      <div ref={listRef} style={{ flex: 1, minHeight: 120, overflowY: "auto", display: "flex", flexDirection: "column", gap: 6, padding: "4px 2px" }}>
        {chatLog.length === 0 && (
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>暂无消息</div>
        )}
        {chatLog.map((m, i) => (
          <div key={`${m.ts}-${i}`} style={{ display: "flex", gap: 6, alignItems: "flex-start", flexDirection: m.self ? "row-reverse" : "row" }}>
            <Avatar dataUrl={m.self ? null : peerAvatars[m.userId] ?? null} name={m.name} size={22} />
            <div style={{ maxWidth: "78%" }}>
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 10, fontWeight: 700, color: "var(--muted)", textAlign: m.self ? "right" : "left" }}>{m.name}</div>
              <div style={{ display: "inline-block", border: "2px solid var(--ink)", padding: "3px 7px", fontSize: 12, fontWeight: 600, background: m.self ? "#fffbeb" : "#fff", overflowWrap: "anywhere" }}>{m.text}</div>
            </div>
          </div>
        ))}
      </div>
      {/* 观战申请（对局者处理，聊天区私有消息） */}
      {isPlayer && specRequests.length > 0 && (
        <div style={{ border: "2px solid var(--ink)", background: "#fffbeb", padding: 6, display: "flex", flexDirection: "column", gap: 4 }}>
          {specRequests.map((r) => (
            <div key={r.from} style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
              <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800 }}>{r.fromName} 申请观战（钥匙不正确）</span>
              <span style={{ flex: 1 }} />
              <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => props.onApproveSpec(r.from)}>同意</button>
              <button className="brutal-btn brutal-btn--sm" onClick={() => props.onRejectSpec(r.from)}>拒绝</button>
            </div>
          ))}
        </div>
      )}
      {/* 观战房间管理（对局者） */}
      {isPlayer && (
        <div style={{ borderTop: "2px solid var(--ink)", paddingTop: 6, display: "flex", flexDirection: "column", gap: 4 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span className="brutal-label">观战（{spectators.length}）</span>
            <span style={{ flex: 1 }} />
            {spectateEnabled ? (
              <button className="brutal-btn brutal-btn--sm" onClick={props.onDisableSpectate}>关闭观战</button>
            ) : (
              <span style={{ fontFamily: "var(--font-mono)", fontSize: 10, fontWeight: 700, color: "#b00020" }}>已关闭</span>
            )}
          </div>
          {spectators.map((s) => (
            <div key={s.id} style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
              <Avatar dataUrl={peerAvatars[s.id] ?? null} name={s.name} size={20} />
              <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800 }}>{s.name}</span>
              {s.muted && <span style={{ fontFamily: "var(--font-mono)", fontSize: 10, fontWeight: 700, color: "#b00020" }}>已禁言</span>}
              <span style={{ flex: 1 }} />
              <button className="brutal-btn brutal-btn--sm" onClick={() => props.onMute(s.id, !s.muted)}>{s.muted ? "解除禁言" : "禁言"}</button>
              <button className="brutal-btn brutal-btn--sm" onClick={() => props.onKick(s.id)}>踢出</button>
            </div>
          ))}
        </div>
      )}
      {/* 输入框：观战者需批准；禁言/关闭后不可发 */}
      {role === "spectator" && (!specCanChat || !spectateEnabled) ? null : (
        <div style={{ display: "flex", gap: 6 }}>
          <input value={text} onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") send(); }}
            placeholder="说点什么…"
            style={{ flex: 1, minWidth: 0, border: "3px solid var(--ink)", padding: "6px 9px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
          <button className="brutal-btn brutal-btn--sm" onClick={send} disabled={!text.trim()}>发送</button>
        </div>
      )}
    </div>
  );
}

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

/* ---------------- 设置页：信令服务器（单选切换） ---------------- */

/** 服务器选择（单选，非开关）：无服务器 / 官方服务器 / 用户自建。
 *  同一时刻只连一台；切换即保存并刷新页面生效。 */
export function ServerSettings() {
  const [customLabel, setCustomLabel] = useState("");
  const [customUrl, setCustomUrl] = useState("");
  const custom = loadServers();
  const selected = loadServerSelection();
  const all: (ServerEntry | { id: typeof SERVER_NONE; label: string; url: string; builtin: boolean })[] = [
    { id: SERVER_NONE, label: "无服务器（纯直连）", url: "", builtin: true },
    ...BUILTIN_SERVERS,
    ...custom,
  ];

  function pick(id: string) {
    if (id === selected) return;
    saveServerSelection(id);
    window.location.reload();
  }

  function addCustom() {
    const url = customUrl.trim();
    if (!/^wss?:\/\//.test(url)) return;
    const entry: ServerEntry = { id: `srv-${Date.now().toString(36)}`, label: customLabel.trim() || url, url, builtin: false };
    saveServers([...custom, entry]);
    setCustomLabel("");
    setCustomUrl("");
  }

  return (
    <div>
      <div className="brutal-label" style={{ marginBottom: 6 }}>服务器（单选切换，同一时刻只连一台）</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {all.map((s) => (
          <div key={s.id} style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", border: "3px solid var(--ink)", padding: "6px 8px", background: selected === s.id ? "#fffbeb" : "#fff" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>{s.label}</span>
            {s.url && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", overflowWrap: "anywhere" }}>{s.url}</span>}
            <span style={{ flex: 1 }} />
            {!s.builtin && (
              <button className="brutal-btn brutal-btn--sm" onClick={() => { saveServers(custom.filter((c) => c.id !== s.id)); if (selected === s.id) saveServerSelection(SERVER_NONE); window.location.reload(); }}>删除</button>
            )}
            <button className={`brutal-btn brutal-btn--sm ${selected === s.id ? "brutal-btn--active" : ""}`}
              onClick={() => pick(s.id)} aria-pressed={selected === s.id}>
              {selected === s.id ? "使用中" : "切换"}
            </button>
          </div>
        ))}
      </div>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginTop: 8 }}>
        <input placeholder="名称（可选）" value={customLabel} onChange={(e) => setCustomLabel(e.target.value)}
          style={{ flex: "1 1 120px", minWidth: 120, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
        <input placeholder="wss://服务器地址/ws" value={customUrl} onChange={(e) => setCustomUrl(e.target.value)}
          style={{ flex: "2 1 200px", minWidth: 180, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
        <button className="brutal-btn brutal-btn--sm" onClick={addCustom} disabled={!/^wss?:\/\//.test(customUrl.trim())}>添加服务器</button>
      </div>
      <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5, marginTop: 6 }}>
        选服务器：在线名册 + 短码邀请/观战（免回执）+ 连不上时的加密中转；不同服务器之间无法对战。
      </div>
    </div>
  );
}

export function BoardPanel(props: {
  kind: GameKind; size: Size;
  board: StoneColor[][]; toMove: StoneColor; winner: StoneColor | null;
  lastMove: Coord | null; hover: Coord | null; onHover: (c: Coord | null) => void;
  disabled: boolean; onPlace: (c: Coord) => void;
  statusText: string; statusNote: string; moveCount: number; history: Coord[];
  onUndo: (() => void) | null; onReset: (() => void) | null;
  actions?: ReactNode;
  /** P2P 对局：原悔棋/重开按钮位替换为聊天入口（协商动作移入聊天面板）。 */
  chatButton?: ReactNode;
}) {
  const { kind, size, board, toMove, winner, lastMove, hover, onHover, disabled, onPlace } = props;
  /* 底部三卡（2026-09-07 用户拍板）：宽时「规则」「对局」并排；显示不下时两宽卡收起，
     显示第三个组件 .bp-swap——独立完整的一张卡，标题/底色/内容随 bottomTab 真切换，
     默认「对局」。判定用容器查询（跟随 stack 实际宽度，视口宽≠stack 宽） */
  const [bottomTab, setBottomTab] = useState<"game" | "rules">("game");
  const ruleLine = { whiteSpace: "nowrap" } as const;
  const rulesBody = (
    <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 13, lineHeight: 1.6, fontWeight: 600 }}>
      {kind === "gomoku" ? (
        <><li style={ruleLine}>黑先，双方轮流落子。</li><li style={ruleLine}>落子于交叉点，已有棋子处不可落子。</li><li style={ruleLine}>任意一方五子连珠即获胜。</li></>
      ) : (
        <><li style={ruleLine}>黑先，双方轮流落子。</li><li style={ruleLine}>落子于交叉点，同点不可重复落子。</li><li style={ruleLine}>无气的棋子被提掉；禁自杀。</li></>
      )}
    </ul>
  );
  const gameBody = (
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
  );
  return (
    <>
      <div className="brutal-card" style={{ flexShrink: 0, width: "100%", maxWidth: "100%", padding: "12px 14px", display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, flexWrap: "wrap", background: winner ? "var(--bg-2)" : "#fff", color: winner ? "#fff" : "var(--ink)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <span style={{ width: 16, height: 16, borderRadius: 999, background: winner ? (winner === "black" ? "#0A0A0A" : "#fff") : toMove === "black" ? "#0A0A0A" : "#fff", border: winner === "white" ? "2px solid var(--ink)" : winner ? "2px solid #fff" : "2px solid var(--ink)", display: "inline-block", flexShrink: 0 }} />
          <span className="brutal-title" style={{ fontSize: 18, letterSpacing: "0.04em" }}>{props.statusText}</span>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase", opacity: 0.6 }}>
            {props.statusNote}
          </span>
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          {props.chatButton ?? (
            <>
              {props.onUndo && <button className="brutal-btn brutal-btn--sm" onClick={props.onUndo}>悔棋</button>}
              {props.onReset && <button className="brutal-btn brutal-btn--sm brutal-btn--primary" onClick={props.onReset}>重开</button>}
            </>
          )}
          {props.actions}
        </div>
      </div>

      <div className="board-wrap">
        <BoardSvg size={size} board={board} onPlace={onPlace} lastMove={lastMove} hover={hover} onHover={onHover} disabled={disabled} kind={kind} />
      </div>

      {/* 底部（用户拍板 2026-09-07）：宽时「规则/对局」两张原卡并排；显示不下时两卡都收起，
          显示第三个组件 .bp-swap——它是独立完整的一张卡，可在「对局/规则」间真切换
          （标题、底色、内容全套跟随），默认显示对局；规则每条独占一行（ruleLine） */}
      <div style={{ flexShrink: 0, width: "100%", maxWidth: "100%", display: "flex", gap: 12, alignItems: "stretch" }}>
        <div className="brutal-card brutal-card--paper bp-wide" style={{ flex: "1 1 0", minWidth: 0, padding: 14 }}>
          <div className="brutal-label">规则</div>
          {rulesBody}
        </div>
        <div className="brutal-card bp-wide" style={{ flex: "1 1 0", minWidth: 0, padding: 14, background: "#fff" }}>
          <div className="brutal-label">对局</div>
          {gameBody}
        </div>
        <div
          className={`brutal-card bp-swap${bottomTab === "rules" ? " brutal-card--paper" : ""}`}
          style={{ flex: 1, minWidth: 0, padding: 14, background: bottomTab === "game" ? "#fff" : undefined, display: "none", flexDirection: "column" }}
        >
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
            <div className="brutal-label">{bottomTab === "game" ? "对局" : "规则"}</div>
            <button
              className="brutal-btn brutal-btn--sm"
              style={{ padding: "3px 8px", fontSize: 11, lineHeight: 1.2 }}
              onClick={() => setBottomTab((t) => (t === "game" ? "rules" : "game"))}
              title="在对局 / 规则之间切换"
            >
              ⇄ {bottomTab === "game" ? "规则" : "对局"}
            </button>
          </div>
          {bottomTab === "game" ? gameBody : rulesBody}
        </div>
      </div>
    </>
  );
}
