/**
 * sessionContext —— useGameSession 与三个域模块（serverSignaling/negotiation/chat）
 * 之间的共享上下文接口。
 *
 * 字段与 hook 内标识符完全同名：域工厂在顶部一次解构，搬移过去的函数体保持原样。
 * ref 所有权全部留在 useGameSession（specPwdRef 等需从 hook 返回值暴露），
 * 这里只声明类型；唯一例外是 peersRefCache——仅 chat 域使用，归 chat 模块私有。
 */
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { PeerInfo } from "../net/presence";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import type { DirectRtcPeer } from "../net/rtc";
import type { Phase, Role } from "../pages/components";

/** 观战房间名单条目（双方对局者共同维护，经 spec-sync 同步）。 */
export type SpectatorEntry = { id: string; name: string; host: string; muted: boolean };
/** 观战申请（pwd 错/无 → 私有申请，仅被申请的对局者可见，聊天区内处理）。 */
export type SpecRequest = { from: string; fromName: string };
/** 聊天记录条目（对局者 Chat 广播 + 观战者经 host 转发的 spec-chat 统一进这里）。 */
export type ChatEntry = { userId: string; name: string; text: string; ts: number; self: boolean };
/** 协商请求弹窗（悔棋/重开/换棋 + 观战发言批准 + 错钥匙连接询问）。 */
export type ConfirmRequest = { kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd"; from: string; fromName: string };
/** 服务器挑战信（对方点名邀请）：等用户同意/拒绝。 */
export type ServerIncomingMsg = { from: string; fromName: string; kind: GameKind; size: Size };

export type SessionCtx = {
  /* ---------- state 值 ---------- */
  peers: PeerInfo[];
  chatLog: ChatEntry[];
  spectators: SpectatorEntry[];
  serverIncoming: ServerIncomingMsg | null;
  /** 启动时按设置选中项决定（设置页切换后 reload 生效）：none=无服务器，其余=连服务器。 */
  serverMode: boolean;
  tabUser: string;

  /* ---------- state setter ---------- */
  setBoard: Dispatch<SetStateAction<StoneColor[][]>>;
  setChatLog: Dispatch<SetStateAction<ChatEntry[]>>;
  setHistory: Dispatch<SetStateAction<Coord[]>>;
  setHover: Dispatch<SetStateAction<Coord | null>>;
  setLastMove: Dispatch<SetStateAction<Coord | null>>;
  setMyColor: Dispatch<SetStateAction<StoneColor>>;
  setPhase: Dispatch<SetStateAction<Phase>>;
  setRole: Dispatch<SetStateAction<Role>>;
  setToMove: Dispatch<SetStateAction<StoneColor>>;
  setWinner: Dispatch<SetStateAction<StoneColor | null>>;
  setAnswerBackUrl: Dispatch<SetStateAction<string | null>>;
  setConfirmReq: Dispatch<SetStateAction<ConfirmRequest | null>>;
  setGameId: Dispatch<SetStateAction<string | null>>;
  setInviteUrl: Dispatch<SetStateAction<string | null>>;
  setPeerAvatars: Dispatch<SetStateAction<Record<string, string>>>;
  setPeerConnected: Dispatch<SetStateAction<boolean>>;
  setPwd: Dispatch<SetStateAction<string | null>>;
  setServerIncoming: Dispatch<SetStateAction<ServerIncomingMsg | null>>;
  setSpecCanChat: Dispatch<SetStateAction<boolean>>;
  setSpecRequests: Dispatch<SetStateAction<SpecRequest[]>>;
  setSpectateEnabled: Dispatch<SetStateAction<boolean>>;
  setSpectators: Dispatch<SetStateAction<SpectatorEntry[]>>;
  setWatchUrl: Dispatch<SetStateAction<string | null>>;

  /* ---------- refs ---------- */
  boardRef: MutableRefObject<StoneColor[][]>;
  historyRef: MutableRefObject<Coord[]>;
  kindRef: MutableRefObject<GameKind>;
  lastMoveRef: MutableRefObject<Coord | null>;
  myColorRef: MutableRefObject<StoneColor>;
  phaseRef: MutableRefObject<Phase>;
  roleRef: MutableRefObject<Role>;
  sizeRef: MutableRefObject<Size>;
  toMoveRef: MutableRefObject<StoneColor>;
  winnerRef: MutableRefObject<StoneColor | null>;
  gameIdRef: MutableRefObject<string | null>;
  pwdRef: MutableRefObject<string | null>;
  confirmRejectRef: MutableRefObject<(() => void) | null>;
  confirmResolveRef: MutableRefObject<(() => void) | null>;
  inviterRtcRef: MutableRefObject<DirectRtcPeer | null>;
  rtcPeersRef: MutableRefObject<DirectRtcPeer[]>;
  /** 观战者自己连的对局者。 */
  myHostRef: MutableRefObject<string | null>;
  /** userId 链接意图（服务器模式）：连接就绪后发 join/spec-join。 */
  pendingLinkRef: MutableRefObject<{ target: string; pwd: string | null; spec: boolean } | null>;
  /** 服务器模式的兜底中转目标（对手 + 观战者的 s- 短 ID）。 */
  relayTargetsRef: MutableRefObject<Set<string>>;
  /** 服务器模式当前对手的短 ID。relayTargets 的插入序第一个不一定是对手
   *  （观战者可能在等待期先于对手加入），对手判定必须用它（审计 B2）。 */
  opponentRef: MutableRefObject<string | null>;
  /** SyncState 回退纪元：本地悔棋/重开时 +1，随快照广播供接收端双键比较（审计 B1）。 */
  syncEpochRef: MutableRefObject<number>;
  /** 观战者侧：发言批准状态（双 host 均 ack 才可发言；被拒则本局锁死）。 */
  specCanChatRef: MutableRefObject<boolean>;
  /** 观战钥匙（每局生成、整局有效）：服务器模式观战链接的 pwd。 */
  specPwdRef: MutableRefObject<string | null>;
  specChatOkRef: MutableRefObject<Set<string>>;
  specRequestDeniedRef: MutableRefObject<boolean>;
  specRequestsRef: MutableRefObject<SpecRequest[]>;
  /** 观战房间名（双方对局者各维护一份全量名单，经 spec-sync 同步）。 */
  spectatorsRef: MutableRefObject<SpectatorEntry[]>;
  spectateEnabledRef: MutableRefObject<boolean>;

  /* ---------- hook 内函数（被域模块调用） ---------- */
  /** 统一 notice 入口：ms 缺省=常驻显示；传 ms 则到期自动清除。 */
  showNotice: (text: string | null, ms?: number) => void;
  /** 按域传入的 P2P 连接注册（对局者受理观战/建局时新建 peer）。 */
  attachPeer: (p: DirectRtcPeer) => void;
  /** 关闭全部 P2P 连接与棋盘 channel（换局入口与回主页共用）。 */
  closeAllRtcPeers: () => void;
  backHome: () => void;
  resetBoardFor: (k: GameKind, s: Size) => void;
  /** 聊天统一入口（chat 域实现，经 ctx 回填；serverSignaling 与 handleNetMessage 使用）。 */
  pushChat: (userId: string, name: string, text: string, self: boolean) => void;
};
