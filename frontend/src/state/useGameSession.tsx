/**
 * useGameSession —— 对局状态机 + 信令编排（D1 拆分第二刀；本文件为编排层）。
 *
 * 域模块已按职责拆出（行为不变，函数体逐字保留、经 SessionCtx 解构）：
 * - state/serverSignaling.ts：服务器模式信令（userId 链接 + pwd 校验 + 观战房间管理）
 * - state/negotiation.ts：协商（悔棋/重开/换棋，对方同意制）
 * - state/chat.ts：聊天（pushChat/sendChat/resolvePeerName）
 * - state/sessionContext.ts：三域工厂共享的 SessionCtx 接口（ref 所有权留在本文件）
 *
 * 本文件保留：全部 state/refs 与镜像 effect、net 消息分发、presence 编排、
 * 邀请/回执/挑战流程、落子与重置、URL 意图处理，以及逐字不变的返回对象。
 * App.tsx 只剩壳：header、页面拼装、footer——从本 hook 解构取值。
 * hook 与 transport 单例（transport/presence/DirectRtc）交互，与原先完全一致。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { checkFive, emptyBoard } from "../game/board";
import { loadDefaults } from "../pages/components";
import type { Phase, Role } from "../pages/components";
import { serverChannel } from "../net/serverChannel";
import type { ServerEvent, ServerState } from "../net/serverChannel";
import { presence } from "../net/presence";
import type { PeerInfo } from "../net/presence";
import { transport } from "../net/gameChannel";
import type { GameMsg } from "../net/gameChannel";
import {
  answerToUrl,
  inviteToUrl,
  nav,
  parsePastedAnswer,
  parsePastedLink,
  parseUrl,
  userToUrl,
  watchToUrl,
} from "../net/links";
import type { UrlIntent } from "../net/links";
import { genGameId, genPwd, myName, myUserId, setMyName } from "../net/identity";
import { loadServerSelection } from "../net/servers";
import { DirectRtcPeer, wireRtcBroadcast } from "../net/rtc";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import { createChat } from "./chat";
import { createNegotiation } from "./negotiation";
import { createServerSignaling } from "./serverSignaling";
import type { SessionCtx } from "./sessionContext";

export function useGameSession() {
  const defs = useMemo(loadDefaults, []);
  const [kind, setKind] = useState<GameKind>(defs.kind);
  const [size, setSize] = useState<Size>(defs.size);
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(defs.size));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);

  // —— 路由 —— //
  const [intent, setIntent] = useState<UrlIntent>(() => parseUrl());

  // —— 身份与联机 —— //
  const [tabUser] = useState(() => myUserId());
  const [name, setName] = useState(() => myName());
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [role, setRole] = useState<Role>("idle");
  const [phase, setPhase] = useState<Phase>("home");
  const [gameId, setGameId] = useState<string | null>(null);
  const [myColor, setMyColor] = useState<StoneColor>("black");
  const [peerConnected, setPeerConnected] = useState(false);
  /** 连接中断标记：已 open 的直连断掉才置位（对局卡红灯「已中断」）。 */
  const [connLost, setConnLost] = useState(false);
  const [pwd, setPwd] = useState<string | null>(null);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [watchUrl, setWatchUrl] = useState<string | null>(null);
  const [incoming, setIncoming] = useState<{ from: string; fromName: string; kind: GameKind; size: Size; gameId: string } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // notice 的唯一写入口是 showNotice（自带旧 timer 清理）；
  // 直接调 setNotice 会绕过计时管理，造成连续提示时旧 timer 提前清掉新消息
  const noticeTimerRef = useRef<number | null>(null);
  /** 统一 notice 入口：ms 缺省=常驻显示；传 ms 则到期自动清除。 */
  function showNotice(text: string | null, ms?: number) {
    if (noticeTimerRef.current !== null) {
      window.clearTimeout(noticeTimerRef.current);
      noticeTimerRef.current = null;
    }
    setNotice(text);
    if (ms !== undefined) {
      noticeTimerRef.current = window.setTimeout(() => {
        setNotice(null);
        noticeTimerRef.current = null;
      }, ms);
    }
  }
  const [answerBackUrl, setAnswerBackUrl] = useState<string | null>(null);
  const [copyFb, setCopyFb] = useState<string | null>(null);

  // —— 服务器模式（可选）—— //
  // 启动时按设置选中项决定（设置页切换后 reload 生效）：none=无服务器，其余=连服务器。
  const serverMode = loadServerSelection() !== "none";
  const [serverState, setServerState] = useState<ServerState>(serverMode ? "connecting" : "off");
  /** 服务器模式的兜底中转目标（对手 + 观战者的 s- 短 ID）。 */
  const relayTargetsRef = useRef<Set<string>>(new Set());
  /** 服务器模式当前对手的短 ID：观战者可能在等待期先于对手进入 relayTargets，
   *  「取集合首元素当对手」在那种时序下会把房间同步/聊天发给观战者（审计 B2）。 */
  const opponentRef = useRef<string | null>(null);
  /** SyncState 回退纪元：本地悔棋/重开时 +1 并随快照广播（审计 B1，见 protocol.ts）。 */
  const syncEpochRef = useRef(0);
  /** 服务器挑战信（对方点名邀请）：等用户同意/拒绝。 */
  const [serverIncoming, setServerIncoming] = useState<{ from: string; fromName: string; kind: GameKind; size: Size } | null>(null);
  /** 大厅挑战：createInvite 完成后向该目标发挑战信。 */
  const pendingChallengeRef = useRef<string | null>(null);
  /** userId 链接意图（服务器模式）：连接就绪后发 join/spec-join。 */
  const pendingLinkRef = useRef<{ target: string; pwd: string | null; spec: boolean } | null>(null);
  /** 观战钥匙（每局生成、整局有效）：服务器模式观战链接的 pwd。 */
  const specPwdRef = useRef<string | null>(null);
  const [spectateEnabled, setSpectateEnabled] = useState(true);
  const spectateEnabledRef = useRef(true);
  /** 观战房间名单（双方对局者共同维护，经 spec-sync 同步）。 */
  const [spectators, setSpectators] = useState<{ id: string; name: string; host: string; muted: boolean }[]>([]);
  /** 观战申请（pwd 错/无 → 私有申请，仅被申请的对局者可见，聊天区内处理）。 */
  const [specRequests, setSpecRequests] = useState<{ from: string; fromName: string }[]>([]);
  const specRequestsRef = useRef<{ from: string; fromName: string }[]>([]);
  useEffect(() => { specRequestsRef.current = specRequests; }, [specRequests]);
  /** 协商请求弹窗（悔棋/重开/换棋 + 观战发言批准 + 错钥匙连接询问）。 */
  const [confirmReq, setConfirmReq] = useState<{ kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd"; from: string; fromName: string } | null>(null);
  /** 弹窗决定后的续体：同意走 resolve，拒绝走 reject（由发起方在 setConfirmReq 时挂上）。 */
  const confirmResolveRef = useRef<(() => void) | null>(null);
  const confirmRejectRef = useRef<(() => void) | null>(null);
  /** 聊天记录（对局者 Chat 广播 + 观战者经 host 转发的 spec-chat 统一进这里）。 */
  const [chatLog, setChatLog] = useState<{ userId: string; name: string; text: string; ts: number; self: boolean }[]>([]);
  /** 对端头像（P2P 直连后互相发送；观战者头像经 host 转发）。 */
  const [peerAvatars, setPeerAvatars] = useState<Record<string, string>>({});
  const myAvatarRef = useRef<string | null>(null);
  /** 观战者侧：发言批准状态（双 host 均 ack 才可发言；被拒则本局锁死）。 */
  const [specCanChat, setSpecCanChat] = useState(false);
  const specCanChatRef = useRef(false);
  const specChatOkRef = useRef<Set<string>>(new Set());
  const specRequestDeniedRef = useRef(false);
  /** 观战房间名（双方对局者各维护一份全量名单，经 spec-sync 同步）。 */
  const spectatorsRef = useRef<{ id: string; name: string; host: string; muted: boolean }[]>([]);
  const myHostRef = useRef<string | null>(null); // 观战者自己连的对局者

  /* ---------- 弹窗（所有信令消息统一走弹窗收发） ---------- */
  // kind:
  // - "paste-invite"：粘贴邀请链接（受邀者侧）
  // - "paste-answer"：粘贴回执链接（邀请者侧，等对手时）
  // - "receipt"：展示本方生成的回执链接（受邀者侧，供复制发回邀请者）
  type ModalKind = "paste-invite" | "paste-answer" | "receipt";
  const [modal, setModal] = useState<ModalKind | null>(null);
  const [modalInput, setModalInput] = useState("");
  const [modalErr, setModalErr] = useState<string | null>(null);

  // refs：供消息回调读取最新值
  const boardRef = useRef(board);
  const toMoveRef = useRef(toMove);
  const winnerRef = useRef(winner);
  const historyRef = useRef(history);
  const lastMoveRef = useRef(lastMove);
  const kindRef = useRef(kind);
  const sizeRef = useRef(size);
  const myColorRef = useRef(myColor);
  const roleRef = useRef(role);
  const gameIdRef = useRef<string | null>(null);
  const pwdRef = useRef<string | null>(null);
  const phaseRef = useRef(phase);
  const rtcPeersRef = useRef<DirectRtcPeer[]>([]);
  const inviterRtcRef = useRef<DirectRtcPeer | null>(null);
  const bootRef = useRef(false);
  const inviteDoneRef = useRef<string | null>(null);
  // 挑战/同意信件队列：presence 回调只收件，消费逻辑走下面的 drain effect，
  // 避免 StrictMode 重挂载时闭包函数捕获旧 state 导致受邀者永远收不到 accept。
  type ChallengeMsg = { from: string; fromName: string; pwd: string | null; kind: GameKind; size: Size; gameId: string; rtcAns: string | null };
  const challengeQueueRef = useRef<ChallengeMsg[]>([]);
  const acceptQueueRef = useRef<{ from: string; gameId: string }[]>([]);
  const rejectQueueRef = useRef<{ from: string; gameId: string }[]>([]);
  const [signalTick, setSignalTick] = useState(0);
  useEffect(() => { boardRef.current = board; }, [board]);
  useEffect(() => { toMoveRef.current = toMove; }, [toMove]);
  useEffect(() => { winnerRef.current = winner; }, [winner]);
  useEffect(() => { historyRef.current = history; }, [history]);
  useEffect(() => { lastMoveRef.current = lastMove; }, [lastMove]);
  useEffect(() => { kindRef.current = kind; }, [kind]);
  useEffect(() => { sizeRef.current = size; }, [size]);
  useEffect(() => { myColorRef.current = myColor; }, [myColor]);
  useEffect(() => { roleRef.current = role; }, [role]);
  useEffect(() => { gameIdRef.current = gameId; }, [gameId]);
  useEffect(() => { pwdRef.current = pwd; }, [pwd]);
  useEffect(() => { phaseRef.current = phase; }, [phase]);

  // kind 切换时自动修正 size
  useEffect(() => {
    const nextSize: Size = kind === "gomoku" ? 15 : size === 15 ? 19 : size;
    if (nextSize !== size) setSize(nextSize);
  }, [kind]); // eslint-disable-line react-hooks/exhaustive-deps

  // 主页 size 变化重置棋盘；对局中由 SyncState 驱动
  useEffect(() => {
    if (phase !== "home") return;
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }, [size, phase]);

  /* ---------- 对局数据消息 ---------- */
  const handleNetMessage = useCallback((msg: GameMsg) => {
    const k = msg.kind;
    switch (k.type) {
      case "SyncRequest": {
        setPeerConnected(true);
        transport.send({
          type: "SyncState", sv: syncEpochRef.current,
          board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
          history: historyRef.current, lastMove: lastMoveRef.current,
          kind: kindRef.current, size: sizeRef.current,
        });
        break;
      }
      case "SyncState": {
        setPeerConnected(true);
        // 覆盖守卫（审查 A3 + 回退纪元）：快照版本 = (sv, history.length) 双键。
        // 直连 open 后本方可能先落子，对方 200ms 后发出的旧空板快照若后到会吞掉
        // 这一手——只应用不旧于本地的快照；同长视为同版本（正常对局中双方历史
        // 一致，覆盖无害）。仅比 length 会让悔棋/重开的「合法变短」回退被丢弃，
        // 观战者从此发散（审计 B1）——sv 更旧的快照才是真旧快照。
        const sv = k.sv ?? 0;
        const mySv = syncEpochRef.current;
        if (sv < mySv || (sv === mySv && k.history.length < historyRef.current.length)) break;
        if (k.kind !== kindRef.current) setKind(k.kind);
        if (k.size !== sizeRef.current) setSize(k.size);
        setBoard(k.board.map((r) => [...r]));
        setToMove(k.toMove);
        setWinner(k.winner);
        setHistory([...k.history]);
        setLastMove(k.lastMove ? { ...k.lastMove } : null);
        break;
      }
      case "Hello": {
        setPeerConnected(true);
        if (myColorRef.current === "black") {
          transport.send({
            type: "SyncState", sv: syncEpochRef.current,
            board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
            history: historyRef.current, lastMove: lastMoveRef.current,
            kind: kindRef.current, size: sizeRef.current,
          });
        }
        break;
      }
      case "Move": {
        setPeerConnected(true);
        // 执子颜色以消息自带的 by 为准，不从本地推断：观战者没有"我的颜色"，
        // 对局者若本地 toMove 与发送端有偏差会错色。旧格式无 by 的消息直接丢弃
        // （不猜）——猜测曾致"任何一方认输观战者都判白胜"。
        const by = k.by;
        if (by !== "black" && by !== "white") break;
        if (k.move.type === "Place") {
          const c = k.move.coord;
          const mover = by;
          if (winnerRef.current) break;
          if (mover !== toMoveRef.current) break; // 非行棋方的落子无效（乱序/重放防御）
          const curBoard = boardRef.current;
          // 跨页消息可能早于受邀者的 kind/size 生效：按消息自带尺寸做边界检查，
          // 落子合并以当前棋盘为准（双方 kind/size 由 SyncState 对齐）。
          const n = curBoard.length;
          if (c.x < 0 || c.y < 0 || c.x >= n || c.y >= n) break;
          if (curBoard[c.y]?.[c.x] !== "empty") break;
          const next = curBoard.map((r) => [...r]);
          next[c.y][c.x] = mover;
          const willWin = kindRef.current === "gomoku" && checkFive(next, c, mover);
          setBoard(next);
          setLastMove(c);
          setHistory((h) => [...h, c]);
          if (willWin) setWinner(mover);
          else setToMove(mover === "black" ? "white" : "black");
        } else if (k.move.type === "Pass") {
          if (by !== toMoveRef.current) break;
          setToMove(by === "black" ? "white" : "black");
        } else if (k.move.type === "Resign") {
          // 认输者 = by，胜者是其对手——与接收方本地颜色无关（观战者/发送方回流都正确）
          setWinner(by === "black" ? "white" : "black");
        }
        break;
      }
      case "Reset": {
        setPeerConnected(true);
        setKind(k.kind);
        setSize(k.size);
        setBoard(emptyBoard(k.size));
        setToMove("black");
        setWinner(null);
        setLastMove(null);
        setHistory([]);
        setHover(null);
        break;
      }
      case "Chat": {
        // 聊天（无服务器模式走 GameMsg；服务器模式对局者走 signal——本 case 服务前者）
        pushChat(msg.userId, msg.sender, k.text, false);
        break;
      }
      case "Avatar": {
        // 头像：P2P 直达，不经服务器（dataURL ≤128px）
        if (typeof k.dataUrl === "string" && k.dataUrl.startsWith("data:image/") && k.dataUrl.length < 20_000) {
          setPeerAvatars((m) => ({ ...m, [msg.userId]: k.dataUrl }));
        }
        break;
      }
      // —— 协商请求：对方同意制，弹窗处理 ——
      case "UndoReq": {
        if (phaseRef.current !== "playing") break;
        setConfirmReq({ kind: "undo", from: msg.userId, fromName: resolvePeerName(msg.userId, msg.sender) });
        confirmResolveRef.current = () => {
          transport.send({ type: "UndoAck", ok: true });
          applyUndoLocal();
        };
        confirmRejectRef.current = () => transport.send({ type: "UndoAck", ok: false });
        break;
      }
      case "UndoAck": {
        if (k.ok) {
          applyUndoLocal();
          showNotice("对方已同意悔棋", 2400);
        } else {
          showNotice("对方拒绝了悔棋", 2400);
        }
        break;
      }
      case "ResetReq": {
        if (phaseRef.current !== "playing") break;
        setConfirmReq({ kind: "reset", from: msg.userId, fromName: resolvePeerName(msg.userId, msg.sender) });
        confirmResolveRef.current = () => {
          transport.send({ type: "ResetAck", ok: true });
          doResetLocal();
        };
        confirmRejectRef.current = () => transport.send({ type: "ResetAck", ok: false });
        break;
      }
      case "ResetAck": {
        if (k.ok) {
          doResetLocal();
          showNotice("对方已同意重开", 2400);
        } else {
          showNotice("对方拒绝了重开", 2400);
        }
        break;
      }
      case "SwapReq": {
        if (phaseRef.current !== "playing") break;
        setConfirmReq({ kind: "swap", from: msg.userId, fromName: resolvePeerName(msg.userId, msg.sender) });
        confirmResolveRef.current = () => {
          transport.send({ type: "SwapAck", ok: true });
          doSwapLocal();
        };
        confirmRejectRef.current = () => transport.send({ type: "SwapAck", ok: false });
        break;
      }
      case "SwapAck": {
        if (k.ok) {
          doSwapLocal();
          showNotice("对方已同意换棋（黑白互换，已重开）", 3000);
        } else {
          showNotice("对方拒绝了换棋", 2400);
        }
        break;
      }
      case "Ping":
      case "Pong":
        break;
    }
  }, []);

  /** 按 URL 意图行动：观战加入 / 邀请自动挑战 / 服务器模式 userId 链接（pwd 客户端校验）。
   *  回执不再走「打开链接」——新标签页身份不同且邀请者 RTC 状态不可迁移，
   *  曾导致同机两窗互弈；统一走等待页「输入回执」弹窗。 */
  function processIntent(it: UrlIntent) {
    if (it.mode === "user" && it.spec === true && phaseRef.current === "home") {
      // 服务器观战链接 `/<userId>?pwd=<specPwd>&spec=1`
      if (!serverMode) {
        showNotice("跨设备观战需要连接服务器：请在设置页选择与房主相同的服务器", 4200);
        return;
      }
      pendingLinkRef.current = { target: it.userId, pwd: it.pwd, spec: true };
      signaling.flushPendingServerIntents();
      return;
    }
    if (it.mode === "user" && it.pwd && serverMode && phaseRef.current === "home") {
      // 服务器模式邀请链接 `/<userId>?pwd=...`：pwd 校验在对局者端（错误会转弹窗询问）
      if (it.userId === tabUser) return;
      if (it.rtc) {
        // 带 rtc 的无服务器式链接：仍按旧路径处理（跨设备回执兼容）
        acceptInvite(it.userId, it.pwd, it.kind, it.size, it.rtc);
        return;
      }
      pendingLinkRef.current = { target: it.userId, pwd: it.pwd, spec: false };
      signaling.flushPendingServerIntents();
      return;
    }
    if (it.mode === "watch" && phaseRef.current === "home") {
      joinAsSpectator(it.gameId);
    } else if (it.mode === "user" && phaseRef.current === "home") {
      let hasReceipt = false;
      try { hasReceipt = !!new URL(window.location.href).searchParams.get("rtcAns"); } catch { /* ignore */ }
      if (hasReceipt) {
        // 回执链接被当页面打开（而非粘贴进弹窗）：绝不据此发起挑战，否则同源两窗会互弈
        showNotice("这是回执链接：请在邀请者的「等待对手」页点「输入回执」粘贴它", 3200);
        return;
      }
      if (it.userId === tabUser) return; // 自己的主页
      if (!it.pwd) return; // 无 pwd：仅展示对方主页，由用户手动挑战
      if (roleRef.current !== "idle") return; // 已在对局流程中：忽略重复意图
      const href = window.location.href;
      if (inviteDoneRef.current === href) return;
      inviteDoneRef.current = href;
      // 带 pwd 打开某用户主页：自动发起带钥匙的连接请求；邀请者校验 pwd 自动同意
      acceptInvite(it.userId, it.pwd, it.kind, it.size, it.rtc ?? null);
    }
  }

  const processedIntentRef = useRef<string | null>(null);

  function resetBoardFor(k: GameKind, s: Size) {
    setKind(k);
    setSize(s);
    setBoard(emptyBoard(s));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
  }

  /** 邀请者：开启对战（waiting），生成每局轮换 pwd + gameId，后台预生成直连 offer 编进邀请链接。
   *  服务器模式下 offer 需异步生成后再交给服务器换短码，故整体 async。 */
  async function createInvite() {
    closeAllRtcPeers();
    const g = genGameId();
    const p = genPwd();
    setGameId(g);
    gameIdRef.current = g;
    setPwd(p);
    pwdRef.current = p;
    setRole("inviter");
    roleRef.current = "inviter";
    setPhase("waiting");
    phaseRef.current = "waiting";
    setMyColor("black");
    setPeerConnected(false);
    resetBoardFor(kind, size);
    transport.join(g);
    // 邀请链接只在 offer 生成完成后才提供（含 rtc 才能跨设备直连）。
    // 旧实现先给无 rtc 链接：用户复制发出的链接跨设备永远连不上（审查 A5）。
    setInviteUrl(null);
    setWatchUrl(null);
    // —— 服务器模式：链接回归 /<userId>?pwd=（无 rtc，最短）；pwd 校验在收到 join
    //    signal 时于本端进行（错误转弹窗询问），offer 在校验通过后才生成发送 ——
    if (serverMode) {
      if (!serverChannel.connected) {
        showNotice("服务器未连接：请检查设置页服务器配置或切换到无服务器模式");
        return;
      }
      showNotice(null);
      // 观战钥匙一并生成（整局有效），对局页可展示观战链接
      specPwdRef.current = genPwd();
      spectateEnabledRef.current = true;
      setSpectateEnabled(true);
      setInviteUrl(inviteToUrl(tabUser, p, kind, size));
      const peer = new DirectRtcPeer({ isInviter: true, role: "player" });
      attachPeer(peer);
      inviterRtcRef.current = peer;
      return;
    }
    showNotice("正在生成直连邀请…");
    // 后台预生成 offer：用户只需复制最终邀请链接，无需触碰 offer 文本
    void (async () => {
      const peer = new DirectRtcPeer({ isInviter: true, role: "player" });
      attachPeer(peer);
      try {
        const offer = await peer.createOffer(p);
        // await 间隙用户可能已取消/换局：不再写本局 state（与 acceptReceipt 的 R2 守卫同模式）
        if (phaseRef.current !== "waiting" || roleRef.current !== "inviter" || pwdRef.current !== p) {
          try { peer.close(); } catch { /* ignore */ }
          return;
        }
        inviterRtcRef.current = peer;
        setInviteUrl(inviteToUrl(tabUser, p, kind, size, offer));
        showNotice(null);
      } catch {
        inviterRtcRef.current = null;
        try { peer.close(); } catch { /* ignore */ }
        // offer 生成失败：退化为无 rtc 链接（仅同源可玩），如实告知跨设备不可用
        setInviteUrl(inviteToUrl(tabUser, p, kind, size));
        showNotice("直连邀请生成失败：已生成同源链接（跨设备不可用），可取消后重开");
      }
    })();
  }

  /** 受邀者：向某用户发起挑战（pwd 可空）。发起后统一到 P2P 页等待/对战。
   *  入口先清理上一局残留（RTC peer/邀请者信令），防止 Awaiting 中粘贴新邀请
   *  时旧连接泄漏（审查 A7：角色被覆盖但底层 PeerConnection 仍开着）。 */
  function acceptInvite(inviterId: string, pwdOrNull: string | null, k: GameKind, s: Size, inviteOffer?: string | null) {
    closeAllRtcPeers();
    const g = genGameId();
    setGameId(g);
    gameIdRef.current = g;
    setRole("invitee");
    roleRef.current = "invitee";
    setMyColor("white");
    myColorRef.current = "white";
    setPeerConnected(false);
    resetBoardFor(k, s);
    transport.join(g);
    setPhase("waiting");
    phaseRef.current = "waiting";
    setPwd(null);
    setInviteUrl(null);
    setWatchUrl(null);
    setAnswerBackUrl(null);
    if (inviteOffer) {
      // 跨设备一键直连：邀请链接自带邀请者 offer，受邀者后台自动生成 answer。
      // 同源页面间 answer 经 Presence 自动回传；跨设备时生成回执链接由受邀者发回邀请者。
      showNotice("邀请已受理，正在建立 P2P 直连…");
      nav("/p2p");
      void (async () => {
        const peer = new DirectRtcPeer({ isInviter: false, role: "player" });
        attachPeer(peer);
        try {
          const ans = await peer.acceptOffer(inviteOffer, pwdOrNull ?? "");
          // 受邀者的 gameId + answer 发给邀请者：同源经 Presence 自动送达；
          // 跨设备 Presence 不可达，弹窗展示回执链接，发回邀请者「输入回执」粘贴即可
          presence.challenge(inviterId, pwdOrNull, k, s, g, ans);
          setAnswerBackUrl(answerToUrl(inviterId, pwdOrNull ?? "", ans, g, k, s));
          // 回执弹窗延迟弹出：若同源 Presence 已把 answer 送达（直连很快建立），就不打扰
          setTimeout(() => {
            if (peer.state !== "open") setModal("receipt");
          }, 1200);
        } catch {
          showNotice("直连建立失败，可检查设置页线路后重试");
        }
      })();
      return;
    }
    // 受邀者的 gameId 发给邀请者，邀请者接受后双方用受邀者的 gameId 建 channel
    presence.challenge(inviterId, pwdOrNull, k, s, g);
    showNotice(pwdOrNull ? "已带邀请钥匙请求连接，等待邀请者自动确认…" : "已发送挑战，等待对方同意…");
    nav("/p2p");
  }

  /** 邀请者用受邀者回传的 answer 完成直连（同源自动；跨设备由弹窗粘贴回执触发）。
   *  返回 null=成功；其余为失败原因文案。不做任何状态变更之外的副作用。 */
  async function applyAnswer(ans: string, pwd: string): Promise<string | null> {
    const peer = inviterRtcRef.current;
    // 无待用 offer（offer 生成失败过）：显式报错，不能静默——静默会让邀请者以为
    // 回执已受理，受邀者却永远等不到直连（审查 A4）
    if (!peer || peer.state !== "waiting-invitee") {
      return "本局邀请的直连信令未就绪（可能生成失败），无法受理回执；请取消等待后重新开战";
    }
    try {
      await peer.acceptAnswer(ans, pwd);
      showNotice("对方已加入，直连建立中…");
      return null;
    } catch (err) {
      // 双路径竞态：弹窗与 Presence 同时送达时，后到路径遇到已成功应用不算失败
      if (peer.answered) return null;
      const msg = err instanceof Error && err.message === "unknown rtc token"
        ? "回执无法解码（可能不是本程序生成的回执）"
        : "回执解码或直连建立失败，请让对方重新发送回执";
      return msg;
    }
  }

  /** 邀请者受理回执：校验钥匙 → 完成直连 → 切到受邀者的 game channel 进对局。
   *  回执路径（弹窗粘贴）都走这里；同源 Presence 路径走 acceptChallenge。
   *  返回错误信息（null 表示受理成功），由弹窗就地展示。 */
  async function acceptReceipt(r: { inviterId: string; pwd: string; rtcAns: string; spectator: boolean; gameId: string | null; kind: GameKind | null; size: Size | null }): Promise<string | null> {
    if (roleRef.current !== "inviter" || phaseRef.current !== "waiting") {
      return "当前不在等待对手状态，无法受理回执";
    }
    if (r.inviterId !== tabUser) {
      return `回执是发给邀请者 ${r.inviterId} 的，本页是 ${tabUser}，不能代收`;
    }
    if (r.pwd !== pwdRef.current) {
      return "回执钥匙与本局不符，已拒绝";
    }
    if (r.spectator) {
      // 观战回执（预留）：跨设备观战尚未实现，此分支只为自动识别与明确报错
      return "跨设备观战尚未实现：已识别为观战回执，请等待后续版本";
    }
    // 直连应答必须先应用成功，才允许进入对局状态——失败时保持 waiting，
    // 弹窗留在原地可重试（旧实现先切 playing 再异步等结果，坏回执会让整局作废）
    const err = await applyAnswer(r.rtcAns, r.pwd);
    if (err) return err;
    // await 间隙用户可能已「取消等待」（backHome 清了 role/phase/keys）：
    // 续体若无条件执行，会把已回主页的用户拉回对局（修复轮 R2 竞态）
    if (phaseRef.current !== "waiting" || roleRef.current !== "inviter") return null;
    const k = r.kind ?? kindRef.current;
    const s = r.size ?? sizeRef.current;
    if (r.gameId) {
      // 切到受邀者的 game channel（两人同一 channel）
      transport.join(r.gameId);
      setGameId(r.gameId);
      gameIdRef.current = r.gameId;
      setWatchUrl(watchToUrl(r.gameId));
    }
    setRole("inviter");
    roleRef.current = "inviter";
    setPhase("playing");
    phaseRef.current = "playing";
    setMyColor("black");
    myColorRef.current = "black";
    setPeerConnected(false);
    resetBoardFor(k, s);
    // pwd 失效：两人已满，不再接受第三人
    setPwd(null);
    pwdRef.current = null;
    setInviteUrl(null);
    setIncoming(null);
    showNotice("回执已受理，直连建立中…");
    nav("/p2p");
    setTimeout(() => {
      transport.send({ type: "Hello", kind: k, size: s });
      transport.send({ type: "SyncRequest" });
    }, 80);
    return null;
  }

  /** 弹窗确认：按弹窗类型解析粘贴文本并执行。解析与域名无关；
   *  校验失败留在弹窗内报错，成功才关闭。回执受理含异步信令，整体 async。 */
  async function submitModal() {
    const m = modal;
    if (!m) return;
    if (m === "paste-invite") {
      const it = parsePastedLink(modalInput);
      if (!it) {
        setModalErr("无法识别该链接：请完整粘贴邀请链接或主页链接");
        return;
      }
      if (it.mode === "user") {
        if (phaseRef.current !== "home" && phaseRef.current !== "waiting") {
          setModalErr("正在对局中，请先离开再加入新对局");
          return;
        }
        if (it.userId === tabUser) {
          setModalErr("这是你自己的主页链接");
          return;
        }
        setModalInput("");
        setModalErr(null);
        setModal(null);
        acceptInvite(it.userId, it.pwd, it.kind, it.size, it.rtc);
        return;
      }
      if (it.mode === "watch") {
        if (phaseRef.current !== "home") {
          setModalErr("正在对局中，请先离开再观战");
          return;
        }
        setModalInput("");
        setModalErr(null);
        setModal(null);
        joinAsSpectator(it.gameId);
        return;
      }
      setModalErr("该链接是本站页面链接，不是邀请链接");
      return;
    }
    if (m === "paste-answer") {
      const r = parsePastedAnswer(modalInput);
      if (!r) {
        setModalErr("无法识别该回执：请完整粘贴受邀者发来的回执链接（含 rtcAns）");
        return;
      }
      const err = await acceptReceipt(r);
      if (err) {
        setModalErr(err);
        return;
      }
      setModalInput("");
      setModalErr(null);
      setModal(null);
      return;
    }
  }

  /** 邀请者接受挑战：pwd 失效（两人满员），进入 playing。 */
  function acceptChallenge(from: string, k: GameKind, s: Size, inviteeGameId: string, auto: boolean, inviteeAns?: string | null) {
    // 同源路径：answer 经 BroadcastChannel 送达，由刚生成的对端产生、损坏概率极低，
    // 不阻塞进局（同源本就不依赖 WebRTC）；万一失败仅显示直连错误，棋局仍可下
    if (inviteeAns) void applyAnswer(inviteeAns, pwdRef.current ?? "");
    // 切换到受邀者的 game channel（两人同一 channel）
    transport.join(inviteeGameId);
    setGameId(inviteeGameId);
    gameIdRef.current = inviteeGameId;
    setRole("inviter");
    roleRef.current = "inviter";
    setPhase("playing");
    phaseRef.current = "playing";
    setMyColor("black");
    myColorRef.current = "black";
    setPeerConnected(false);
    resetBoardFor(k, s);
    // pwd 失效：两人已满，不再接受第三人
    setPwd(null);
    pwdRef.current = null;
    setInviteUrl(null);
    setWatchUrl(watchToUrl(inviteeGameId));
    presence.accept(from, inviteeGameId);
    setIncoming(null);
    showNotice(auto ? "邀请钥匙校验通过，已自动开始对局" : "已接受挑战，对局开始");
    nav("/p2p");
    setTimeout(() => {
      transport.send({ type: "Hello", kind: k, size: s });
      transport.send({ type: "SyncRequest" });
    }, 80);
  }

  function rejectChallenge(from: string, inviteeGameId: string) {
    presence.reject(from, inviteeGameId);
    setIncoming(null);
    showNotice("已拒绝该挑战");
  }

  function enterPlayingAsInvitee() {
    if (phaseRef.current === "playing") return;
    setModal(null); // 若回执弹窗还开着（同源延迟弹出的竞态），对局开始即收起
    setPhase("playing");
    phaseRef.current = "playing";
    setPwd(null);
    pwdRef.current = null;
    // 服务器模式观战走 spec 链接（spec 钥匙）；/watch/ 是同源旧通道，跨设备无效不展示
    if (!serverMode) setWatchUrl(watchToUrl(gameIdRef.current ?? ""));
    showNotice("对方已同意，对局开始（你执白）");
    setTimeout(() => {
      transport.send({ type: "Hello", kind: kindRef.current, size: sizeRef.current });
      transport.send({ type: "SyncRequest" });
    }, 80);
  }

  function joinAsSpectator(g: string) {
    closeAllRtcPeers();
    setGameId(g);
    gameIdRef.current = g;
    setRole("spectator");
    roleRef.current = "spectator";
    setPhase("playing");
    phaseRef.current = "playing";
    // 观战者没有执子颜色：占位 white 仅供 UI（棋盘禁用等），
    // 消息判定一律用消息自带 by（审查 A2），不得从 myColor 推断
    setMyColor("white");
    myColorRef.current = "white";
    setPeerConnected(false);
    transport.join(g);
    setPwd(null);
    pwdRef.current = null;
    setInviteUrl(null);
    setWatchUrl(null);
    showNotice("观战模式：只读同步，不可落子");
    setTimeout(() => transport.send({ type: "SyncRequest" }), 120);
  }

  /** 关闭全部 P2P 连接与棋盘 channel（换局入口 createInvite/acceptInvite 与 backHome 共用）。
   *  开新局前必调：旧 PeerConnection 不关会泄漏（审查 A7）。 */
  function closeAllRtcPeers() {
    transport.leave();
    for (const p of rtcPeersRef.current) { try { p.close(); } catch { /* ignore */ } }
    rtcPeersRef.current = [];
    inviterRtcRef.current = null;
    relayTargetsRef.current = new Set();
    // peer.close() 的回调在本函数返回前同步触发时 phase 仍是 playing，
    // 会误置 connLost——这里显式复位（审计 B3：旧局的红灯不得带进新局）
    setConnLost(false);
    opponentRef.current = null;
    // 观战聊天权限是「本局」概念：不复位会把上局的获准/被拒带进新局（审计 B4）
    specCanChatRef.current = false;
    setSpecCanChat(false);
    specChatOkRef.current.clear();
    specRequestDeniedRef.current = false;
    pendingChallengeRef.current = null;
    setServerIncoming(null);
    setModal(null);
    setModalInput("");
    setModalErr(null);
  }

  function backHome() {
    closeAllRtcPeers();
    // 观战钥匙不清理会泄漏到下一局：取消等待后被挑战进局，卡片会挂上过期钥匙的 spec 链接
    specPwdRef.current = null;
    spectateEnabledRef.current = true;
    setSpectateEnabled(true);
    setRole("idle");
    roleRef.current = "idle";
    setPhase("home");
    phaseRef.current = "home";
    setGameId(null);
    gameIdRef.current = null;
    setPwd(null);
    pwdRef.current = null;
    setInviteUrl(null);
    setWatchUrl(null);
    setIncoming(null);
    setPeerConnected(false);
    setMyColor("black");
    myColorRef.current = "black";
    showNotice(null);
    setAnswerBackUrl(null);
    nav("/");
    setBoard(emptyBoard(sizeRef.current));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
  }

  async function copyText(t: string, okMsg: string) {
    try {
      await navigator.clipboard.writeText(t);
      setCopyFb(okMsg);
      setTimeout(() => setCopyFb(null), 1600);
    } catch {
      setCopyFb("复制失败，请手动复制");
      setTimeout(() => setCopyFb(null), 2000);
    }
  }

  function attachPeer(p: DirectRtcPeer) {
    // 只有「已 open 过的连接」断掉才算中断：建局/换局时的主动 close 不亮红灯
    let wasOpen = false;
    p.onRemote = (msg) => transport.injectRemote(msg);
    p.onState = (s) => {
      // 关闭/失败即出列：rtcPeersRef 只装活连接，防止跨局累积（审查 D7）。
      // 红灯只跟「对手」的连接走：观战者离开不构成对局中断（审计 B3）
      if (s === "closed" || s === "error") {
        rtcPeersRef.current = rtcPeersRef.current.filter((q) => q !== p);
        if (wasOpen && phaseRef.current === "playing" && p.role === "player") setConnLost(true);
      }
      if (s === "open") {
        wasOpen = true;
        setConnLost(false);
        setPeerConnected(true);
        // 跨设备时 Presence 不可达（无 BroadcastChannel）：直连一旦打通，
        // 等待中的受邀者直接进对局，不再依赖邀请者的 accept 信件
        if (roleRef.current === "invitee" && phaseRef.current === "waiting") enterPlayingAsInvitee();
        // 直连建立后互发头像（P2P，不经服务器）
        if (myAvatarRef.current) {
          setTimeout(() => transport.send({ type: "Avatar", dataUrl: myAvatarRef.current! }), 150);
        }
        // 直连建立后主动同步一次
        setTimeout(() => {
          transport.send({
            type: "SyncState", sv: syncEpochRef.current,
            board: boardRef.current, toMove: toMoveRef.current, winner: winnerRef.current,
            history: historyRef.current, lastMove: lastMoveRef.current,
            kind: kindRef.current, size: sizeRef.current,
          });
        }, 200);
      }
    };
    rtcPeersRef.current.push(p);
  }

  /* ---------- 头像（圆形裁剪，本地存储，P2P 交换） ---------- */

  function loadMyAvatar(): string | null {
    try { return localStorage.getItem("goptop:avatar"); } catch { return null; }
  }

  function saveMyAvatar(dataUrl: string | null) {
    myAvatarRef.current = dataUrl;
    try {
      if (dataUrl) localStorage.setItem("goptop:avatar", dataUrl);
      else localStorage.removeItem("goptop:avatar");
    } catch { /* ignore */ }
  }

  /** 弹窗统一处理：同意/拒绝走挂上的续体。 */
  function confirmApprove() {
    const r = confirmResolveRef.current;
    confirmResolveRef.current = null;
    confirmRejectRef.current = null;
    setConfirmReq(null);
    r?.();
  }

  function confirmDecline() {
    const r = confirmRejectRef.current;
    confirmResolveRef.current = null;
    confirmRejectRef.current = null;
    setConfirmReq(null);
    r?.();
  }

  /* ---------- 落子 ---------- */
  function handlePlace(c: Coord) {
    if (winner) return;
    if (board[c.y][c.x] !== "empty") return;
    if (role === "spectator") return;
    if (phase === "waiting") return;
    if (phase === "playing" && peerConnected && toMove !== myColor) return;
    // 直连未建立时：服务器模式可经兜底中转落子（relay 双发去重），无服务器模式不可
    if (phase === "playing" && !peerConnected && !(serverMode && relayTargetsRef.current.size > 0)) return;

    const mover = toMove;
    const next = board.map((row) => [...row]);
    next[c.y][c.x] = mover;
    const willWin = kind === "gomoku" && checkFive(next, c, mover);
    setBoard(next);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (willWin) setWinner(mover);
    else setToMove(mover === "black" ? "white" : "black");

    if (phase === "playing") {
      transport.send({ type: "Move", move: { type: "Place", coord: c }, by: mover });
    }
  }

  function reset() {
    const s = size;
    const k = kind;
    setBoard(emptyBoard(s));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
    if (phase === "playing") {
      // 本地对战：直接重开并广播；P2P：改为对方同意制（见 requestReset）
      if (serverMode && (role === "inviter" || role === "invitee")) {
        requestReset();
        return;
      }
      transport.send({ type: "Reset", kind: k, size: s });
    }
  }

  function saveName() {
    setMyName(name.trim());
    showNotice(name.trim() ? `昵称已保存：${name.trim()}` : "昵称已清空", 1600);
  }

  /* ---------- 域模块组装（编排层） ---------- */
  // 组合顺序：states/refs/函数声明 → ctx → 三域工厂 → effects（见下方各 effect）。
  // pushChat 的实现在 chat 域，而 ctx 需先于工厂存在：这里先放占位，createChat
  // 返回后立即回填。serverSignaling 在工厂顶部解构时拿到的已是真实现；
  // handleNetMessage 经下方同名解构绑定同样落到 chat 域实现（setChatLog 为
  // 稳定引用，闭包语义与原先完全一致）。
  const ctx: SessionCtx = {
    peers, chatLog, spectators, serverIncoming, serverMode, tabUser,
    setBoard, setHistory, setHover, setLastMove, setMyColor, setPhase, setRole,
    setToMove, setWinner, setAnswerBackUrl, setChatLog, setConfirmReq, setGameId,
    setInviteUrl, setPeerAvatars, setPeerConnected, setPwd, setServerIncoming,
    setSpecCanChat, setSpecRequests, setSpectateEnabled, setSpectators, setWatchUrl,
    boardRef, historyRef, kindRef, lastMoveRef, myColorRef, phaseRef, roleRef,
    sizeRef, toMoveRef, winnerRef, confirmRejectRef, confirmResolveRef, gameIdRef,
    inviterRtcRef, myHostRef, opponentRef, pendingLinkRef, pwdRef, relayTargetsRef, rtcPeersRef,
    specCanChatRef, specChatOkRef, specPwdRef, specRequestDeniedRef, specRequestsRef,
    spectateEnabledRef, spectatorsRef, syncEpochRef,
    showNotice, attachPeer, backHome, closeAllRtcPeers, resetBoardFor,
    pushChat: () => {},
  };
  const negotiation = createNegotiation(ctx);
  const chat = createChat(ctx);
  ctx.pushChat = chat.pushChat;
  const signaling = createServerSignaling(ctx);
  // 原名解构：handleNetMessage/reset 与返回对象保持逐字不变，调用落到域模块实现。
  const { applyUndoLocal, requestUndo, requestReset, requestSwap, doResetLocal, doSwapLocal } = negotiation;
  const { pushChat, sendChat, resolvePeerName } = chat;
  const {
    serverChallengePeer, serverAcceptChallenge, serverRejectChallenge,
    approveSpecRequest, rejectSpecRequest, kickSpectator, muteSpectator,
    disableSpectate, requestSpecChat,
  } = signaling;

  useEffect(() => {
    const off = transport.onMessage(handleNetMessage);
    return off;
  }, [handleNetMessage]);

  /* ---------- 在线发现与挑战 ---------- */
  // 注意：StrictMode 下 effect 会挂载→卸载→重挂载一次。若在 effect 内用闭包函数
  // acceptChallenge/enterPlayingAsInvitee，第二次挂载会拿到旧闭包，导致受邀者收
  // 到 accept 时 phaseRef 仍是旧值。所以这里只做"收件+分发"，把最新状态判断留给
  // ref，实际进入对局走统一的 enterPlaying。
  useEffect(() => {
    presence.start();
    const off = presence.onEvent((e) => {
      if (e.type === "peers") {
        // 服务器模式的名册只信 serverChannel：presence（同源）与服务器名册是
        // 两个写者，互相整表覆盖会让名册来回闪烁（审计顺手修）
        if (!serverMode) setPeers(e.peers.filter((p) => p.id !== tabUser));
      } else if (e.type === "challenge") {
        challengeQueueRef.current.push(e);
        setSignalTick((t) => t + 1);
      } else if (e.type === "accept") {
        acceptQueueRef.current.push(e);
        setSignalTick((t) => t + 1);
      } else if (e.type === "reject") {
        rejectQueueRef.current.push(e);
        setSignalTick((t) => t + 1);
      }
    });
    return () => { off(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabUser]);

  // 消费挑战/同意信件（用最新 state 做判断，不受 StrictMode 闭包影响）。
  // 顺序：先处理 challenge（邀请者自动同意→发出 accept），再处理 accept（受邀者进入对局）。
  useEffect(() => {
    if (signalTick === 0) return;
    const challenges = challengeQueueRef.current;
    challengeQueueRef.current = [];
    for (const e of challenges) {
      if (phaseRef.current === "playing" || (phaseRef.current === "waiting" && roleRef.current === "invitee")) {
        // 对局中/自己也是等待中的受邀者：pwd 已失效或本局已满，回拒绝信——
        // 静默吞掉会让挑战方永远停在"等待对方同意"（审查 A8）
        presence.reject(e.from, e.gameId);
        continue;
      }
      if (phaseRef.current === "waiting" && pwdRef.current && e.pwd === pwdRef.current) {
        // 带正确 pwd 的连接请求：自动同意（邀请钥匙语义）
        acceptChallenge(e.from, e.kind, e.size, e.gameId, true, e.rtcAns ?? null);
      } else if (phaseRef.current === "home") {
        // 主页：无 pwd 或 pwd 不对 → 弹窗手动确认
        setIncoming({ from: e.from, fromName: e.fromName, kind: e.kind, size: e.size, gameId: e.gameId });
      }
      // waiting(inviter) 但 pwd 对不上：第三人拿旧 pwd，回拒绝信
      else if (phaseRef.current === "waiting") {
        presence.reject(e.from, e.gameId);
      }
    }
    const accepts = acceptQueueRef.current;
    acceptQueueRef.current = [];
    for (const e of accepts) {
      const gid = gameIdRef.current;
      if (roleRef.current === "invitee" && gid && e.gameId === gid) {
        enterPlayingAsInvitee();
      }
    }
    const rejects = rejectQueueRef.current;
    rejectQueueRef.current = [];
    for (const e of rejects) {
      const gid = gameIdRef.current;
      if (roleRef.current === "invitee" && gid && e.gameId === gid) {
        showNotice("对方拒绝了对局");
        backHome();
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signalTick]);

  // presence 状态上报
  useEffect(() => {
    if (phase === "home") presence.setStatus("idle", null);
    else if (phase === "waiting") presence.setStatus("waiting", gameId);
    // 观战者不是"对局中"（不占对战席位，可被挑战），对他人显示空闲（审查 D9）
    else presence.setStatus(role === "spectator" ? "idle" : "in-game", gameId);
  }, [phase, gameId, role]);

  // 服务器模式状态上报（与 presence 并行；announce 内部有重连重放）
  useEffect(() => {
    if (!serverMode) return;
    if (phase === "home") serverChannel.announce("idle", null);
    else if (phase === "waiting") serverChannel.announce("waiting", gameId);
    else serverChannel.announce(role === "spectator" ? "idle" : "in-game", gameId);
  }, [phase, gameId, role, serverMode]);

  // 服务器通道生命周期与事件分发。信令语义（pwd 校验/同意弹窗/观战房间管理）
  // 全在客户端解释，服务器只做点对点转发——与「服务器无储存」的承诺一致。
  useEffect(() => {
    if (!serverMode) { if (import.meta.env.DEV) console.log("[srv-effect] skipped, serverMode=false"); return; }
    serverChannel.connectFromSettings();
    serverChannel.onEvent = (e: ServerEvent) => {
      switch (e.t) {
        case "state":
          setServerState(e.s);
          if (e.s === "ready") {
            showNotice(null);
            signaling.flushPendingServerIntents();
          } else if (e.s === "error") {
            showNotice(`服务器连接失败：${e.detail ?? "请检查设置页服务器配置"}`, 3000);
          }
          break;
        case "welcome":
          break;
        case "peers":
          setPeers(e.users
            .filter((u) => u.id !== serverChannel.myServerId)
            .map((u) => ({ id: u.id, name: u.name, status: u.status, gameId: u.gameId, ts: Date.now() })));
          break;
        case "signal":
          signaling.handleSignal(e.from, e.kind, e.payload);
          break;
        case "relayed":
          try { transport.injectRemote(e.payload); } catch { /* ignore */ }
          break;
        case "error":
          showNotice(`服务器：${e.msg}`, 3000);
          break;
      }
    };
    return () => { serverChannel.onEvent = null; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverMode]);

  // 路由变化监听（站内 nav / 前进后退）
  useEffect(() => {
    const onPop = () => setIntent(parseUrl());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  // 启动意图（StrictMode 下：effect 跑两次，但靠 processedIntentRef 只执行一次）
  useEffect(() => {
    const href = window.location.href;
    if (processedIntentRef.current === href) return;
    processedIntentRef.current = href;
    bootRef.current = true;
    processIntent(parseUrl());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 站内路由变化时处理新意图（跳过启动时已处理过的同一 URL，避免重复发起挑战）
  useEffect(() => {
    if (!bootRef.current) return;
    const href = window.location.href;
    if (processedIntentRef.current === href) return;
    processedIntentRef.current = href;
    processIntent(intent);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intent]);

  /* ---------- P2P 直连（邀请链接自动信令，无服务器、无手动输入） ---------- */

  // 广播注入：GameChannel.send 单入口，同一条消息同时发 BroadcastChannel、所有
  // WebRTC 直连，以及（服务器模式）对每个对局相关人的兜底中转；
  // 接收端对 Move 按 (sender, seq) 去重，多链路重复送达只应用一次。
  useEffect(() => {
    wireRtcBroadcast((msg) => {
      for (const q of rtcPeersRef.current) {
        try { q.send(msg); } catch { /* ignore */ }
      }
      if (serverMode && serverChannel.connected) {
        for (const t of relayTargetsRef.current) {
          serverChannel.relay(t, msg);
        }
      }
    });
    return () => wireRtcBroadcast(null);
  }, [serverMode]);

  const moveCount = history.length;
  const myHomeUrl = userToUrl(tabUser);

  const statusText = useMemo(() => {
    if (winner) return `${winner === "black" ? "黑" : "白"} 胜`;
    return `${toMove === "black" ? "黑" : "白"} 落子`;
  }, [winner, toMove]);

  const p2pStatusText = useMemo(() => {
    if (phase === "home") return role === "spectator" ? "观战中" : "主页 · 选择对手或等待被挑战";
    if (phase === "waiting") return `等待对手 · 执${myColor === "black" ? "黑" : "白"}`;
    // 直连未通但有中转目标：服务器模式已可对弈（走兜底中转）
    if (!peerConnected && serverMode && relayTargetsRef.current.size > 0) return `经服务器中转 · 执${myColor === "black" ? "黑" : "白"}`;
    if (!peerConnected) return `连接中 · 执${myColor === "black" ? "黑" : "白"}`;
    return `已直连 · 执${myColor === "black" ? "黑" : "白"}${role === "spectator" ? "（观战）" : toMove === myColor ? " · 轮到你" : " · 等待对手"}`;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, role, peerConnected, myColor, toMove, serverMode]);

  /** 对局卡连接指示灯：绿=可对弈（直连或服务器中转），橙黄=连接未就绪，红=连接中断。 */
  const linkLamp: { color: string; text: string } = connLost
    ? { color: "#b00020", text: "已中断" }
    : phase === "playing" && (peerConnected || (serverMode && relayTargetsRef.current.size > 0))
      ? { color: "#0a7a2e", text: "已连接" }
      : { color: "#FF8C1A", text: "等待对手" };

  const boardDisabled = useMemo(() => {
    if (winner) return true;
    if (role === "spectator") return true;
    if (phase !== "playing") return phase === "waiting";
    if (!peerConnected && !(serverMode && relayTargetsRef.current.size > 0)) return true;
    return toMove !== myColor;
  }, [winner, role, phase, peerConnected, toMove, myColor, serverMode]);

  const mode = intent.mode;
  const viewedUserId = mode === "user" ? intent.userId : null;
  const viewedPeer = viewedUserId && viewedUserId !== tabUser ? peers.find((p) => p.id === viewedUserId) ?? null : null;
  const isSelfPage = viewedUserId === tabUser;

  // 顶部选择器：五子棋/围棋 + 尺寸；对局中（非主页）禁用切换。
  const topLocked = phase !== "home";
  const topLockedTitle = topLocked ? "对局/等待中不可切换，请先取消或离开" : undefined;

  function pickKind(k: GameKind) {
    if (topLocked) return;
    setKind(k);
  }

  function pickSize(s: Size) {
    if (topLocked) return;
    setSize(s);
  }
  return {
    // 状态
    kind, size, board, toMove, winner, lastMove, hover, history,
    intent, tabUser, name, peers, role, phase, gameId, myColor,
    peerConnected, pwd, inviteUrl, watchUrl, incoming, notice,
    answerBackUrl, copyFb, modal, modalInput, modalErr,
    // 服务器模式（可选）
    serverMode, serverState, serverIncoming,
    spectators, specRequests, spectateEnabled, specCanChat,
    specPwd: specPwdRef.current,
    chatLog, peerAvatars, confirmReq,
    // 渲染层直接写状态的 setter
    setModal, setModalInput, setModalErr, setName, setHover,
    // 动作
    submitModal, createInvite, acceptInvite, acceptChallenge, rejectChallenge,
    backHome, copyText, handlePlace, reset, saveName, pickKind, pickSize,
    serverChallengePeer, serverAcceptChallenge, serverRejectChallenge,
    sendChat, requestUndo, requestReset, requestSwap,
    approveSpecRequest, rejectSpecRequest, kickSpectator, muteSpectator,
    disableSpectate, requestSpecChat, confirmApprove, confirmDecline,
    loadMyAvatar, saveMyAvatar,
    // 派生
    moveCount, myHomeUrl, statusText, p2pStatusText, boardDisabled, linkLamp,
    mode, viewedUserId, viewedPeer, isSelfPage,
    topLocked, topLockedTitle, showNotice,
  };
}

/** 页面组件的消费面：pages/* 以整包 session 取值，避免 60 余项逐 props 穿透。 */
export type GameSession = ReturnType<typeof useGameSession>;
