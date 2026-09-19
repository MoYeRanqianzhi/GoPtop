/**
 * useGameSession —— UI 绑定壳（Rust 化第二阶段落地后的形态）。
 *
 * 一切协议/信令/状态机逻辑都在 goptop-net（Rust，经 goptop-transport 的 wasm
 * 产物运行）：本文件只负责——
 * - 挂载 WasmSession（50ms 事件泵 + goptopOnChange/goptopNotice/goptopCopy 钩子）；
 * - 把快照 JSON 解构为历史 hook 返回形状（App/pages 组件零改动）；
 * - 纯 UI 状态（弹窗/悬停/路由 intent/复制反馈）留在 TS。
 * 逻辑迁移基线与增强项见 crates/goptop-net/src/session/mod.rs 头注释。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import initTransport, { WasmSession } from "../wasm/transport/goptop_transport.js";
import { emptyBoard } from "../game/board";
import { loadDefaults } from "../pages/components";
import type { Phase, Role } from "../pages/components";
import { parseUrl, shareOrigin } from "../net/links";
import type { UrlIntent } from "../net/links";
import { myName } from "../net/identity";
import { storeGet, storeRemove, storeSet } from "../net/store";
import { loadServerSelection } from "../net/servers";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";

type Snap = ReturnType<typeof parseSnap>;

/** 快照 JSON 的宽松类型（字段与 goptop-net session::snapshot 一一对应）。 */
function parseSnap(raw: string) {
  return JSON.parse(raw) as {
    userId: string; peerId: string; name: string; avatar: string | null;
    serverMode: boolean; serverState: string;
    kind: GameKind; size: Size; board: StoneColor[][]; toMove: StoneColor;
    winner: StoneColor | null; history: (Coord | "pass")[]; lastMove: Coord | null;
    moveCount: number; scoring: boolean;
    phase: "home" | "waiting" | "playing"; role: "idle" | "inviter" | "invitee" | "spectator";
    gameId: string | null; myColor: StoneColor;
    peerConnected: boolean; connLost: boolean;
    pwd: string | null; specPwd: string | null; spectateEnabled: boolean;
    inviteUrl: string | null; watchUrl: string | null;
    answerBackUrl: string | null; specUrl: string | null;
    serverIncoming: { from: string; fromName: string; kind: GameKind; size: Size } | null;
    peers: { id: string; name: string; status: string; gameId: string | null }[];
    spectators: { id: string; name: string; host: string; muted: boolean }[];
    specRequests: { from: string; fromName: string }[];
    specCanChat: boolean; specDenied: boolean;
    confirmReq: { kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd" | "score-confirm"; from: string; fromName: string; queued: number } | null;
    chatLog: { userId: string; name: string; text: string; ts: number; self: boolean }[];
    peerAvatars: Record<string, string>;
    myDead: Coord[]; peerDead: Coord[];
    myScoreOk: boolean; peerScoreOk: boolean;
    scoreResult: { black: number; white: number; winner: StoneColor; deadRemoved: number } | null;
  };
}

export function useGameSession() {
  const defs = useMemo(loadDefaults, []);
  /* ---------- wasm 会话挂载 ---------- */
  const [session, setSession] = useState<WasmSession | null>(null);
  const [snap, setSnap] = useState<Snap | null>(null);
  const serverMode = loadServerSelection() !== "none";
  const [intent, setIntent] = useState<UrlIntent>(() => parseUrl());

  useEffect(() => {
    let disposed = false;
    void (async () => {
      await initTransport();
      if (disposed) return;
      const s = new WasmSession(
        JSON.stringify({
          name: myName(),
          serverMode,
          shareOrigin: shareOrigin(),
          kind: defs.kind,
          size: defs.size,
        }),
      );
      (window as unknown as Record<string, unknown>).goptopOnChange = () => setSnap(parseSnap(s.snapshot()));
      (window as unknown as Record<string, unknown>).goptopNotice = (arg: string) => {
        try {
          const { text, ms } = JSON.parse(arg) as { text: string | null; ms: number | null };
          noticeRef.current = { text, ms };
          setNotice(text);
          if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
          noticeTimer.current = null;
          if (text && ms) {
            noticeTimer.current = window.setTimeout(() => {
              setNotice(null);
              noticeTimer.current = null;
            }, ms);
          }
        } catch { /* ignore */ }
      };
      (window as unknown as Record<string, unknown>).goptopCopy = async (arg: string) => {
        try {
          const { text, ok } = JSON.parse(arg) as { text: string; ok: string };
          await navigator.clipboard.writeText(text);
          setCopyFb(ok);
          setTimeout(() => setCopyFb(null), 1600);
        } catch {
          setCopyFb("复制失败，请手动复制");
          setTimeout(() => setCopyFb(null), 2000);
        }
      };
      s.start_pump();
      setSession(s);
      setSnap(parseSnap(s.snapshot()));
      // E2E/调试钩子：dump 快照用（生产无副作用）。
      (window as unknown as Record<string, unknown>).__session = s;
    })();
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ---------- 纯 UI 状态（不进状态机） ---------- */
  const [hover, setHover] = useState<Coord | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimer = useRef<number | null>(null);
  const noticeRef = useRef<{ text: string | null; ms: number | null } | null>(null);
  const [copyFb, setCopyFb] = useState<string | null>(null);
  /** 昵称输入缓冲（受控 input；保存才提交 Rust 并清缓冲）。 */
  const [nameDraft, setNameDraft] = useState<string | null>(null);
  type ModalKind = "paste-invite" | "paste-answer" | "receipt";
  const [modal, setModal] = useState<ModalKind | null>(null);
  const [modalInput, setModalInput] = useState("");
  const [modalErr, setModalErr] = useState<string | null>(null);

  /* ---------- 路由（UI 层：popstate → parseUrl；Rust 只管连接意图与导航决策） ---------- */
  useEffect(() => {
    const onPop = () => setIntent(parseUrl());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  /* ---------- 便捷命令封装 ---------- */
  const cmd = useCallback(
    (f: (s: WasmSession) => void) => {
      if (session) f(session);
    },
    [session],
  );

  /* ---------- 派生（对齐旧 hook 公式） ---------- */
  const s = snap;
  const board = s?.board ?? emptyBoard(defs.size);
  const history = s?.history ?? [];
  const kind = s?.kind ?? defs.kind;
  const size = s?.size ?? defs.size;
  const toMove = s?.toMove ?? "black";
  const winner = s?.winner ?? null;
  const lastMove = s?.lastMove ?? null;
  const phase: Phase = s?.phase ?? "home";
  const role: Role = s?.role ?? "idle";
  const myColor = s?.myColor ?? "black";
  const moveCount = s?.moveCount ?? 0;
  const tabUser = s?.userId ?? "";
  const name = nameDraft ?? s?.name ?? "";
  const peers = useMemo(
    () => (s?.peers ?? []).map((p) => ({ ...p, status: (p.status === "waiting" || p.status === "in-game" ? p.status : "idle") as "idle" | "waiting" | "in-game", ts: Date.now() })),
    [s],
  );
  const chatLog = s?.chatLog ?? [];
  const peerAvatars = s?.peerAvatars ?? {};
  const spectators = s?.spectators ?? [];
  const specRequests = s?.specRequests ?? [];
  const confirmReq = s?.confirmReq ?? null;
  const incoming = s?.serverIncoming ?? null;
  const peerConnected = s?.peerConnected ?? false;

  /** 服务器模式的兜底中转可用性：直连未通但有 relay 目标（快照以 opponents/spec推导）。 */
  const relayTargets = useMemo(() => {
    if (!serverMode || !s) return false;
    return s.serverState === "ready" && (phase === "waiting" || phase === "playing");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverMode, s, phase]);

  const p2pStatusText = useMemo(() => {
    const color = myColor === "black" ? "黑" : "白";
    if (phase === "home") return role === "spectator" ? "观战中" : "主页 · 选择对手或等待被挑战";
    if (phase === "waiting") return `等待对手 · 执${color}`;
    if (!peerConnected && relayTargets) return `经服务器中转 · 执${color}`;
    if (!peerConnected) return `连接中 · 执${color}`;
    const turn = role === "spectator" ? "（观战）" : toMove === myColor ? " · 轮到你" : " · 等待对手";
    return `已直连 · 执${color}${turn}`;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, role, peerConnected, myColor, toMove, relayTargets]);

  const linkLamp: { color: string; text: string } = useMemo(() => {
    if (s?.connLost) return { color: "#b00020", text: "已中断" };
    if (phase === "playing" && (peerConnected || relayTargets)) return { color: "#0a7a2e", text: "已连接" };
    return { color: "#FF8C1A", text: "等待对手" };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s?.connLost, phase, peerConnected, relayTargets]);

  const boardDisabled = useMemo(() => {
    if (winner || role === "spectator") return true;
    if (phase !== "playing") return phase === "waiting";
    if (!peerConnected && !relayTargets) return true;
    return toMove !== myColor;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [winner, role, phase, peerConnected, toMove, myColor, relayTargets]);

  const statusText = useMemo(() => {
    if (winner) return `${winner === "black" ? "黑" : "白"} 胜`;
    return `${toMove === "black" ? "黑" : "白"} 落子`;
  }, [winner, toMove]);

  const myHomeUrl = tabUser ? `${shareOrigin().replace(/\/$/, "")}/${encodeURIComponent(tabUser)}` : "";
  const topLocked = phase !== "home";
  const topLockedTitle = topLocked ? "对局/等待中不可切换，请先取消或离开" : undefined;
  const mode = intent.mode;
  const viewedUserId = mode === "user" ? intent.userId : null;
  const viewedPeer = viewedUserId && viewedUserId !== tabUser ? peers.find((p) => p.id === viewedUserId) ?? null : null;
  const isSelfPage = viewedUserId === tabUser;

  function pickKind(k: GameKind) {
    if (topLocked) return;
    cmd((s2) => s2.pick_kind(k));
    // 主页棋盘重置由 Rust 状态机处理（pick_kind 内部 new_game）。
  }
  function pickSize(sz: Size) {
    if (topLocked) return;
    cmd((s2) => s2.pick_size(sz));
  }
  function saveName() {
    const trimmed = (nameDraft ?? name).trim();
    cmd((s2) => s2.set_name(trimmed));
    setNameDraft(null);
  }

  /* ---------- 弹窗提交（解析用 Rust 导出，分派在 TS） ---------- */
  async function submitModal() {
    const m = modal;
    if (!m || !session) return;
    if (m === "paste-invite") {
      const parsed = JSON.parse(session.parse_link(modalInput)) as { ok: boolean; intent?: { mode: string; userId?: string; pwd?: string | null; kind?: GameKind; size?: Size; rtc?: string | null; spec?: boolean } };
      if (!parsed.ok || !parsed.intent) {
        setModalErr("无法识别该链接：请完整粘贴邀请链接或主页链接");
        return;
      }
      const it = parsed.intent;
      if (it.mode === "user" && it.userId) {
        if (phase !== "home" && phase !== "waiting") {
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
        // spec 标志必须一起透传：观战链接（spec=1）走观战通道，否则会被当成对局 join
        // 而被房主按「对局中」拒绝，粘贴观战链接直接无效。
        session.accept_invite(it.userId, it.pwd ?? null, it.kind ?? "gomoku", it.size ?? 15, it.rtc ?? null, it.spec ?? false);
        return;
      }
      if (it.mode === "watch" && "gameId" in it) {
        if (phase !== "home") {
          setModalErr("正在对局中，请先离开再观战");
          return;
        }
        setModalInput("");
        setModalErr(null);
        setModal(null);
        // 同源观战走 Rust Boot 等价意图：直接导航到 /watch/<id> 由 Boot 处理。
        window.history.pushState(null, "", `/watch/${encodeURIComponent(it.gameId as string)}`);
        window.dispatchEvent(new PopStateEvent("popstate"));
        return;
      }
      setModalErr("该链接是本站页面链接，不是邀请链接");
      return;
    }
    if (m === "paste-answer") {
      const parsed = JSON.parse(session.parse_answer(modalInput)) as { ok: boolean; answer?: unknown };
      if (!parsed.ok || !parsed.answer) {
        setModalErr("无法识别该回执：请完整粘贴受邀者发来的回执链接（含 rtcAns）");
        return;
      }
      setModalInput("");
      setModalErr(null);
      setModal(null);
      session.accept_receipt(JSON.stringify(parsed.answer));
    }
  }

  return {
    // 状态（快照直通）
    kind, size, board, toMove, winner, lastMove, hover, history,
    intent, tabUser, name, peers, role, phase, gameId: s?.gameId ?? null, myColor,
    peerConnected, pwd: s?.pwd ?? null, inviteUrl: s?.inviteUrl ?? null,
    watchUrl: s?.watchUrl ?? null, incoming, notice,
    specUrl: s?.specUrl ?? null,
    answerBackUrl: s?.answerBackUrl ?? null, copyFb, modal, modalInput, modalErr,
    // 服务器模式（可选）
    serverMode, serverState: (s?.serverState ?? (serverMode ? "connecting" : "off")) as "off" | "connecting" | "ready" | "error",
    serverIncoming: s?.serverIncoming ?? null,
    spectators, specRequests, spectateEnabled: s?.spectateEnabled ?? true, specCanChat: s?.specCanChat ?? false,
    specPwd: s?.specPwd ?? null,
    chatLog, peerAvatars, confirmReq,
    scoring: s?.scoring ?? false,
    myDead: s?.myDead ?? [], peerDead: s?.peerDead ?? [],
    myScoreOk: s?.myScoreOk ?? false, peerScoreOk: s?.peerScoreOk ?? false,
    scoreResult: s?.scoreResult ?? null,
    // 渲染层直接写状态的 setter
    setModal, setModalInput, setModalErr, setName: setNameDraft, setHover,
    // 动作（转 wasm 命令）
    submitModal,
    createInvite: () => cmd((s2) => s2.create_invite()),
    acceptInvite: (inviterId: string, pwdOrNull: string | null, k: GameKind, sz: Size, rtc?: string | null) =>
      cmd((s2) => s2.accept_invite(inviterId, pwdOrNull, k, sz, rtc ?? null, false)),
    acceptChallenge: () => cmd((s2) => s2.accept_challenge()),
    rejectChallenge: () => cmd((s2) => s2.reject_challenge()),
    backHome: () => cmd((s2) => s2.back_home()),
    copyText: async (t: string, okMsg: string) => {
      try {
        await navigator.clipboard.writeText(t);
        setCopyFb(okMsg);
        setTimeout(() => setCopyFb(null), 1600);
      } catch {
        setCopyFb("复制失败，请手动复制");
        setTimeout(() => setCopyFb(null), 2000);
      }
    },
    handlePlace: (c: Coord) => cmd((s2) => s2.place(c.x, c.y)),
    handlePass: () => cmd((s2) => s2.pass()),
    handleResign: () => cmd((s2) => s2.resign()),
    toggleDead: (c: Coord) => cmd((s2) => s2.toggle_dead(c.x, c.y)),
    confirmScore: () => cmd((s2) => s2.confirm_score()),
    reset: () => {
      // 本地对战：直接重开（broadcast Reset）；P2P：对方同意制。
      if (serverMode && (role === "inviter" || role === "invitee")) {
        cmd((s2) => s2.request_reset());
        return;
      }
      if (phase === "playing" && !serverMode) {
        cmd((s2) => s2.request_reset());
        return;
      }
      // 主页：重置本地规则（Rust pick_kind 内部已重置；这里触发重置等价）。
      cmd((s2) => s2.pick_kind(kind));
    },
    saveName, pickKind, pickSize,
    serverChallengePeer: (to: string) => cmd((s2) => s2.server_challenge(to)),
    serverAcceptChallenge: () => cmd((s2) => s2.server_accept_challenge()),
    serverRejectChallenge: () => cmd((s2) => s2.server_reject_challenge()),
    sendChat: (text: string) => cmd((s2) => s2.send_chat(text)),
    requestUndo: () => cmd((s2) => s2.request_undo()),
    requestReset: () => cmd((s2) => s2.request_reset()),
    requestSwap: () => cmd((s2) => s2.request_swap()),
    approveSpecRequest: (id: string) => cmd((s2) => s2.approve_spec(id)),
    rejectSpecRequest: (id: string) => cmd((s2) => s2.reject_spec(id)),
    kickSpectator: (id: string) => cmd((s2) => s2.kick_spec(id)),
    muteSpectator: (id: string, muted: boolean) => cmd((s2) => s2.mute_spec(id, muted)),
    disableSpectate: () => cmd((s2) => s2.disable_spectate()),
    requestSpecChat: () => cmd((s2) => s2.request_spec_chat()),
    confirmApprove: () => cmd((s2) => s2.confirm_approve()),
    confirmDecline: () => cmd((s2) => s2.confirm_decline()),
    acceptSpecReceipt: (r: unknown) => cmd((s2) => s2.accept_spec_receipt(JSON.stringify(r))),
    loadMyAvatar: () => {
      try {
        return storeGet("goptop:avatar");
      } catch {
        return null;
      }
    },
    saveMyAvatar: (dataUrl: string | null) => {
      try {
        if (dataUrl) storeSet("goptop:avatar", dataUrl);
        else storeRemove("goptop:avatar");
      } catch { /* ignore */ }
      cmd((s2) => s2.set_avatar(dataUrl));
    },
    // 派生
    moveCount, myHomeUrl, statusText, p2pStatusText, boardDisabled, linkLamp,
    mode, viewedUserId, viewedPeer, isSelfPage,
    topLocked, topLockedTitle, showNotice: (text: string | null, ms?: number) => {
      setNotice(text);
      if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
      noticeTimer.current = null;
      if (text && ms) {
        noticeTimer.current = window.setTimeout(() => {
          setNotice(null);
          noticeTimer.current = null;
        }, ms);
      }
    },
  };
}

/** 页面组件的消费面：pages/* 以整包 session 取值，避免 60 余项逐 props 穿透。 */
export type GameSession = ReturnType<typeof useGameSession>;
