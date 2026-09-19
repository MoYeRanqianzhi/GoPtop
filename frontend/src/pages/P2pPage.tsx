/**
 * P2P 对战页 `/p2p`（D1 拆分自 App.tsx；此后已在其上补齐围棋停一手/终局计分、聊天停靠栏）：
 * home 大厅 / waiting 等待卡+棋盘 / playing 对局卡+棋盘。
 * 服务器模式观战链接 specUrl 原在 App 计算，只在本页使用，随之归属本页。
 */
import { nav, specLinkUrl } from "../net/links";
import type { GameSession } from "../state/useGameSession";
import { BoardPanel, PeerList } from "./components";
import { NoticeLine } from "../components/NoticeLine";
import { StatusLamp } from "../components/StatusLamp";
import { UrlRow } from "../components/UrlRow";
import { MessageCircle } from "lucide-react";
import { useWinRate } from "../ai/useWinRate";

export function P2pPage(props: { s: GameSession; toggleChat: () => void }) {
  const { s, toggleChat } = props;
  const {
    kind, size, board, toMove, winner, lastMove, hover,
    tabUser, peers, role, phase, myColor,
    inviteUrl, watchUrl, notice, answerBackUrl, copyFb, myHomeUrl,
    serverMode, serverState, spectateEnabled, specPwd,
    linkLamp, statusText, boardDisabled, moveCount,
    scoring, myScoreOk, peerScoreOk, scoreResult, myDead, peerDead,
    setModal, setModalInput, setModalErr, setHover,
    createInvite, acceptInvite, backHome, copyText, handlePlace, serverChallengePeer,
    handlePass, toggleDead, confirmScore,
  } = s;

  // —— 围棋终局计分（2026-09-18 实机测试发现页面层未接线：引擎/状态机/wasm 早已就绪，
  //    但没有任何组件渲染停一手与计分入口，用户实际走不到这两个功能）——
  const scoringActive = kind === "go" && scoring && !winner;
  const deadMarked = myDead.length + peerDead.length;

  // 观战链接：Rust 状态机权威生成（服务器模式 spec 链接；无服务器含 specrtc 直连参数）。
  // 快照无值时回退本地拼接（兼容无服务器流程前的展示）。
  const specUrl = s.specUrl ?? (serverMode && spectateEnabled && specPwd && (phase === "playing" || phase === "waiting") && role !== "spectator"
    ? specLinkUrl(tabUser, specPwd)
    : null);

  /* 胜率（用户拍板：全部模式）。局面取自 WasmSession 而非本地规则引擎——P2P 的对局
     归 Rust 状态机所有。观战者没有「我方」颜色（myColor 为空），与本地双人同款按
     黑方视角处理，标签也照实写「黑/白」而不是「我/对手」。 */
  const odds = useWinRate({
    getState: () => s.stateJson(),
    myColor: myColor === "white" ? "White" : "Black",
    moveCount,
    budgetMs: 500,
    enabled: phase === "playing" && !winner,
  });

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

          {/* 围棋终局计分：**不新增卡片**，控件并入下面 BoardPanel 的状态行。
              此前完全没有 UI——双 Pass 后玩家只能看到「黑/白 落子」的普通对局态，
              既标不了死子也确认不了，终局卡死（实机测试发现）。
              为什么不单独做一张卡：--stack-max（整组宽度锚点）= main 剩余高度 − 非棋盘卡片总高
              （App.tsx 一律在最宽态量），卡片高度直接从这份剩余高度里扣——多一张卡等于从棋盘
              高度里拿走一整张卡的高度（桌面壳 1100×760 实测棋盘掉到 92px 级别，点不准棋子）。
              （正反馈塌陷在 569f155 已由「最宽态测量」修掉，不再是本取舍的理由。） */}
          <BoardPanel
            kind={kind} size={size} board={board} toMove={toMove} winner={winner}
            lastMove={lastMove} hover={hover} onHover={setHover}
            /* 计分阶段：点棋子=标死子，且不受轮次限制（双方都要能标） */
            disabled={scoringActive ? false : boardDisabled}
            onPlace={scoringActive ? (c) => toggleDead(c) : handlePlace}
            /* 上屏用并集：双方各自标的死子都画出来供对照；计分只取交集（Rust 侧 my_dead ∩ peer_dead） */
            dead={[...myDead, ...peerDead]}
            allowOccupied={scoringActive}
            statusText={scoringActive ? "终局计分" : statusText}
            statusNote={scoringActive ? "点击棋子标记死子" : `${myColor === "black" ? "执黑" : "执白"}`}
            moveCount={moveCount}
            onUndo={null} onReset={null}
            odds={{
              winRate: odds.winRate,
              series: odds.series,
              thinking: odds.thinking,
              myLabel: myColor ? "我" : "黑",
              oppLabel: myColor ? "对手" : "白",
            }}
            chatButton={
              /* 开关同一个按钮：开着再点即收起（停靠栏形态下面板自身也有「收起」，
                 两者等价——实机用户就是点这个按钮发现关不掉的） */
              <button
                className="brutal-btn brutal-btn--sm"
                onClick={toggleChat}
                title="聊天 / 悔棋 / 重开 / 换棋 / 认输"
                aria-label="聊天与对局操作"
              >
                <MessageCircle size={15} strokeWidth={2.5} style={{ display: "block" }} />
              </button>
            }
            actions={
              <>
                {scoringActive && (
                  <>
                    <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, flexShrink: 0 }}>
                      已标 {deadMarked} 子　我方{myScoreOk ? "✓" : "待"} 对方{peerScoreOk ? "✓" : "待"}
                    </span>
                    {scoreResult ? (
                      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>
                        黑 {scoreResult.black} : 白 {scoreResult.white} → {scoreResult.winner === "black" ? "黑" : "白"} 胜
                      </span>
                    ) : (
                      <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={confirmScore} disabled={myScoreOk}>
                        {myScoreOk ? "已确认，等对方" : "确认计分"}
                      </button>
                    )}
                  </>
                )}
                {/* 围棋：停一手（双 Pass 触发终局计分）；计分阶段不再提供 */}
                {kind === "go" && !winner && !scoring && toMove === myColor && (
                  <button className="brutal-btn brutal-btn--sm" onClick={handlePass} title="停一手（双方连续停一手进入终局计分）">停一手</button>
                )}
                {/* 认输已移入聊天区（对局操作集合区）——棋盘旁的操作行只留
                    棋盘/终局直接相关的东西，见 pages/components.tsx ChatPanel */}
                {specUrl && <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(specUrl, "观战链接已复制")}>复制观战链接</button>}
              </>
            }
          />
        </>
      )}
    </>
  );
}
