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
  // 聊天面板开合（形态另由 chatDocked 决定：右侧停靠栏 or 覆盖式弹窗）
  const [chatOpen, setChatOpen] = useState(false);
  // 停靠栏还是弹窗，由实测布局决定（compute 里的 measureChat 写回）——旧的
  // 「1080px 媒体查询」是拿视口宽度猜的，看不见「整组被高度压窄」这种显示不下。
  const [chatDocked, setChatDocked] = useState(true);
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
    /* 聊天停靠栏三档（用户拍板 2026-09-19）：
       1) 空间充足 → 与棋盘整组等宽（左右两栏对称，视觉平衡）；
       2) 放不下 → 压缩聊天栏——本软件是下棋软件，棋盘完整优先于聊天栏；
       3) 压到下限 CHAT_MIN_W 仍放不下 → 退回弹窗形态（与窄屏同一策略）。
       必须在 --stack-max 落定之后量：整组宽度本身就是它决定的。 */
    const CHAT_MIN_W = 240;
    const measureChat = (stack: Element) => {
      const layout = stack.parentElement;
      if (!layout) return;
      const gap = parseFloat(getComputedStyle(layout).columnGap) || 0;
      let used = 0;
      let count = 0;
      for (const child of Array.from(layout.children)) {
        // 停靠栏自身不算「已占用」——它要的正是剩下的空间（算进去会自反馈）
        if (child.classList.contains("chat-dock")) continue;
        used += child.getBoundingClientRect().width;
        count++;
      }
      // 加进停靠栏就多一个间隙：n 个已有项 → n 个间隙
      const avail = layout.clientWidth - used - gap * count;
      const stackW = stack.getBoundingClientRect().width;
      const dockW = Math.max(0, Math.min(stackW, Math.floor(avail)));
      main.style.setProperty("--chat-w", `${dockW}px`);
      setChatDocked(dockW >= CHAT_MIN_W);
    };
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
        const stack = boardWrap.parentElement;
        if (!stack) return;
        // 关键：固定卡片（非棋盘）的高度必须在**最宽**的 stack 下量。
        // 卡片宽度越窄内容越换行、越高——若按当前（可能已被挤窄的）宽度量，
        // 会形成「棋盘变小 → stack 变窄 → 卡片变高 → 棋盘更小」的正反馈，
        // 一路塌陷到宽度下限（桌面壳 1100×760 实测棋盘只剩 50px，肉眼不可用）。
        // 先临时撑到最大宽度量一次，再算棋盘可用高度（同一帧内改回，不会闪）。
        main.style.setProperty("--stack-max", "720px");
        const gap = parseFloat(getComputedStyle(stack).rowGap) || 0;
        let fixed = 0;
        let count = 0;
        for (const child of Array.from(stack.children)) {
          if (child === boardWrap) continue;
          fixed += child.getBoundingClientRect().height;
          count++;
        }
        fixed += gap * (count + 1);
        const next = Math.max(240, Math.min(720, Math.floor(main.clientHeight - fixed)));
        // 抖动阈值内的变化不写回（避免来回抖），但仍要走完聊天几何——否则
        // 停靠栏宽度只在整组变宽变窄时才更新，窗口横向缩放时它不跟手。
        const stackMax = Math.abs(next - last) < 4 ? (last || next) : next;
        last = next;
        main.style.setProperty("--stack-max", `${stackMax}px`);
        measureChat(stack);
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

  // 聊天弹窗形态（z-index 800）会整个盖住协商横幅（ConfirmBanner 在常规流里，
  // 静态 z-index 不生效）——请求到了用户根本看不见，对方只能一直等（实机测试发现：
  // 移动端开着聊天时悔棋/换棋/重开请求全部不可达）。有待决请求就收起聊天弹窗，
  // 横幅随即可见；处理完随时可再打开聊天。
  useEffect(() => {
    // 只有弹窗形态才收起；停靠栏在侧边与横幅并排不遮挡，收起反而打断用户
    //（浏览器基线 E2E 曾因此回归）。是不是弹窗由实测布局决定，不看视口宽度。
    if (confirmReq && chatOpen && !chatDocked) setChatOpen(false);
  }, [confirmReq, chatOpen, chatDocked]);

  // 聊天面板内容（停靠栏与弹窗共用同一份；形态由 chatDocked 决定）
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
  const chatOpenable = chatOpen && mode === "p2p" && phase === "playing";
  // 停靠栏与弹窗互斥（形态由实测布局拍板），不会同时进 DOM：
  // 同内容渲染两份既浪费又会让「点第一个匹配」的自动化选到被遮罩的那份。
  const chatDock = chatOpenable && chatDocked && (
    <aside className="brutal-card chat-dock chat-dock--open" style={{ padding: 12 }}>
      {chatPanel}
    </aside>
  );
  const chatModal = chatOpenable && !chatDocked && (
    <div className="chat-modal-bg" onClick={() => setChatOpen(false)}>
      <div className="brutal-card chat-modal" onClick={(e) => e.stopPropagation()} style={{ padding: 12 }}>
        {chatPanel}
      </div>
    </div>
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
        {chatModal}
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
