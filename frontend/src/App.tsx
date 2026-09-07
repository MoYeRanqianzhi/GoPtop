/**
 * App 壳（D1 拆分后）——对局状态机与信令编排全部在 state/useGameSession.ts，
 * 复用组件在 pages/components.tsx，本地对战页在 pages/LocalPage.tsx，
 * 棋盘规则在 game/board.ts。本文件只做 header、页面拼装与 footer。
 */
import { useGameSession } from "./state/useGameSession";
import { nav } from "./net/transport";
import type { Size } from "./net/transport";
import { BoardPanel, PeerList, StunSettings } from "./pages/components";
import { LocalPage } from "./pages/LocalPage";
export default function App() {
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
          {/* 回执入口常驻（用户拍板）：任何页面都能粘贴收到的回执/邀请链接 */}
          <button className="brutal-btn brutal-btn--sm" onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-answer"); }} title="粘贴对方发来的回执链接（自动识别加入对局或观战）">
            输入回执
          </button>
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")} title="回选项页（本地对战 / P2P 对战 / 在线用户 / 设置）">
            选项
          </button>
        </div>
      </header>

      <main style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: mode === "menu" ? "center" : "flex-start", padding: "clamp(6px, 1.2vh, 12px) 12px clamp(6px, 1vh, 10px)", width: "100%", maxWidth: 760, margin: "0 auto", overflow: "hidden" }}>
        <div className="play-stack">
          {incomingBanner}

          {/* —— 选项页 `/` —— */}
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
                <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={createInvite}>开启对战（等对手）</button>
                <button className="brutal-btn brutal-btn--sm" onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-invite"); }}>粘贴邀请链接</button>
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
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
                <span className="brutal-label">等待对手 · 邀请（本局有效）</span>
                <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{p2pStatusText}</span>
              </div>
              {role === "inviter" && inviteUrl ? (
                <>
                  <code style={{ border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{inviteUrl}</code>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制邀请链接</button>
                    <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-answer"); }}>输入回执</button>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消等待</button>
                    {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
                  </div>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                    对方只需打开此链接即自动加入并建立直连（pwd 为本局钥匙，每局轮换；直连信息已编进链接）。
                    跨设备：对方打开链接后会弹出回执链接发给你，点「输入回执」粘贴即可开局。
                    两人进对局后 pwd 失效，不可再加入第三人；此邀请区将隐藏。
                  </div>
                  {rtcStatus}
                </>
              ) : role === "inviter" ? (
                /* offer 生成中/失败：不给链接可复制（防发出无 rtc 的废链接，审查 A5） */
                <>
                  <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>{notice ?? "正在生成直连邀请…"}</div>
                  <div style={{ display: "flex", gap: 8 }}>
                    <button className="brutal-btn brutal-btn--sm" onClick={backHome}>取消</button>
                  </div>
                  {rtcStatus}
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
                statusText="等待对手加入…" statusNote={`联机 · 你是${myColor === "black" ? "黑" : "白"}`}
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
              <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
                直连约束：同源页面间走 BroadcastChannel 同源直传；跨设备走 WebRTC DataChannel（仅上面启用的 STUN、无 TURN 中转），信令随邀请链接自动走，无信令服务器、无手动输入。
              </div>
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
                        在在线用户列表中暂未发现该用户（对方可能已离线）。仍可尝试发起挑战。
                      </div>
                      <div>
                        <button className="brutal-btn brutal-btn--sm brutal-btn--accent"
                          onClick={() => acceptInvite(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null)}>
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
            {(phase === "waiting" || phase === "playing") && (role === "inviter" || role === "invitee") && (
              <BoardPanel
                kind={kind} size={size} board={board} toMove={toMove} winner={winner}
                lastMove={lastMove} hover={hover} onHover={setHover}
                disabled={phase === "waiting" ? true : boardDisabled} onPlace={phase === "waiting" ? () => undefined : handlePlace}
                statusText={phase === "waiting" ? "等待对手加入…" : statusText}
                statusNote={phase === "waiting" ? `联机 · 你是${myColor === "black" ? "黑" : "白"}` : `联机 · ${myColor === "black" ? "你执黑" : "你执白"}`}
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
        @media (max-width: 640px) {
          .bottom-grid { grid-template-columns: 1fr !important; }
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
                  把下面的回执链接发给邀请者；邀请者在「等待对手」页点「输入回执」粘贴即可开局。
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
                  placeholder={modal === "paste-invite" ? "粘贴邀请链接或主页链接（任意域名均可识别）"
                    : "粘贴受邀者发来的回执链接（任意域名均可识别）"}
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
