/**
 * App 壳（Phase 4 页面拆分后）——对局状态机与信令编排全部在 state/useGameSession.tsx，
 * 页面在 pages/*（Menu/Local/P2p/Users/Settings/User/Watch），跨页复用组件在
 * pages/components.tsx，共用小组件（指示灯/URL 行/棋种选择器/弹窗横幅）在 components/，
 * 棋盘规则在 game/board.ts。本文件只做 --stack-max 同步、header（棋种/尺寸选择器 +
 * 菜单按钮）、页面拼装、chatDock 组装、弹窗/横幅挂载与 footer。
 * 页面壳布局样式（header 三级降级、底部三卡容器查询）在 styles/brutal.css 末尾。
 */
import { useEffect, useRef, useState } from "react";
import { useGameSession } from "./state/useGameSession";
import { nav } from "./net/links";
import { ChatPanel } from "./pages/components";
import { LocalPage } from "./pages/LocalPage";
import { MenuPage } from "./pages/MenuPage";
import { P2pPage } from "./pages/P2pPage";
import { UsersPage } from "./pages/UsersPage";
import { SettingsPage } from "./pages/SettingsPage";
import { UserPage } from "./pages/UserPage";
import { WatchPage } from "./pages/WatchPage";
import { ConfirmBanner } from "./components/ConfirmBanner";
import { InviteModal } from "./components/InviteModal";
import { KindSizePicker } from "./components/KindSizePicker";
import { PasteModal } from "./components/PasteModal";
import { PosterStrip } from "./components/PosterStrip";

