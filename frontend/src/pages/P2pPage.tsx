/**
 * P2P 对战页 `/p2p`（原 App.tsx 三段 JSX 原样搬出，禁止行为变化）：
 * home 大厅 / waiting 等待卡+棋盘 / playing 对局卡+棋盘。
 * 服务器模式观战链接 specUrl 原在 App 计算，只在本页使用，随之归属本页。
 */
import { nav, specLinkUrl } from "../net/links";
import type { GameSession } from "../state/useGameSession";
import { BoardPanel, PeerList } from "./components";
import { NoticeLine } from "../components/NoticeLine";
import { StatusLamp } from "../components/StatusLamp";
import { UrlRow } from "../components/UrlRow";

export function P2pPage(props: { s: GameSession; setChatOpen: (open: boolean) => void }) {
  const { s, setChatOpen } = props;
  const {
    kind, size, board, toMove, winner, lastMove, hover,
    tabUser, peers, role, phase, myColor,
    inviteUrl, watchUrl, notice, answerBackUrl, copyFb, myHomeUrl,
    serverMode, serverState, spectateEnabled, specPwd,
    linkLamp, statusText, boardDisabled, moveCount,
    setModal, setModalInput, setModalErr, setHover,
    createInvite, acceptInvite, backHome, copyText, handlePlace, serverChallengePeer,
  } = s;

  // 观战链接：Rust 状态机权威生成（服务器模式 spec 链接；无服务器含 specrtc 直连参数）。
  // 快照无值时回退本地拼接（兼容无服务器流程前的展示）。
  const specUrl = s.specUrl ?? (serverMode && spectateEnabled && specPwd && (phase === "playing" || phase === "waiting") && role !== "spectator"
    ? specLinkUrl(tabUser, specPwd)
    : null);

  return (
    <>
      {phase === "home" && (
        <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
            <span className="brutal-label">P2P 对战大厅</span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>
              {serverMode ? (serverState === "ready" ? "服务器已连接" : serverState === "connecting" ? "服务器连接中…" : "服务器未连接") : tabUser}
            </span>
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={createInvite}>开启对战（等对手）</button>
            <button className="brutal-btn brutal-btn--sm" onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-invite"); }}>粘贴邀请链接</button>
            {!serverMode && <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>}
            {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
          </div>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap", marginTop: 4 }}>
            <span className="brutal-label">在线用户（{peers.length}）</span>
          </div>
          <PeerList
            peers={peers}
            emptyHint={serverMode ? "暂无其他在线用户（同服务器的用户会出现在这里）。" : "暂无其他在线用户。"}
            actionLabel={() => "挑战"}
            onAction={(p) => (serverMode ? serverChallengePeer(p.id) : acceptInvite(p.id, null, kind, size))}
            extraAction={serverMode ? undefined : (p) => (
              <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(p.id)}`)}>主页</button>
            )}
          />
          <NoticeLine text={notice} />
        </div>
      )}

      {phase === "waiting" && (
        <>
        <div className="brutal-card" style={{ padding: "10px 12px", background: "#fffbeb", display: "flex", flexDirection: "column", gap: 8 }}>
          <StatusLamp color={linkLamp.color} text={linkLamp.text} />
          {role === "inviter" && inviteUrl ? (
            <>
              <UrlRow url={inviteUrl}>
                <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={() => copyText(inviteUrl, "邀请链接已复制")}>复制</button>
                {/* 服务器模式信令经服务器直达（免回执）；回执仅无服务器跨设备需要 */}
                {!serverMode && (
                  <button className="brutal-btn brutal-btn--sm brutal-btn--accent" style={{ flexShrink: 0 }} onClick={() => { setModalInput(""); setModalErr(null); setModal("paste-answer"); }}>回执</button>
                )}
                <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={backHome}>取消</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </UrlRow>
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
            statusText="等待对手加入…" statusNote=""
            moveCount={moveCount}
            onUndo={null} onReset={null}
          />
        )}
        </>
      )}

      {phase === "playing" && (
        <>
          <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
            <StatusLamp color={linkLamp.color} text={linkLamp.text} />
            {/* 对局中的提示面（悔棋/重开/换棋结果、观战批复）：卡片瘦身时曾随
                「对局·直连」行一并消失，对局内反馈无处显示——恢复为条件渲染单行 */}
            <NoticeLine text={notice} />
            {(watchUrl || specUrl) && role !== "spectator" && (
              <UrlRow url={specUrl ?? watchUrl}>
                <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={() => copyText(specUrl ?? watchUrl ?? "", "观战链接已复制")}>邀请观战</button>
                <button className="brutal-btn brutal-btn--sm" style={{ flexShrink: 0 }} onClick={backHome}>离开</button>
                {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
              </UrlRow>
            )}
          </div>
          <BoardPanel
            kind={kind} size={size} board={board} toMove={toMove} winner={winner}
            lastMove={lastMove} hover={hover} onHover={setHover}
            disabled={boardDisabled} onPlace={handlePlace}
            statusText={statusText} statusNote={`${myColor === "black" ? "执黑" : "执白"}`}
            moveCount={moveCount}
            onUndo={null} onReset={null}
            chatButton={
              <button className="brutal-btn brutal-btn--sm" onClick={() => setChatOpen(true)} title="聊天 / 悔棋 / 重开 / 换棋">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" style={{ display: "block" }}>
                  <path d="M21 12a8 8 0 0 1-8 8H4l2.4-3A8 8 0 1 1 21 12z" strokeLinejoin="round" />
                  <circle cx="9" cy="12" r="0.6" fill="currentColor" /><circle cx="13" cy="12" r="0.6" fill="currentColor" /><circle cx="17" cy="12" r="0.6" fill="currentColor" />
                </svg>
              </button>
            }
            actions={specUrl && <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(specUrl, "观战链接已复制")}>复制观战链接</button>}
          />
        </>
      )}
    </>
  );
}
