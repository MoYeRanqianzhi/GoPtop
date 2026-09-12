/**
 * 用户主页 `/<userId>`（原 App.tsx JSX 原样搬出，禁止行为变化）：
 * isSelfPage 两分支（自己的主页：头像/昵称/等待与对局状态；他人主页：资料卡+挑战），
 * 等待/对局中附 BoardPanel。
 */
import { nav } from "../net/links";
import type { GameSession } from "../state/useGameSession";
import { AvatarSettings, BoardPanel } from "./components";
import { NoticeLine } from "../components/NoticeLine";

export function UserPage(props: { s: GameSession }) {
  const {
    intent, tabUser, name, kind, size, board, toMove, winner, lastMove, hover, history,
    viewedUserId, viewedPeer, isSelfPage, role, phase, myColor,
    inviteUrl, watchUrl, notice, copyFb, myHomeUrl,
    serverMode, statusText, boardDisabled, moveCount,
    setName, setHover,
    acceptInvite, serverChallengePeer, createInvite, backHome, copyText,
    reset, handlePlace, saveName, loadMyAvatar, saveMyAvatar,
  } = props.s;
  const myAvatar = loadMyAvatar();
  return viewedUserId ? (
    <>
    <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
      {isSelfPage ? (
        <>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
            <span className="brutal-label">我的主页</span>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <div className="brutal-label">头像与昵称（对局/聊天中展示）</div>
            <AvatarSettings dataUrl={myAvatar} onSave={saveMyAvatar} />
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
              <input placeholder="给自己起个昵称" value={name} onChange={(e) => setName(e.target.value)}
                style={{ flex: "1 1 160px", minWidth: 140, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }} />
              <button className="brutal-btn brutal-btn--sm" onClick={saveName}>保存昵称</button>
            </div>
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
                onClick={() => {
                  // 服务器模式跨设备走 presence 的 acceptInvite 不可达（TODO 2026-09-13
                  // 记录的漏改入口）：无 rtc 的挑战/无钥匙请求改发服务器挑战信；
                  // 带 rtc 的旧链接仍走回执兼容路径
                  if (serverMode && !(intent.mode === "user" && intent.rtc)) serverChallengePeer(viewedUserId);
                  else acceptInvite(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null);
                }}
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
                  onClick={() => {
                    // 同上：服务器模式无 rtc 时改发服务器挑战信（对方离线由超时提示兜底）
                    if (serverMode && !(intent.mode === "user" && intent.rtc)) serverChallengePeer(viewedUserId);
                    else acceptInvite(viewedUserId, intent.mode === "user" ? intent.pwd : null, kind, size, intent.mode === "user" ? intent.rtc : null);
                  }}>
                  {intent.mode === "user" && intent.pwd ? "接受邀请进入对局" : "挑战"}
                </button>
              </div>
            </div>
          )}
        </>
      )}
      <NoticeLine text={notice} />
      {copyFb && <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</div>}
    </div>
    {(phase === "waiting" || phase === "playing") && (role === "inviter" || role === "invitee") && (
      <BoardPanel
        kind={kind} size={size} board={board} toMove={toMove} winner={winner}
        lastMove={lastMove} hover={hover} onHover={setHover}
        disabled={phase === "waiting" ? true : boardDisabled} onPlace={phase === "waiting" ? () => undefined : handlePlace}
        statusText={phase === "waiting" ? "等待对手加入…" : statusText}
        statusNote={phase === "waiting" ? "" : `${myColor === "black" ? "执黑" : "执白"}`}
        moveCount={moveCount} history={history}
        onUndo={null} onReset={phase === "waiting" ? null : reset}
      />
    )}
    </>
  ) : null;
}
