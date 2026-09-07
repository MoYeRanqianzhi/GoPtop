/**
 * App 壳（D1 拆分后）——对局状态机与信令编排全部在 state/useGameSession.tsx，
 * 复用组件在 pages/components.tsx，本地对战页在 pages/LocalPage.tsx，
 * 棋盘规则在 game/board.ts。本文件只做 header、页面拼装与 footer。
 */
import { useEffect, useRef, useState } from "react";
import { useGameSession } from "./state/useGameSession";
import { nav } from "./net/transport";
import type { Size } from "./net/transport";
import { BoardPanel, PeerList, StunSettings } from "./pages/components";
import { LocalPage } from "./pages/LocalPage";
export default function App() {
  const [typeOpen, setTypeOpen] = useState(false);
  const mainRef = useRef<HTMLElement | null>(null);

  /* 等宽同步缩放（Phase 2 设计的实测版）：board-wrap 以 flex 吃掉剩余高度，
     其 clientHeight 就是棋盘可用边长——把它写成 --stack-max 限住整组宽度，
     棋盘（min(100cqw,100cqh)）恰好占满宽度，上下卡片与棋盘永远左右对齐。
     取代旧公式 calc(100dvh - 360px)：那是按当时 chrome 估的死数，卡片加高后失准。 */
  useEffect(() => {
    const main = mainRef.current;
    if (!main) return;
    let last = 0;
    let armed = true;
    let watched: Element | null = null;
    const compute = () => {
      const boardWrap = main.querySelector<HTMLElement>(".board-wrap");
      if (!boardWrap) {
        if (watched) { ro.disconnect(); watched = null; }
        main.style.setProperty("--stack-max", "720px");
        return;
      }
      if (watched !== boardWrap) { ro.disconnect(); ro.observe(main); ro.observe(boardWrap); watched = boardWrap; }
      if (!armed) return;
      armed = false;
      requestAnimationFrame(() => {
        armed = true;
        const next = Math.max(220, Math.floor(boardWrap.clientHeight));
        if (Math.abs(next - last) < 4) return;
        last = next;
        main.style.setProperty("--stack-max", `${next}px`);
      });
    };
    const ro = new ResizeObserver(() => compute());
    const mo = new MutationObserver(() => compute());
    mo.observe(main, { childList: true, subtree: true });
    compute();
    return () => { ro.disconnect(); mo.disconnect(); };
  }, []);

  const {
    kind, size, board, toMove, winner, lastMove, hover, history,
    intent, tabUser, name, peers, role, phase, myColor, peerConnected,
    inviteUrl, watchUrl, notice, answerBackUrl, copyFb,
    modal, modalInput, modalErr,
    setModal, setModalInput, setModalErr, setName, setHover,
    showNotice,
    submitModal, createInvite, acceptInvite, backHome, copyText,
    handlePlace, reset, saveName, pickKind, pickSize,
    moveCount, myHomeUrl, statusText, p2pStatusText, boardDisabled,
    rtcStatus, incomingBanner, mode, viewedUserId, viewedPeer, isSelfPage,
    topLocked, topLockedTitle,
  } = useGameSession();

  return (
    <div style={{ height: "100dvh", minHeight: "100dvh", display: "flex", flexDirection: "column", background: "var(--bg-page)", overflow: "hidden" }}>
      <div className="poster-strip">GoPtop · P2P Gomoku & Go · Neubrutalism · 用户直连 · 无服务器无中转</div>

      <header
        data-kind={kind}
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
          <div className="brutal-title hp-title" style={{ fontSize: 30, lineHeight: 1 }}>GoPtop</div>
          <div
            className="hp-badge"
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

        {/* 对局设置组：大屏直接展开，窄屏收纳进「类型」弹出面板（.hp-setup 隐藏 / .hp-type-btn 显示） */}
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }} onClick={() => setTypeOpen(false)}>
          <div className="hp-setup" style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
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
          </div>
          <div style={{ position: "relative" }} onClick={(e) => e.stopPropagation()}>
            <button className="brutal-btn brutal-btn--sm hp-type-btn" onClick={() => setTypeOpen((o) => !o)} aria-expanded={typeOpen}>
              类型
            </button>
            {typeOpen && (
              <div
                style={{
                  position: "absolute", top: "calc(100% + 6px)", right: 0, zIndex: 900,
                  background: "#fff", border: "3px solid var(--ink)", boxShadow: "4px 4px 0 var(--ink)",
                  padding: 10, display: "flex", flexDirection: "column", gap: 8, minWidth: 200,
                }}
              >
                <div style={{ display: "flex", gap: 6 }}>
                  <button
                    className={`brutal-btn brutal-btn--sm ${kind === "gomoku" ? "brutal-btn--active" : ""}`}
                    onClick={() => { pickKind("gomoku"); setTypeOpen(false); }}
                    aria-pressed={kind === "gomoku"}
                    disabled={topLocked}
                    title={topLockedTitle}
                  >
                    五子棋
                  </button>
                  <button
                    className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
                    onClick={() => { pickKind("go"); setTypeOpen(false); }}
                    aria-pressed={kind === "go"}
                    disabled={topLocked}
                    title={topLockedTitle}
                  >
                    围棋
                  </button>
                </div>
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                  {(kind === "gomoku" ? [15] : [9, 13, 19]).map((s) => (
                    <button
                      key={s}
                      className={`brutal-btn brutal-btn--sm ${size === s ? "brutal-btn--active" : ""}`}
                      onClick={() => { pickSize(s as Size); setTypeOpen(false); }}
                      aria-pressed={size === s}
                      disabled={topLocked}
                      title={topLockedTitle ?? undefined}
                    >
                      {s}×{s}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")} title="回菜单页（本地对战 / P2P 对战 / 在线用户 / 设置）">
            菜单
          </button>
        </div>
      </header>

      <main ref={mainRef} style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: mode === "menu" ? "center" : "flex-start", padding: "clamp(6px, 1.2vh, 12px) 12px clamp(6px, 1vh, 10px)", width: "100%", maxWidth: 760, margin: "0 auto", overflow: "hidden" }}>
        <div className="play-stack">
          {incomingBanner}

          {/* —— 菜单页 `/` —— */}
          {mode === "menu" && (
            <div className="brutal-card" style={{ padding: "16px 14px", background: "#fff", display: "flex", flexDirection: "column", gap: 12 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">开始 · 选一个玩法</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/local")}>本地对战</button>
                <button className="brutal-btn brutal-btn--accent" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/p2p")}>P2P 对战</button>
                <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/users")}>在线用户（{peers.length}）</button>
                <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/settings")}>设置</button>
              </div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(tabUser)}`)}>我的主页</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </div>
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
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
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={createInvite}>开启对战（等对手）</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-invite"); }}>粘贴邀请链接</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </div>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap", marginTop: 4 }}>
                <span className="brutal-label">在线用户（{peers.length}）</span>
              </div>
              <PeerList
                peers={peers}
                emptyHint="暂无其他在线用户。"
                actionLabel={() => "挑战"}
                onAction={(p) => acceptInvite(p.id, null, kind, size)}
                extraAction={(p) => (
                  <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(p.id)}`)}>主页</button>
                )}
              />
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
            </div>
          )}

          {mode === "p2p" && phase === "waiting" && (
            <>
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fffbeb", display: "flex", flexDirection: "column", gap: 8 }}>
              {role === "inviter" && inviteUrl ? (
                <>
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <code style={{ flex: 1, minWidth: 0, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{inviteUrl}</code>
                    <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制</button>
                    <button className="brutal-btn brutal-btn--sm brutal-btn--accent" style={{ flexShrink: 0 }} onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-answer"); }}>回执</button>
                    <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={backHome}>取消</button>
                    {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                  </div>
                </>
              ) : role === "inviter" ? (
                /* offer 生成中/失败：不给链接可复制（防发出无 rtc 的废链接，审查 A5） */
                <>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>{notice ?? "正在生成直连邀请…"}</div>
                  <div style={{ display: "flex", gap: 8 }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消</button>
                  </div>
                </>
              ) : (
                <>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>{notice ?? "等待对方确认…"}</div>
                  {answerBackUrl && (
                    /* 误关回执弹窗后可从这里重新查看（审查 A6） */
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                      <button className="brutal-btn brutal-btn--sm" onClick={() => setModal("receipt")}>查看回执</button>
                    </div>
                  )}
                  <div style={{ display: "flex", gap: 8 }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消</button>
                  </div>
                </>
              )}
            </div>
            {(role === "inviter" || role === "invitee") && (
              <BoardPanel
                kind={kind} size={size} board={board} toMove={toMove} winner={winner}
                lastMove={lastMove} hover={hover} onHover={setHover}
                disabled onPlace={() => undefined}
                statusText="等待对手加入…" statusNote={`${myColor === "black" ? "执黑" : "执白"}`}
                moveCount={moveCount} history={history}
                onUndo={null} onReset={() => undefined}
              />
            )}
            </>
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
                statusText={statusText} statusNote={`${myColor === "black" ? "执黑" : "执白"}`}
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
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回菜单页</button>
              </div>
              {phase !== "home" && (
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
                  你当前{phase === "waiting" ? "正在等待对手" : "正在对局中"}。
                  <button className="brutal-btn brutal-btn--sm" style={{ marginLeft: 8 }} onClick={() => nav("/p2p")}>前往 P2P 页</button>
                </div>
              )}
              <PeerList
                peers={peers}
                emptyHint="暂无其他在线用户。"
                actionLabel={() => "挑战"}
                onAction={(p) => { if (phase === "home") acceptInvite(p.id, null, kind, size); }}
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
                <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回菜单页</button>
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
                <div className="brutal-label" style={{ marginBottom: 6 }}>默认规则与尺寸（新对局/开页时使用）</div>
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
                  当前：{kind === "gomoku" ? "五子棋" : "围棋"} {size}×{size}
                  {phase !== "home" && "（对局/等待中，顶部已锁定）"}
                </div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginTop: 6 }}>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => {
                    try { localStorage.setItem("goptop:defaults", JSON.stringify({ kind, size })); } catch { /* ignore */ }
                    showNotice("已保存当前顶部选择为默认值", 1600);
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
            </div>
          )}

          {/* —— 用户主页 `/<userId>` —— */}
          {mode === "user" && viewedUserId && (
            <>
            <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
              {isSelfPage ? (
                <>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                    <span className="brutal-label">我的主页</span>
                    <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
                  </div>
                  {phase === "waiting" && role === "inviter" && inviteUrl ? (
                    <>
                      <span className="brutal-label">等待对手 · 邀请</span>
                      <code style={{ border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{inviteUrl}</code>
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制邀请链接</button>
                        <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/p2p")}>前往 P2P 页</button>
                        <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消等待</button>
                      </div>
                    </>
                  ) : phase === "playing" ? (
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>对局中。</span>
                      <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => nav("/p2p")}>回到对局</button>
                      {watchUrl && <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(watchUrl, "观战链接已复制")}>复制观战链接</button>}
                    </div>
                  ) : (
                    <>
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={createInvite}>开启对战（等对手）</button>
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
                        onClick={() => acceptInvite(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null)}
                        disabled={viewedPeer.status === "in-game"}
                        title={viewedPeer.status === "in-game" ? "对方对局中，不可挑战" : "向其发起对局"}>
                        {intent.mode === "user" && intent.pwd ? "接受邀请进入对局" : "挑战"}
                      </button>
                    </div>
                  ) : (
                    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                      <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                        对方可能已离线。仍可尝试发起挑战。
                      </div>
                      <div>
                        <button className="brutal-btn brutal-btn--sm brutal-btn--accent"
                          onClick={() => acceptInvite(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null)}>
                          {intent.mode === "user" && intent.pwd ? "接受邀请进入对局" : "挑战"}
                        </button>
                      </div>
                    </div>
                  )}
                </>
              )}
              {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{notice}</div>}
              {copyFb && <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</div>}
            </div>
            {(phase === "waiting" || phase === "playing") && (role === "inviter" || role === "invitee") && (
              <BoardPanel
                kind={kind} size={size} board={board} toMove={toMove} winner={winner}
                lastMove={lastMove} hover={hover} onHover={setHover}
                disabled={phase === "waiting" ? true : boardDisabled} onPlace={phase === "waiting" ? () => undefined : handlePlace}
                statusText={phase === "waiting" ? "等待对手加入…" : statusText}
                statusNote={`${myColor === "black" ? "执黑" : "执白"}`}
                moveCount={moveCount} history={history}
                onUndo={null} onReset={phase === "waiting" ? () => undefined : reset}
              />
            )}
            </>
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
        /* bottom-grid 列数已由 minmax(auto-fit) 自适应，窄屏自动单列，旧 640px 断点删除 */
        /* header 三级降级（手机端致命挤压）：先隐徽章，再隐标题，最后把对局设置收进「类型」按钮。
           阈值按模式分测（含 padding，2026-09-07 实测）：五子棋全量 578 / 无徽章 442；
           围棋（3 个尺寸钮）全量 677 / 无徽章 572。各 +2px 余量——宁可挤一点也不提前隐藏 */
        @media (max-width: 580px) { header[data-kind="gomoku"] .hp-badge { display: none; } }
        @media (max-width: 679px) { header[data-kind="go"] .hp-badge { display: none; } }
        @media (max-width: 444px) { header[data-kind="gomoku"] .hp-title { display: none; } }
        @media (max-width: 574px) { header[data-kind="go"] .hp-title { display: none; } }
        @media (max-width: 463px) {
          header[data-kind="go"] .hp-setup { display: none !important; }
          header[data-kind="go"] .hp-type-btn { display: inline-block; }
        }
        @media (max-width: 342px) {
          .hp-setup { display: none !important; }
          .hp-type-btn { display: inline-block; }
          header { padding: 8px 10px !important; }
        }
        /* 底部三卡切换（用户拍板 2026-09-07）：判定用容器查询——.play-stack 是容器
           （brutal.css），stack 宽度跟随棋盘可用高度缩放，视口宽时 stack 也可能很窄，
           媒体查询看不到这种「显示不下」。阈值以下宽卡放不下 → 收起显示 swap 卡 */
        .bp-swap { display: none; }
        @container (max-width: 520px) {
          .bp-wide { display: none !important; }
          .bp-swap { display: flex !important; }
        }
        @container (max-width: 300px) {
          .bp-swap { padding: 10px !important; }
        }
      `}</style>

      {/* —— 弹窗（信令消息统一经弹窗收发：粘贴邀请 / 输入回执 / 展示回执） —— */}
      {modal && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 1000, background: "rgba(10,10,10,0.55)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}
          onClick={() => setModal(null)}
        >
          <div
            className="brutal-card"
            style={{ width: "min(560px, 92vw)", maxHeight: "80vh", overflow: "auto", background: "#fff", padding: 16, display: "flex", flexDirection: "column", gap: 10 }}
            onClick={(e) => e.stopPropagation()}
          >
            {modal === "receipt" ? (
              <>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                  <span className="brutal-label">回执已生成 · 发给邀请者</span>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => setModal(null)}>关闭</button>
                </div>
                <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 600, lineHeight: 1.5 }}>
                  把下面的回执链接发给邀请者即可开局。
                </div>
                <textarea
                  value={answerBackUrl ?? ""}
                  readOnly
                  rows={3}
                  onFocus={(e) => e.currentTarget.select()}
                  style={{ border: "3px solid var(--ink)", padding: "8px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fffbeb", overflowWrap: "anywhere", resize: "vertical" }}
                />
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => answerBackUrl && copyText(answerBackUrl, "回执链接已复制")}>复制回执链接</button>
                  {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                </div>
              </>
            ) : (
              <>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                  <span className="brutal-label">
                    {modal === "paste-invite" ? "粘贴邀请链接（对方发来的）"
                      : "输入回执（受邀者发来的回执链接）"}
                  </span>
                  <button className="brutal-btn brutal-btn--sm" onClick={() => setModal(null)}>关闭</button>
                </div>
                <input
                  autoFocus
                  value={modalInput}
                  onChange={(e) => setModalInput(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") submitModal(); }}
                  placeholder={modal === "paste-invite" ? "粘贴邀请链接或主页链接"
                    : "粘贴受邀者发来的回执链接"}
                  style={{ border: "3px solid var(--ink)", padding: "9px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }}
                />
                {modalErr && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#b00020" }}>{modalErr}</div>}
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={submitModal}>
                    {modal === "paste-answer" ? "确认回执" : "连接"}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}

      <footer style={{ flexShrink: 0, padding: "10px 16px", borderTop: "3px solid var(--ink)", background: "#fff", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.06em", textTransform: "uppercase", color: "var(--muted)", textAlign: "center" }}>
        GoPtop · P2P Gomoku & Go · 用户直连 · 无服务器无中转
      </footer>
    </div>
  );
}