export default function App() {
  // 聊天面板开合：宽屏=右侧停靠栏，窄屏=弹窗（CSS .chat-dock/.chat-modal 控制形态）
  const [chatOpen, setChatOpen] = useState(false);
  const mainRef = useRef<HTMLElement | null>(null);
  const s = useGameSession();
  const {
    kind, size, mode, phase, role, chatLog, peerAvatars, spectators, specRequests,
    specCanChat, spectateEnabled, confirmReq, serverIncoming, incoming,
    modal, modalInput, modalErr, answerBackUrl, copyFb,
    setModal, setModalInput, setModalErr, pickKind, pickSize,
    submitModal, acceptChallenge, rejectChallenge,
    serverAcceptChallenge, serverRejectChallenge,
    sendChat, requestUndo, requestReset, requestSwap,
    approveSpecRequest, rejectSpecRequest, kickSpectator, muteSpectator,
    disableSpectate, requestSpecChat, confirmApprove, confirmDecline,
    topLocked, topLockedTitle,
  } = s;

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

  // 对局邀请统一弹窗（用户拍板 2026-09-13：横幅不够明显）。服务器挑战（serverIncoming）
  // 与同源 presence 挑战（incoming）同一形态；只在主页状态出现，必须明确同意/拒绝，
  // 不设背景点击关闭——静默忽略会让挑战方停在「等待对方同意」。
  const inviteReq = serverIncoming
    ? {
        fromName: serverIncoming.fromName,
        desc: `${serverIncoming.kind === "gomoku" ? "五子棋" : "围棋"} ${serverIncoming.size}×${serverIncoming.size}`,
        accept: serverAcceptChallenge,
        reject: serverRejectChallenge,
      }
    : incoming
      ? {
          fromName: incoming.fromName,
          desc: `${incoming.kind === "gomoku" ? "五子棋" : "围棋"} ${incoming.size}×${incoming.size}`,
          accept: acceptChallenge,
          reject: rejectChallenge,
        }
      : null;

  // 窄屏下聊天弹窗（z-index 800）会整个盖住协商横幅（ConfirmBanner 在常规流里，
  // 静态 z-index 不生效）——请求到了用户根本看不见，对方只能一直等（实机测试发现：
  // 移动端开着聊天时悔棋/换棋/重开请求全部不可达）。有待决请求就收起聊天弹窗，
  // 横幅随即可见；处理完随时可再打开聊天。
  useEffect(() => {
    if (confirmReq && chatOpen) setChatOpen(false);
  }, [confirmReq, chatOpen]);

  // 聊天面板（宽屏右侧停靠；窄屏弹窗，形态由 CSS 控制）
  const chatPanel = (
    <ChatPanel
      role={role}
      chatLog={chatLog}
      peerAvatars={peerAvatars}
      spectators={spectators}
      specRequests={specRequests}
      specCanChat={specCanChat}
      spectateEnabled={spectateEnabled}
      onSend={sendChat}
      onUndo={requestUndo}
      onReset={requestReset}
      onSwap={requestSwap}
      onKick={kickSpectator}
      onMute={muteSpectator}
      onDisableSpectate={disableSpectate}
      onRequestSpecChat={requestSpecChat}
      onApproveSpec={approveSpecRequest}
      onRejectSpec={rejectSpecRequest}
    />
  );
  const chatDockOpen = chatOpen && mode === "p2p" && phase === "playing";
  const chatDock = chatDockOpen && (
    <>
      <aside className="brutal-card chat-dock chat-dock--open" style={{ padding: 12 }}>
        {chatPanel}
      </aside>
      {/* 窄屏：同内容以弹窗覆盖 */}
      {chatOpen && mode === "p2p" && phase === "playing" && (
        <div className="chat-modal-bg" onClick={() => setChatOpen(false)}>
          <div className="brutal-card chat-modal" onClick={(e) => e.stopPropagation()} style={{ padding: 12 }}>
            {chatPanel}
          </div>
        </div>
      )}
    </>
  );

  return (
    <div style={{ height: "100dvh", minHeight: "100dvh", display: "flex", flexDirection: "column", background: "var(--bg-page)", overflow: "hidden", paddingTop: "env(safe-area-inset-top, 0px)" }}>
      <PosterStrip />

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

        <KindSizePicker
          kind={kind}
          size={size}
          onPickKind={pickKind}
          onPickSize={pickSize}
          locked={topLocked}
          lockedTitle={topLockedTitle}
        >
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")} title="回菜单页（本地对战 / P2P 对战 / 在线用户 / 设置）">
            菜单
          </button>
        </KindSizePicker>
      </header>

      <main ref={mainRef} style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: mode === "menu" ? "center" : "flex-start", padding: "clamp(6px, 1.2vh, 12px) 12px clamp(6px, 1vh, 10px)", width: "100%", maxWidth: "none", margin: "0 auto", overflow: "hidden" }}>
        <div className="game-layout">
        <ConfirmBanner req={confirmReq} onApprove={confirmApprove} onDecline={confirmDecline} />
        <div className="play-stack">

          {/* —— 菜单页 `/` —— */}
          {mode === "menu" && <MenuPage s={s} />}

          {/* —— 本地对战 `/local`：与 P2P 同一套棋盘 UI，只是没有邀请链接 —— */}
          {mode === "local" && <LocalPage kind={kind} size={size} />}

          {/* —— P2P 对战 `/p2p`：大厅 / 等待 / 对局三段 —— */}
          {mode === "p2p" && <P2pPage s={s} setChatOpen={setChatOpen} />}

          {/* —— 在线用户 `/users` —— */}
          {mode === "users" && <UsersPage s={s} />}

          {/* —— 设置 `/settings` —— */}
          {mode === "settings" && <SettingsPage s={s} />}

          {/* —— 用户主页 `/<userId>` —— */}
          {mode === "user" && <UserPage s={s} />}

          {/* —— 观战 `/watch/<game>` —— */}
          {mode === "watch" && <WatchPage s={s} />}
        </div>
        {chatDock}
        </div>
      </main>

      {/* 页面壳布局样式（header 三级降级、底部三卡容器查询）在 styles/brutal.css 末尾 */}

      {/* —— 对局邀请弹窗（服务器挑战 / 同源挑战统一；必须明确选择） —— */}
      {inviteReq && (
        <InviteModal req={inviteReq} onAccept={inviteReq.accept} onReject={inviteReq.reject} />
      )}

      {/* —— 弹窗（信令消息统一经弹窗收发：粘贴邀请 / 输入回执 / 展示回执） —— */}
      <PasteModal
        modal={modal}
        modalInput={modalInput}
        modalErr={modalErr}
        setModal={setModal}
        setModalInput={setModalInput}
        setModalErr={setModalErr}
        submitModal={submitModal}
        answerBackUrl={answerBackUrl}
        copyFb={copyFb}
        copyText={s.copyText}
      />

      {/* 装饰行限一行：显示不下硬截断（与顶部 poster-strip 同款处理，不换行不省略号）；
          底部留 safe-area，避开 Android edge-to-edge 的手势条 */}
      <footer style={{ flexShrink: 0, padding: "10px 16px calc(10px + env(safe-area-inset-bottom, 0px))", borderTop: "3px solid var(--ink)", background: "#fff", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, letterSpacing: "0.06em", textTransform: "uppercase", color: "var(--muted)", textAlign: "center", whiteSpace: "nowrap", overflow: "hidden" }}>
        GoPtop · P2P Gomoku & Go · 优先直连 · 服务器可选中转
      </footer>
    </div>
  );
}
