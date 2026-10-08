/**
 * Agent 对战页 `/agent` —— 唯一入口（计划 2026-10-08-agent-battle.md「AgentPage」节）。
 *
 * 对面坐的是「真思考、会犯错」的模型玩家：内置（应用配置的 LLM 循环驱动 B 席）或
 * MCP（外部 Agent 经内嵌 MCP 服务器认领席位，仅桌面）。与 `/ai` 的算法 AI 是两回事
 * ——Agent 等同玩家，拥有完整操作面（看盘、落子、聊天、悔棋请求/批复、认输、计分）。
 *
 * 会话拓扑（本页与其它页最大的不同）：
 * - 主会话归 useGameSession，本页另建**专用会话 A'**（固定 serverMode:false）——
 *   经 net/session 门面 createSession 的 onChange 注入自管 poll（window.goptopOnChange
 *   是单槽，A' 再挂上去会把主会话的订阅顶掉）。
 * - A' 经 `agent_bind` 登记给后端 AgentHub（配对目标 + 拦截面豁免），`agent_start`
 *   起 B 席并在后端结对。方向随执子：我执黑=A' 先邀请（bind 登记时后端代发
 *   create_invite，链含 rtc 后再 agent_start 交 Hub）；我执白=B 先邀请
 *  （`agent_start` 先行，邀请链接经 `agent_status` 的 detail 送回，A' 以 Boot
 *   路径携链创建）。
 * - 本页渲染 A' 的棋盘/聊天/协商横幅，另加 Agent 状态卡（agent_status + agent_events）。
 *
 * 布局（与 P2P 页的「棋盘栈 + 聊天停靠栏」同构）：本组件挂在 App 的 game-layout 下
 * （不经主 play-stack）——对局栈只装棋盘整组，聊天与 Agent 状态卡走侧栏；侧栏放不下
 * 时退聊天弹窗（同 P2P 的实测口径，阈值 240px）。聊天/状态若塞进对局栈，App 的
 * --stack-max 实测会把棋盘挤塌（固定高卡片吃光纵向预算的正反馈）。
 *
 * Tauri 命令契约（与 src-tauri/src/agent.rs 逐字一致，F/G 两路同源）：
 *   agent_start(cfgJson)->u32 / agent_stop(id) / agent_status(id)->JSON /
 *   agent_events(id, since)->JSON / agent_bind(sessionId) / agent_llm_test()->string /
 *   agent_mcp_set(enabled)->JSON / agent_mcp_info()->JSON
 *
 * 平台矩阵（阶段⑤起）：**桌面原生路**——Agent 局走 src-tauri 的原生会话表与
 * AgentHub；**Web 内置路**——A' 走 wasm 的 detached 会话（createDetachedSession）、
 * 命令走 wasm 导出的 agent_*（webAgentCall），B 席与循环在 wasm 侧 Hub。web 后端
 * 可用由 webAgentAvailable 判定（产物带 agent 导出 + Vfs 存储钩子已装），不满足
 * （鸿蒙壳 / 产物未带 agent）整页横幅降级。MCP 依赖内嵌 TCP 服务器，**仅桌面**。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { MessageCircle } from "lucide-react";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import { emptyBoard } from "../game/board";
import { isTauri, nav, shareOrigin } from "../net/links";
import { createDetachedSession, createSession, webAgentAvailable, webAgentCall } from "../net/session";
import type { GameSession } from "../net/session";
import { agentVfsReady } from "../net/agentVfs";
import { myName } from "../net/identity";
import { storeGet, storeSet, storeSetAsync } from "../net/store";
import { ConfirmBanner } from "../components/ConfirmBanner";
import { BoardPanel, ChatPanel } from "./components";

/** 前端界面语言（全仓文案中文、无 i18n 层）；agent_start 的 uiLang 取它，
 *  后端把「回复语言未配置」时的默认锚在这上面。 */
const UI_LANG = "简体中文";

/** 设置持久化键（全部经 net/store 门面落平台存储；Rust 侧经同一份文件读）。 */
const KEY_LLM_CONFIG = "goptop:llm-config";
const KEY_LLM_KEY = "goptop:llm-key";
const KEY_CTX_LIMIT = "goptop:agent-ctx-limit";
const KEY_MCP_ENABLED = "goptop:agent-mcp-enabled";
const KEY_MCP_PORT = "goptop:agent-mcp-port";
const KEY_MCP_TOKEN = "goptop:agent-mcp-token";

/* ============================ 纯逻辑（AgentPage.test.ts 直测） ============================ */

export type AgentDriver = "builtin" | "mcp";
export type LlmProtocol = "anthropic" | "openai-responses" | "openai-chat";

/** `agent_start` 的 cfg 契约（与 Rust 侧逐字一致；agentName 空 = null 走后端默认「Agent」）。 */
export function buildStartCfg(input: {
  driver: AgentDriver;
  kind: GameKind;
  size: Size;
  myColor: "black" | "white";
  agentName: string;
  uiLang: string;
}): string {
  const name = input.agentName.trim();
  return JSON.stringify({
    driver: input.driver,
    kind: input.kind,
    size: input.size,
    myColor: input.myColor,
    agentName: name ? name : null,
    uiLang: input.uiLang,
  });
}

/** 从 `agent_status` 的 detail 里提取可 Boot 的邀请链接（我执白方向：B 先出链，
 *  链接经 detail 送回前端）。就绪判据与 goptop-agent pair.rs 一致：**必须含 rtc=**
 *  ——没有 rtc 说明 offer 还没编进链接，Boot 进去也连不上。 */
export function extractInviteLink(detail: string | null | undefined): string | null {
  if (!detail) return null;
  const candidates = detail.match(/https?:\/\/[^\s"'<>]+/g) ?? [];
  return candidates.find((u) => u.includes("rtc=")) ?? null;
}

/** 上下文上限 clamp [8k, 1M]（与 goptop-agent LoopConfig 同一口径）；
 *  空串/非数值回默认（空输入框 = 未设置，不能当 0 压到下限）。 */
export const DEFAULT_CTX_LIMIT = 176_000;
export function clampCtxLimit(raw: string | number | null | undefined): number {
  if (raw === null || raw === undefined) return DEFAULT_CTX_LIMIT;
  if (typeof raw === "string" && !raw.trim()) return DEFAULT_CTX_LIMIT;
  const n = Math.floor(typeof raw === "number" ? raw : Number(raw));
  if (!Number.isFinite(n)) return DEFAULT_CTX_LIMIT;
  return Math.min(1_000_000, Math.max(8_000, n));
}

export type LlmConfig = {
  protocol: LlmProtocol;
  baseUrl: string;
  model: string;
  maxOutputTokens: number;
  /** 回复语言（任意字符串，不校验）；空 = 跟随界面语言（agent_start 的 uiLang）。 */
  replyLang: string;
  /** delegate 子代理开关（默认关）。 */
  enableSubagent: boolean;
};

export const DEFAULT_LLM_CONFIG: LlmConfig = {
  protocol: "anthropic",
  baseUrl: "",
  model: "",
  maxOutputTokens: 1024,
  replyLang: "",
  enableSubagent: false,
};

/** 存储里的 LLM 配置解析（坏/缺字段逐项回默认——设置半截写入不能把表单打挂）。 */
export function normalizeLlmConfig(raw: string | null): LlmConfig {
  if (!raw) return { ...DEFAULT_LLM_CONFIG };
  try {
    const d = JSON.parse(raw) as Partial<LlmConfig>;
    return {
      protocol: d.protocol === "openai-responses" || d.protocol === "openai-chat" ? d.protocol : "anthropic",
      baseUrl: typeof d.baseUrl === "string" ? d.baseUrl : "",
      model: typeof d.model === "string" ? d.model : "",
      maxOutputTokens:
        Number.isFinite(Number(d.maxOutputTokens)) && Number(d.maxOutputTokens) > 0
          ? Math.floor(Number(d.maxOutputTokens))
          : DEFAULT_LLM_CONFIG.maxOutputTokens,
      replyLang: typeof d.replyLang === "string" ? d.replyLang : "",
      enableSubagent: d.enableSubagent === true,
    };
  } catch {
    return { ...DEFAULT_LLM_CONFIG };
  }
}

/** BoardPanel 的 disabled 派生（与 useGameSession 的 boardDisabled 公式逐项一致；
 *  A' 固定无服务器，没有 relayTargets 分支）。 */
export function agentBoardDisabled(s: {
  winner: StoneColor | null;
  role: string;
  phase: string;
  peerConnected: boolean;
  toMove: StoneColor;
  myColor: "black" | "white";
}): boolean {
  if (s.winner || s.role === "spectator") return true;
  if (s.phase !== "playing") return s.phase === "waiting";
  if (!s.peerConnected) return true;
  return s.toMove !== s.myColor;
}

/** A' 快照的宽松解析（字段与 goptop-net session::snapshot 对应，只取本页用的）；
 *  坏文本/"null" 回 null（会话未挂起/创建中）。 */
export type AgentSnap = {
  kind: GameKind;
  size: Size;
  board: StoneColor[][];
  toMove: StoneColor;
  winner: StoneColor | null;
  lastMove: Coord | null;
  moveCount: number;
  phase: "home" | "waiting" | "playing";
  role: "idle" | "inviter" | "invitee" | "spectator";
  myColor: StoneColor;
  peerConnected: boolean;
  scoring: boolean;
  inviteUrl: string | null;
  myDead: Coord[];
  peerDead: Coord[];
  confirmReq: { kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd" | "score-confirm"; from: string; fromName: string; queued: number } | null;
  chatLog: { userId: string; name: string; text: string; ts: number; self: boolean }[];
  peerAvatars: Record<string, string>;
  spectators: { id: string; name: string; host: string; muted: boolean }[];
  specRequests: { from: string; fromName: string }[];
  specCanChat: boolean;
  spectateEnabled: boolean;
};

export function parseAgentSnap(raw: string | null | undefined): AgentSnap | null {
  if (!raw || raw === "null") return null;
  try {
    return JSON.parse(raw) as AgentSnap;
  } catch {
    return null;
  }
}

/** `agent_status` 回执（契约见文件头）。 */
export type AgentStatus = {
  state: "pairing" | "waiting_mcp" | "thinking" | "waiting" | "done" | "error";
  detail: string | null;
  stagedMove: Coord | null;
  llmCalls: number;
  tokensIn: number;
  tokensOut: number;
  compactions: number;
};

export const EMPTY_STATUS: AgentStatus = {
  state: "pairing",
  detail: null,
  stagedMove: null,
  llmCalls: 0,
  tokensIn: 0,
  tokensOut: 0,
  compactions: 0,
};

/** 状态卡的人话标签（未知值原样透出——后端加了新状态也不能显示成空白）。 */
export function agentStateLabel(state: string): string {
  switch (state) {
    case "pairing": return "配对中";
    case "waiting_mcp": return "等待 MCP Agent 接入";
    case "thinking": return "Agent 思考中";
    case "waiting": return "等待你的行动";
    case "done": return "已终局";
    case "error": return "出错";
    default: return state;
  }
}

/** `agent_events` 的一条工具日志。 */
export type AgentEventItem = { ts: number; tool: string; ok: boolean; ms: number; summary: string };

/** 日志行的操作式排版（pi / claude code 风格）：Read(/game/board)、Write(/memory/x)、
 *  Submit(落子 (7,7))。llm HTTP 行不进日志（是噪音，次数在统计行——过滤在渲染处）；
 *  耗时也不上屏（数据里在）。失败一律缀 ×。 */
const OP_LABEL: Record<string, string> = {
  read: "Read",
  write: "Write",
  edit: "Edit",
  grep: "Grep",
  wait_events: "Wait",
  delegate: "Delegate",
  game_start: "Start",
  game_leave: "Leave",
};
export function formatAgentEvent(e: AgentEventItem): string {
  const op = OP_LABEL[e.tool];
  const mark = e.ok ? "" : " ×";
  if (op) return `${op}(${e.summary})${mark}`;
  return `Submit(${e.summary})${mark}`;
}

/** .mcp.json 连接串（一键复制给外部 MCP 客户端）。 */
export function buildMcpJson(url: string, token: string): string {
  return JSON.stringify(
    { mcpServers: { goptop: { url, headers: { Authorization: `Bearer ${token}` } } } },
    null,
    2,
  );
}

/** MCP Bearer token 生成（CSPRNG base36，同 identity.genPwd 手法；仅运行时点击触发）。 */
export function genMcpToken(): string {
  const buf = new Uint32Array(4);
  crypto.getRandomValues(buf);
  return Array.from(buf, (n) => n.toString(36)).join("").slice(0, 32);
}

/** Tauri 命令调用。仅桌面壳可达；失败带原文上抛（调用方决定是否可容忍）。 */
export async function agentInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<T>(cmd, args ?? {});
}

/** 页面启用判定（阶段⑤ Web 启用翻转的核心布尔）：桌面原生路或 web 后端路，
 *  其一就绪即整页可用；降级横幅只在两头都不可用时出现（鸿蒙壳 / wasm 产物
 *  未带 agent）。设置卡控件、开始按钮、start 守卫全部吃 `enabled`。 */
export function agentPageMode(native: boolean, webOk: boolean): { enabled: boolean; banner: boolean } {
  return { enabled: native || webOk, banner: !native && !webOk };
}

/** web 通道的位置参数表（契约 §5.3.1 命令映射，与 wasm 导出签名逐字对应）。
 *  桌面 invoke 吃命名参数对象，wasm 导出吃位置参数——映射只此一处。MCP 两命令
 *  仅桌面（web 连卡都不渲染），默认臂永远不该被走到，折空参让它撞导出缺失的
 *  error 回执而非静默。 */
export function agentCmdArgs(cmd: string, args: Record<string, unknown>): unknown[] {
  switch (cmd) {
    case "agent_start": return [args.cfgJson];
    case "agent_stop":
    case "agent_status": return [args.id];
    case "agent_events": return [args.id, args.since];
    case "agent_bind": return [args.sessionId];
    case "agent_llm_test": return [];
    default: return [];
  }
}

/** web 回执 → 桌面 invoke 等价值（两边形状契约不同，归一后组件逻辑只有一份）：
 * - agent_start：`{"ok":true,"id":N}` → N；`{"ok":false,"error"}` → 上抛（同
 *   桌面 Result 的 reject）；
 * - agent_bind：`{"ok":true}` → resolve，否则上抛；
 * - agent_status / agent_events：成功回执与桌面逐字同形，原串透传；被折成
 *   `{"ok":false,"error"}` 的失败上抛（run 不存在等，同桌面 reject 语义）；
 * - agent_llm_test：桌面契约是「"ok"=通，其余原样展示的人话文本」——web 导出
 *   内部异常被折成 `{"ok":false,"error"}` 时把 error 抽出来当文本，其余原样；
 * - agent_stop：回执无消费者，原样。
 */
export function parseAgentReply<T>(cmd: string, raw: string): T {
  const folded = ((): { ok: false; error: string } | null => {
    try {
      const d = JSON.parse(raw) as { ok?: unknown; error?: unknown };
      if (d && typeof d === "object" && d.ok === false && typeof d.error === "string") {
        return { ok: false, error: d.error };
      }
    } catch { /* 非 JSON（agent_llm_test 的人话文本走这里） */ }
    return null;
  })();
  if (folded) {
    if (cmd === "agent_llm_test") return folded.error as T;
    throw new Error(folded.error);
  }
  if (cmd === "agent_start") {
    const d = JSON.parse(raw) as { id: number };
    if (typeof d.id !== "number") throw new Error(`agent_start 回执缺 id: ${raw}`);
    return d.id as T;
  }
  if (cmd === "agent_bind") {
    try {
      const d = JSON.parse(raw) as { ok?: unknown };
      if (!(d && typeof d === "object" && d.ok === true)) throw new Error(`agent_bind 回执异常: ${raw}`);
    } catch {
      throw new Error(`agent_bind 回执异常: ${raw}`);
    }
    return undefined as T;
  }
  return raw as T;
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/* ============================ 页面组件 ============================ */

type Setup = {
  kind: GameKind;
  size: Size;
  myColor: "black" | "white";
  driver: AgentDriver;
  agentName: string;
};

type RunPhase = "setup" | "pairing" | "playing";

const inputStyle = { border: "3px solid var(--ink)", padding: "6px 9px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" } as const;

export function AgentPage() {
  const native = useMemo(() => isTauri(), []);
  /** web 后端（wasm agent 导出 + Vfs 钩子）就绪标记：挂载时异步评估一次。
   *  桌面恒 false（走原生路，探测纯浪费一次 wasm 装载）；鸿蒙壳由判定内部排除。 */
  const [webOk, setWebOk] = useState(false);
  /* ---------- 设置卡 ---------- */
  const [setup, setSetup] = useState<Setup>({ kind: "gomoku", size: 15, myColor: "black", driver: "builtin", agentName: "" });
  const [llm, setLlm] = useState<LlmConfig>(() => normalizeLlmConfig(storeGet(KEY_LLM_CONFIG)));
  const [llmKey, setLlmKey] = useState<string>(() => storeGet(KEY_LLM_KEY) ?? "");
  const [ctxLimit, setCtxLimit] = useState<string>(() => storeGet(KEY_CTX_LIMIT) ?? String(DEFAULT_CTX_LIMIT));
  const [llmTest, setLlmTest] = useState<{ busy: boolean; ok: boolean; text: string | null }>({ busy: false, ok: false, text: null });
  const [mcpEnabled, setMcpEnabled] = useState<boolean>(() => storeGet(KEY_MCP_ENABLED) === "true");
  const [mcpPort, setMcpPort] = useState<string>(() => storeGet(KEY_MCP_PORT) || "9537");
  const [mcpToken, setMcpToken] = useState<string>(() => storeGet(KEY_MCP_TOKEN) || "");
  const [mcpInfo, setMcpInfo] = useState<{ enabled: boolean; url: string | null; token: string | null } | null>(null);
  /* ---------- 运行态 ---------- */
  const [phase, setPhase] = useState<RunPhase>("setup");
  const [aPrime, setAPrime] = useState<GameSession | null>(null);
  const [snap, setSnap] = useState<AgentSnap | null>(null);
  const [status, setStatus] = useState<AgentStatus>(EMPTY_STATUS);
  const [events, setEvents] = useState<AgentEventItem[]>([]);
  const [logOpen, setLogOpen] = useState(true);
  const logRef = useRef<HTMLDivElement | null>(null);
  const [err, setErr] = useState<string | null>(null);
  /** 过程提示（停止成功等中性信息；err 专司失败，红色）。 */
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [chatOpen, setChatOpen] = useState(true);
  const [hover, setHover] = useState<Coord | null>(null);
  const [copyFb, setCopyFb] = useState<string | null>(null);

  /** 句柄/游标走 ref：agent_stop 与 dispose 必须能拿到**当前**值（unmount 时 state 闭包会过期）。 */
  const runIdRef = useRef<number | null>(null);
  const aPrimeRef = useRef<GameSession | null>(null);
  const sinceRef = useRef(0);
  const disposedRef = useRef(false);

  /** Agent 命令双通道（契约 §5.3.1）：桌面走 Tauri invoke（原路，零变化）；web
   *  走 wasm 导出。通道只看 native（useMemo([]) 稳定）——unmount cleanup 捕获的
   *  首帧闭包因此安全。web 回执经 parseAgentReply 归一成桌面等价值后，组件其余
   *  逻辑对通道无感。 */
  async function agentCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    if (native) return agentInvoke<T>(cmd, args);
    const raw = await webAgentCall(cmd, agentCmdArgs(cmd, args ?? {}));
    return parseAgentReply<T>(cmd, raw);
  }

  /** 拆局：agent_stop（后端先 resign 再终止并清理）+ 释放 A'，全部容错——
   *  停止路径是收尾动作，后端已结束/壳重启时失败不该打断 UI 复位。 */
  async function teardown(errText: string | null) {
    const id = runIdRef.current;
    runIdRef.current = null;
    if (id !== null) {
      try {
        await agentCall("agent_stop", { id });
      } catch {
        /* 已结束 / 壳重启：忽略 */
      }
    }
    const s = aPrimeRef.current;
    aPrimeRef.current = null;
    if (s) await s.dispose();
    if (disposedRef.current) return;
    setAPrime(null);
    setSnap(null);
    setStatus(EMPTY_STATUS);
    setEvents([]);
    sinceRef.current = 0;
    setHover(null);
    setChatOpen(true);
    setPhase("setup");
    setErr(errText);
    setNotice(null);
  }

  useEffect(() => {
    // StrictMode 会跑 cleanup 再重挂：复位标记，别让第一次 cleanup 的
    // 「已卸载」状态泄漏给第二次挂载（start() 的 in-flight 守卫靠它）。
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      const id = runIdRef.current;
      // 卸载路径不 await：组件已亡，fire-and-forget 即可
      if (id !== null) void agentCall("agent_stop", { id }).catch(() => {});
      void aPrimeRef.current?.dispose();
    };
  }, []);

  /* ---------- web 后端探测（仅非桌面）：先等 Vfs 镜像从 IndexedDB 装载完毕
      （契约：agentVfsReady 之后才许 agent_start），再评 webAgentAvailable 四道
      判定。装载失败不拦判定——钩子仍在，只是没有持久记忆，webOk 照常成立。 ---------- */
  useEffect(() => {
    if (native) return;
    let dead = false;
    void agentVfsReady().catch(() => {}).then(() => {
      if (dead) return;
      void webAgentAvailable().then((ok) => {
        if (!dead) setWebOk(ok);
      });
    });
    return () => {
      dead = true;
    };
  }, [native]);

  /* ---------- MCP 连接卡：进页拉一次服务器状态（仅桌面；老壳无此命令则静默） ---------- */
  useEffect(() => {
    if (!native) return;
    let dead = false;
    void agentInvoke<string>("agent_mcp_info")
      .then((raw) => {
        if (dead) return;
        try {
          setMcpInfo(JSON.parse(raw) as { enabled: boolean; url: string | null; token: string | null });
        } catch {
          /* 非 JSON（壳未带 agent.rs）：连接卡以本地存储为准 */
        }
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [native]);

  /* ---------- 侧栏宽度实测（同 App 的 measureChat 口径）：对局栈旁放得下就走停靠栏，
      放不下（<240）退聊天弹窗。--chat-w 归 App 管（写 main 元素），本页只量自己的
      停靠宽度并直接以 px 落样式——两套测量互不写同一变量，不会互相覆盖。 ---------- */
  const stackRef = useRef<HTMLDivElement | null>(null);
  const [dockW, setDockW] = useState<number | null>(null);
  useEffect(() => {
    if (phase === "setup") {
      setDockW(null);
      return;
    }
    const stack = stackRef.current;
    const layout = stack?.parentElement;
    if (!stack || !layout) return;
    const measure = () => {
      const gap = parseFloat(getComputedStyle(layout).columnGap) || 0;
      const stackW = stack.getBoundingClientRect().width;
      const avail = layout.clientWidth - stackW - gap;
      setDockW(Math.max(0, Math.min(Math.floor(stackW), Math.floor(avail))));
    };
    const ro = new ResizeObserver(measure);
    ro.observe(layout);
    ro.observe(stack);
    measure();
    return () => ro.disconnect();
  }, [phase]);
  const docked = (dockW ?? 0) >= 240;

  /* ---------- agent_status / agent_events 增量轮询（对局存活期 500ms 一拍） ---------- */
  useEffect(() => {
    if (phase === "setup") return;
    let dead = false;
    const tick = async () => {
      const id = runIdRef.current;
      if (id === null || dead) return;
      try {
        const st = JSON.parse(await agentCall<string>("agent_status", { id })) as AgentStatus;
        if (dead) return;
        setStatus(st);
        // pairing 一旦离开（进入思考/等待/终局/出错）就切对局视图
        if (st.state !== "pairing") setPhase((p) => (p === "pairing" ? "playing" : p));
      } catch {
        /* 单拍失败容忍（切页竞态/壳忙），下一拍再取 */
      }
      try {
        const r = JSON.parse(await agentCall<string>("agent_events", { id, since: sinceRef.current })) as {
          next: number;
          items: AgentEventItem[];
        };
        if (dead) return;
        if (Array.isArray(r.items) && r.items.length > 0) {
          if (typeof r.next === "number") sinceRef.current = r.next;
          // 环形 ≤200 由后端保证；前端再截一次防 detail 泛滥把 DOM 撑爆
          setEvents((prev) => [...prev, ...r.items].slice(-200));
        }
      } catch {
        /* 同上 */
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 500);
    return () => {
      dead = true;
      window.clearInterval(timer);
    };
  }, [phase]);

  /* ---------- 设置持久化（改动即落盘：Rust 从同一份平台存储读配置） ---------- */
  function updateLlm(patch: Partial<LlmConfig>) {
    const next = { ...llm, ...patch };
    setLlm(next);
    storeSet(KEY_LLM_CONFIG, JSON.stringify(next));
  }
  function updateLlmKey(v: string) {
    setLlmKey(v);
    storeSet(KEY_LLM_KEY, v);
  }
  function updateCtxLimit(v: string) {
    setCtxLimit(v);
    if (v.trim()) storeSet(KEY_CTX_LIMIT, String(clampCtxLimit(v)));
  }
  function updateMcpPort(v: string) {
    setMcpPort(v);
    storeSet(KEY_MCP_PORT, v);
  }
  function updateMcpToken(v: string) {
    setMcpToken(v);
    storeSet(KEY_MCP_TOKEN, v);
  }

  /** 重新生成 token：落盘新串；服务器在跑就**同步重启**（agent_mcp_set(true)——
   *  先停旧实例再按当前 store 起新的）。链路事实：token 只在 McpServer::start 时
   *  烘焙进鉴权中间件，不重启的话运行中的服务器仍认旧串、新串反而 401——「旧串
   *  立即作废」只在重启后成立。连接卡只在对局外渲染（设置卡），重启不会拆活局。 */
  async function regenerateMcpToken() {
    const next = genMcpToken();
    setMcpToken(next);
    // 可等待写：store_set 与 agent_mcp_set 在 Rust 侧并发无先后承诺（persistSettings 同款教训），
    // 先落盘再重启，重启读到的一定是新串。
    await storeSetAsync(KEY_MCP_TOKEN, next);
    if (mcpInfo?.enabled) {
      try {
        const raw = await agentInvoke<string>("agent_mcp_set", { enabled: true });
        setMcpInfo(JSON.parse(raw) as { enabled: boolean; url: string | null; token: string | null });
      } catch {
        /* 壳未带 agent.rs：新串下次启用服务器时生效 */
      }
    }
  }

  /** 落盘当前设置（开始/测试连接前调——后端读的是存储，不是本组件 state）。
   *  用可等待写并 await：store_set 与 agent_start/agent_llm_test 在 Rust 侧并发
   *  执行无先后承诺，fire-and-forget 会让开局/测试读到上一份配置（错 key/错模型）。 */
  async function persistSettings(): Promise<void> {
    await storeSetAsync(KEY_LLM_CONFIG, JSON.stringify(llm));
    await storeSetAsync(KEY_LLM_KEY, llmKey);
    await storeSetAsync(KEY_CTX_LIMIT, String(clampCtxLimit(ctxLimit)));
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

  async function runLlmTest() {
    if (llmTest.busy) return;
    await persistSettings();
    setLlmTest({ busy: true, ok: false, text: null });
    try {
      const r = await agentCall<string>("agent_llm_test");
      // 契约："ok"=通；其余一律是人话错误文本，原样展示
      setLlmTest({ busy: false, ok: r === "ok", text: r === "ok" ? "连接成功" : r || "连接失败" });
    } catch (e) {
      setLlmTest({ busy: false, ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  async function toggleMcp(enabled: boolean) {
    setMcpEnabled(enabled);
    storeSet(KEY_MCP_ENABLED, enabled ? "true" : "false");
    try {
      const raw = await agentInvoke<string>("agent_mcp_set", { enabled });
      setMcpInfo(JSON.parse(raw) as { enabled: boolean; url: string | null; token: string | null });
    } catch {
      /* 壳未带 agent.rs：开关已落存储，状态行保持本地口径 */
    }
  }

  /* ---------- 开局 ---------- */

  /** 建专用会话 A'（固定无服务器）。`link` 非空 = 我执白方向：A' 以 B 的邀请链接
   *  经 Boot 路径创建（受邀方）。基座链接走 /p2p 意图而非当前路径 /agent——原生侧
   *  的链接解析会把陌生单段路径当用户主页处理（goptop-agent pair.rs 的邀请方基座
   *  就是这么选的，on_boot 只认意图不落页）。 */
  async function bootFront(link: string | null): Promise<GameSession> {
    const cfg = JSON.stringify({
      name: myName(),
      serverMode: false,
      shareOrigin: shareOrigin(),
      kind: setup.kind,
      size: setup.size,
    });
    const href = link ?? `${shareOrigin().replace(/\/$/, "")}/p2p`;
    // onChange 注入：A' 的快照变化只喂本页（单槽详见 session.ts 注）。
    // suppressNav：状态机在受理回执/挑战时会发 Nav("/p2p")——那是「页面级」导航，
    // 对常驻 /agent 的 A' 是错误导航，执行了会把整页拖离、随卸载拆掉整局。
    // 桌面走原生路（原样零变化）；web 走 wasm 的 detached 会话（不占全局槽，
    // emit 走注入回调，同参同语义）。
    const onReady = () => setSnap(parseAgentSnap(aPrimeRef.current?.snapshot() ?? null));
    const s = native
      ? await createSession(cfg, href, onReady, { suppressNav: true })
      : await createDetachedSession(cfg, href, onReady, { suppressNav: true });
    // A' 自管 poll 立刻起泵：我执黑方向随后要在 waitInviteReady 里读快照缓存，
    // 没有泵就永远是 createSession 首拍那份旧缓存（inviteUrl 永远等不到 rtc=）
    s.start_pump();
    return s;
  }

  /** 我执黑：等 A' 的邀请链接编入 rtc（≤40s，与 pair.rs 同一口径同文案）。
   *  快照刷新靠 A' 自己的泵（bootFront 已 start_pump），这里只读缓存。 */
  async function waitInviteReady(s: GameSession): Promise<void> {
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline && !disposedRef.current) {
      const url = parseAgentSnap(s.snapshot())?.inviteUrl ?? "";
      if (url.includes("rtc=")) return;
      await sleep(150);
    }
    if (disposedRef.current) throw new Error("已取消");
    throw new Error("40 秒内未生成含 rtc 的邀请链接（ICE gathering 未完成？）");
  }

  /** 我执白：等 B 的邀请链接经 agent_status.detail 送回（≤50s）。 */
  async function waitAgentLink(runId: number): Promise<string> {
    const deadline = Date.now() + 50_000;
    while (Date.now() < deadline && !disposedRef.current) {
      let st: AgentStatus;
      try {
        st = JSON.parse(await agentCall<string>("agent_status", { id: runId })) as AgentStatus;
      } catch {
        await sleep(500);
        continue;
      }
      if (st.state === "error") throw new Error(st.detail || "Agent 启动失败");
      const link = extractInviteLink(st.detail);
      if (link) return link;
      await sleep(500);
    }
    if (disposedRef.current) throw new Error("已取消");
    throw new Error("等待 Agent 生成对局超时（50 秒）");
  }

  async function start() {
    if (busy || !agentPageMode(native, webOk).enabled || runIdRef.current !== null) return;
    setBusy(true);
    setErr(null);
    setPhase("pairing");
    setStatus(EMPTY_STATUS);
    setEvents([]);
    sinceRef.current = 0;
    await persistSettings();
    /** bind 的会话 id 按通道取：原生是会话表 id，web 是 FRONT 注册表 id（十进制串）。 */
    const bindId = (s: GameSession) => (native ? String(s.nativeId?.() ?? "") : String(s.agentId?.() ?? ""));
    try {
      const cfgJson = buildStartCfg({ ...setup, uiLang: UI_LANG });
      if (setup.myColor === "white") {
        // 我执白：B 先建局出链接 → A' 携链 Boot 入局 → 登记 → 后端收尾配对
        const runId = await agentCall<number>("agent_start", { cfgJson });
        runIdRef.current = runId;
        const link = await waitAgentLink(runId);
        const created = await bootFront(link);
        if (disposedRef.current) {
          await created.dispose();
          return;
        }
        aPrimeRef.current = created;
        try {
          await agentCall("agent_bind", { sessionId: bindId(created) });
        } catch (e) {
          throw new Error(`登记 A' 会话失败: ${e instanceof Error ? e.message : String(e)}`);
        }
        adopt(created);
      } else {
        // 我执黑：A' 建局 → bind 登记（后端代发 CreateInvite——A' 建在 /p2p 基座上
        // 不会自发邀请，先等 rtc 再 bind 会互相等死）→ 链含 rtc → agent_start 结对
        const created = await bootFront(null);
        if (disposedRef.current) {
          await created.dispose();
          return;
        }
        aPrimeRef.current = created;
        try {
          await agentCall("agent_bind", { sessionId: bindId(created) });
        } catch (e) {
          throw new Error(`登记 A' 会话失败: ${e instanceof Error ? e.message : String(e)}`);
        }
        await waitInviteReady(created);
        const runId = await agentCall<number>("agent_start", { cfgJson });
        runIdRef.current = runId;
        adopt(created);
      }
    } catch (e) {
      await teardown(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  /** 采纳 A'：接线到渲染层并同步首帧快照（泵已在 bootFront 起好）。 */
  function adopt(created: GameSession) {
    aPrimeRef.current = created;
    setAPrime(created);
    setSnap(parseAgentSnap(created.snapshot()));
  }

  /* ---------- 派生（对局视图） ---------- */
  const kind = snap?.kind ?? setup.kind;
  const size = snap?.size ?? setup.size;
  const myColor = (snap?.myColor ?? setup.myColor) as "black" | "white";
  /* 状态行文案与 P2P 同款极短式（useGameSession 的 statusText 就是「黑 落子」/「黑 胜」）：
     这行在 BoardPanel 里是 18px 大字，长了必折行、卡片长高直接吃棋盘高度——
     紧凑红线（2026-09-18 挤压教训）。「谁在行动」的语义由执色+侧栏状态卡表达。 */
  const statusText = snap?.winner
    ? `${snap.winner === "black" ? "黑" : "白"} 胜`
    : status.state === "error"
      ? "对局中止"
      : phase === "pairing"
        ? "配对中…"
        : `${snap?.toMove === "black" ? "黑" : "白"} 落子`;
  /* 错误详情钳制成单行短句（Rust 侧 excerpt 已摘要化，这里兜底防其它来源的长文案
     再挤爆状态卡——紧凑红线；全文仍在工具日志与 agent_status 可查）。 */
  const errNote = status.state === "error" && status.detail ? status.detail.split("\n")[0].slice(0, 80) : "";
  const statusNote = errNote || `执${myColor === "black" ? "黑" : "白"}`;
  const running = phase !== "setup";
  /* 工具日志显示面：llm HTTP 行不上屏（次数在统计行）；只渲染尾部 60 条。 */
  const logEvents = useMemo(() => events.filter((e) => e.tool !== "llm").slice(-60), [events]);
  // 新事件自动置底：人只关心最新动作（无此逻辑时滚动条停原位，最新内容在视口外）
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [logEvents.length, logOpen]);
  /* 页面启用态：桌面原生或 web 后端其一就绪即可用；横幅只在两头都不可用时出
     （鸿蒙壳 / web 产物未带 agent 导出）。 */
  const { enabled, banner } = agentPageMode(native, webOk);

  /* ============================ 渲染 ============================ */

  const setupCard = (
    <div className="brutal-card" style={{ padding: "16px 14px", background: "#fff", display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
        <span className="brutal-label">Agent 对战 · 对局设置</span>
        <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回菜单页</button>
      </div>

      {banner && (
        <div style={{ border: "3px solid var(--ink)", background: "#fffbeb", padding: "8px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>
          内置 Agent 对战仅桌面版可用
        </div>
      )}

      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>棋种与路数</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          {(["gomoku", "go"] as const).map((k) => (
            <button
              key={k}
              className={`brutal-btn brutal-btn--sm${setup.kind === k ? " brutal-btn--active" : ""}`}
              aria-pressed={setup.kind === k}
              disabled={!enabled}
              onClick={() =>
                setSetup((prev) => ({
                  ...prev,
                  kind: k,
                  // 尺寸表与 KindSizePicker 同源：五子棋只有 15，围棋 9/13/19
                  size: k === "gomoku" ? 15 : ([9, 13, 19] as number[]).includes(prev.size) ? prev.size : 19,
                }))
              }
            >
              {k === "gomoku" ? "五子棋" : "围棋"}
            </button>
          ))}
          <div style={{ width: 1, height: 26, background: "var(--ink)", opacity: 0.18 }} />
          {(setup.kind === "gomoku" ? [15] : [9, 13, 19]).map((s) => (
            <button
              key={s}
              className={`brutal-btn brutal-btn--sm${setup.size === s ? " brutal-btn--active" : ""}`}
              aria-pressed={setup.size === s}
              disabled={!enabled}
              onClick={() => setSetup((prev) => ({ ...prev, size: s as Size }))}
            >
              {s}×{s}
            </button>
          ))}
        </div>
      </div>

      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>我执子（邀请方执黑）</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          {(["black", "white"] as const).map((c) => (
            <button
              key={c}
              className={`brutal-btn brutal-btn--sm${setup.myColor === c ? " brutal-btn--active" : ""}`}
              aria-pressed={setup.myColor === c}
              disabled={!enabled}
              onClick={() => setSetup((prev) => ({ ...prev, myColor: c }))}
            >
              {c === "black" ? "我执黑（先行）" : "我执白"}
            </button>
          ))}
        </div>
      </div>

      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>对手驱动</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button
            className={`brutal-btn brutal-btn--sm${setup.driver === "builtin" ? " brutal-btn--active" : ""}`}
            aria-pressed={setup.driver === "builtin"}
            disabled={!enabled}
            onClick={() => setSetup((prev) => ({ ...prev, driver: "builtin" }))}
          >
            内置（LLM 循环）
          </button>
          {/* MCP 依赖内嵌 TCP 服务器（桌面固有）；Web 连卡都不出（计划 R7） */}
          {native && (
            <button
              className={`brutal-btn brutal-btn--sm${setup.driver === "mcp" ? " brutal-btn--active" : ""}`}
              aria-pressed={setup.driver === "mcp"}
              onClick={() => setSetup((prev) => ({ ...prev, driver: "mcp" }))}
            >
              MCP（外部 Agent）
            </button>
          )}
        </div>
      </div>

      {setup.driver === "mcp" && native ? (
        <div style={{ border: "3px solid var(--ink)", padding: 10, display: "flex", flexDirection: "column", gap: 8 }}>
          <div className="brutal-label">MCP 服务器（外部 Agent 经此认领对手席）</div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <button
              className={`brutal-btn brutal-btn--sm${mcpEnabled ? " brutal-btn--active" : ""}`}
              aria-pressed={mcpEnabled}
              onClick={() => void toggleMcp(!mcpEnabled)}
            >
              {mcpEnabled ? "已启用（点此关闭）" : "已关闭（点此启用）"}
            </button>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>
              状态：{mcpInfo ? (mcpInfo.enabled ? "服务器运行中" : "未运行") : "未知"}
            </span>
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>端口</span>
            <input value={mcpPort} inputMode="numeric" style={{ ...inputStyle, width: 90 }} onChange={(e) => updateMcpPort(e.target.value)} />
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>Token</span>
            <input value={mcpToken} style={{ ...inputStyle, flex: 1, minWidth: 160 }} onChange={(e) => updateMcpToken(e.target.value)} />
            <button
              className="brutal-btn brutal-btn--sm"
              title="重新生成随机 Bearer Token（服务器运行中会自动重启使旧串立即作废；未运行则下次启用生效）"
              onClick={() => void regenerateMcpToken()}
            >
              重新生成
            </button>
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <button
              className="brutal-btn brutal-btn--sm"
              onClick={() => {
                // 真相分型：运行中→info 的实际 url/token（端口回退、运行中的 token 以实例为权威）；
                // 未运行→当前输入（store 即下次启动的配置，state 里的 info 只是上次查询的快照——
                // 重新生成/手动改串后照抄它会把已作废的旧串复制出去）。
                const url = mcpInfo?.enabled
                  ? mcpInfo.url ?? `http://127.0.0.1:${mcpPort.trim() || "9537"}/mcp`
                  : `http://127.0.0.1:${mcpPort.trim() || "9537"}/mcp`;
                const token = mcpInfo?.enabled ? mcpInfo.token ?? mcpToken : mcpToken;
                void copyText(buildMcpJson(url, token), "连接串已复制");
              }}
            >
              复制 .mcp.json 连接串
            </button>
            {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
          </div>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)", lineHeight: 1.5 }}>
            把连接串放进外部 Agent 的 .mcp.json，它即可经 MCP 认领对手席；启用后本页显示「等待 MCP Agent 接入」。
          </div>
        </div>
      ) : (
        <div style={{ border: "3px solid var(--ink)", padding: 10, display: "flex", flexDirection: "column", gap: 8 }}>
          <div className="brutal-label">内置 LLM 配置</div>
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
            {([
              ["anthropic", "Anthropic"],
              ["openai-responses", "OpenAI Responses"],
              ["openai-chat", "OpenAI Chat"],
            ] as const).map(([v, label]) => (
              <button
                key={v}
                className={`brutal-btn brutal-btn--sm${llm.protocol === v ? " brutal-btn--active" : ""}`}
                aria-pressed={llm.protocol === v}
                disabled={!enabled}
                onClick={() => updateLlm({ protocol: v })}
              >
                {label}
              </button>
            ))}
          </div>
          <label style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>Base URL</span>
            <input
              value={llm.baseUrl}
              disabled={!enabled}
              placeholder="https://your-gateway.example（留空用协议默认）"
              style={{ ...inputStyle, flex: 1, minWidth: 200 }}
              onChange={(e) => updateLlm({ baseUrl: e.target.value })}
            />
          </label>
          <label style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>模型</span>
            <input
              value={llm.model}
              disabled={!enabled}
              placeholder="如 claude-sonnet-4-5"
              style={{ ...inputStyle, flex: 1, minWidth: 200 }}
              onChange={(e) => updateLlm({ model: e.target.value })}
            />
          </label>
          <label style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>API Key</span>
            <input
              type="password"
              value={llmKey}
              disabled={!enabled}
              placeholder="sk-…"
              style={{ ...inputStyle, flex: 1, minWidth: 200 }}
              onChange={(e) => updateLlmKey(e.target.value)}
            />
          </label>
          <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "#b00020" }}>
            密钥以明文存于本机配置（store.json），请自行注意保管。
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>上下文上限</span>
            <input value={ctxLimit} inputMode="numeric" disabled={!enabled} style={{ ...inputStyle, width: 110 }} onChange={(e) => updateCtxLimit(e.target.value)} />
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--muted)" }}>tokens（8000 – 1000000，超出按边界取）</span>
          </div>
          <label style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>回复语言</span>
            <input
              value={llm.replyLang}
              disabled={!enabled}
              placeholder="留空跟随界面语言（可填任意文字）"
              style={{ ...inputStyle, flex: 1, minWidth: 200 }}
              onChange={(e) => updateLlm({ replyLang: e.target.value })}
            />
          </label>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, width: 96 }}>子代理</span>
            <button
              className={`brutal-btn brutal-btn--sm${llm.enableSubagent ? " brutal-btn--active" : ""}`}
              aria-pressed={llm.enableSubagent}
              disabled={!enabled}
              onClick={() => updateLlm({ enableSubagent: !llm.enableSubagent })}
            >
              {llm.enableSubagent ? "已开启（深思更费 token）" : "已关闭"}
            </button>
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            <button className="brutal-btn brutal-btn--sm" disabled={!enabled || llmTest.busy} onClick={() => void runLlmTest()}>
              {llmTest.busy ? "测试中…" : "测试连接"}
            </button>
            {llmTest.text && (
              <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, color: llmTest.ok ? "#0a7a2e" : "#b00020", overflowWrap: "anywhere" }}>
                {llmTest.text}
              </span>
            )}
          </div>
        </div>
      )}

      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>对手名字（仅聊天展示）</div>
        <input
          value={setup.agentName}
          disabled={!enabled}
          placeholder="Agent"
          style={{ ...inputStyle, width: 220 }}
          onChange={(e) => setSetup((prev) => ({ ...prev, agentName: e.target.value }))}
        />
      </div>

      {err && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, color: "#b00020", overflowWrap: "anywhere" }}>{err}</div>}
      {notice && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, color: "#0a7a2e" }}>{notice}</div>}

      <button className="brutal-btn brutal-btn--accent" style={{ padding: "14px 8px" }} disabled={!enabled || busy} onClick={() => void start()}>
        {busy ? "正在开局…" : "开始对局"}
      </button>
    </div>
  );

  if (!running) return <div className="play-stack">{setupCard}</div>;

  const confirmReq = snap?.confirmReq ?? null;
  const dead = snap ? [...snap.myDead, ...snap.peerDead] : [];

  /* Agent 状态卡：状态/暂存着子/token 计量/工具日志（增量）/停止。住侧栏（弹窗形态随聊天同进退）。 */
  const agentCard = (
    <div className="brutal-card" style={{ padding: 12, display: "flex", flexDirection: "column", gap: 8, background: "#fff", flexShrink: 0 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span className="brutal-label">Agent 状态</span>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, color: status.state === "error" ? "#b00020" : "var(--ink)" }}>
          {agentStateLabel(status.state)}
        </span>
        {status.stagedMove && !snap?.winner && (
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }} title="Agent 已暂存、尚未提交的着子（棋盘上为幽灵子）">
            拟落 {status.stagedMove.x},{status.stagedMove.y}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <button className="brutal-btn brutal-btn--sm" aria-pressed={logOpen} title="收起/展开 Agent 的操作日志" onClick={() => setLogOpen((v) => !v)}>
          {logOpen ? "收日志" : "日志"}
        </button>
        <button
          className="brutal-btn brutal-btn--sm"
          title="认输并终止 Agent（后端先 resign 再清理）"
          onClick={() => {
            void teardown(null).then(() => {
              if (!disposedRef.current) setNotice("已停止 Agent 对局");
            });
          }}
        >
          停止对局
        </button>
        {(status.state === "done" || status.state === "error") && (
          <button className="brutal-btn brutal-btn--sm brutal-btn--primary" onClick={() => void teardown(null)}>
            返回设置
          </button>
        )}
      </div>
      <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)", display: "flex", gap: 12, flexWrap: "wrap" }}>
        <span>LLM 调用 {status.llmCalls}</span>
        <span>输入 {status.tokensIn} tok</span>
        <span>输出 {status.tokensOut} tok</span>
        <span>压缩 {status.compactions} 次</span>
      </div>
      {logOpen && (
        /* 滚动条隐藏（.agent-log）；新事件自动置底——人只关心最新动作。
           只渲染尾部 60 条：环有界但 DOM 无界一样会拖慢页面。llm 行不进日志。 */
        <div ref={logRef} className="agent-log" style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)", maxHeight: 130, display: "flex", flexDirection: "column", gap: 2 }}>
          {logEvents.length === 0 ? (
            <span>暂无操作日志</span>
          ) : (
            logEvents.map((e, i) => (
              <span key={`${e.ts}-${i}`} style={{ overflowWrap: "anywhere", color: e.ok ? "var(--muted)" : "#b00020" }}>
                {formatAgentEvent(e)}
              </span>
            ))
          )}
        </div>
      )}
    </div>
  );

  /* 聊天（A' 的聊天流与全部协商动作，ChatPanel 原样复用）+ Agent 状态卡——停靠栏与弹窗共用。
      收起=折叠成细条（ChatPanel 的 onClose 语义），不导航离页——离页会拆掉整局。 */
  const chatAndStatus = (
    <>
      {chatOpen ? (
        <div style={{ flex: 1, minHeight: 120, display: "flex", flexDirection: "column" }}>
          <ChatPanel
            role={snap?.role ?? "idle"}
            chatLog={snap?.chatLog ?? []}
            peerAvatars={snap?.peerAvatars ?? {}}
            spectators={snap?.spectators ?? []}
            specRequests={snap?.specRequests ?? []}
            specCanChat={snap?.specCanChat ?? false}
            spectateEnabled={snap?.spectateEnabled ?? true}
            onSend={(t) => aPrime?.send_chat(t)}
            onUndo={() => aPrime?.request_undo()}
            onReset={() => aPrime?.request_reset()}
            onSwap={() => aPrime?.request_swap()}
            onKick={(id) => aPrime?.kick_spec(id)}
            onMute={(id, muted) => aPrime?.mute_spec(id, muted)}
            onDisableSpectate={() => aPrime?.disable_spectate()}
            onRequestSpecChat={() => aPrime?.request_spec_chat()}
            onApproveSpec={(from) => aPrime?.approve_spec(from)}
            onRejectSpec={(from) => aPrime?.reject_spec(from)}
            onClose={() => setChatOpen(false)}
            onResign={() => aPrime?.resign()}
            canResign={!snap?.winner && !snap?.scoring}
          />
        </div>
      ) : (
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexShrink: 0, borderBottom: "2px solid var(--ink)", paddingBottom: 8 }}>
          <span className="brutal-label">聊天（已收起）</span>
          <span style={{ flex: 1 }} />
          <button className="brutal-btn brutal-btn--sm" onClick={() => setChatOpen(true)}>打开聊天</button>
        </div>
      )}
      {agentCard}
    </>
  );

  return (
    <>
      <div className="play-stack" ref={stackRef}>
        <ConfirmBanner req={confirmReq} onApprove={() => aPrime?.confirm_approve()} onDecline={() => aPrime?.confirm_decline()} />

        <BoardPanel
          kind={kind}
          size={size}
          board={snap?.board ?? emptyBoard(size)}
          toMove={snap?.toMove ?? "black"}
          winner={snap?.winner ?? null}
          lastMove={snap?.lastMove ?? null}
          hover={hover}
          onHover={setHover}
          disabled={agentBoardDisabled({
            winner: snap?.winner ?? null,
            role: snap?.role ?? "idle",
            phase: snap?.phase ?? "waiting",
            peerConnected: snap?.peerConnected ?? false,
            toMove: snap?.toMove ?? "black",
            myColor,
          })}
          onPlace={(c) => {
            if (!aPrime) return;
            if (snap?.scoring) aPrime.toggle_dead(c.x, c.y);
            else aPrime.place(c.x, c.y);
          }}
          statusText={statusText}
          statusNote={statusNote}
          moveCount={snap?.moveCount ?? 0}
          onUndo={null}
          onReset={null}
          dead={dead}
          allowOccupied={!!snap?.scoring}
          pendingStone={status.stagedMove && !snap?.winner ? status.stagedMove : null}
          chatButton={
            <button
              className="brutal-btn brutal-btn--sm"
              onClick={() => setChatOpen((v) => !v)}
              title="聊天与 Agent 状态"
              aria-label="聊天与 Agent 状态"
            >
              <MessageCircle size={15} strokeWidth={2.5} style={{ display: "block" }} />
            </button>
          }
        />
      </div>

      {/* 侧栏（实测放得下）：与棋盘并行，纵向不占棋盘预算；Agent 状态卡常驻——收聊天不收状态 */}
      {docked && (
        <aside className="brutal-card" style={{ width: dockW ?? 0, flexShrink: 0, display: "flex", flexDirection: "column", gap: 10, padding: 12, background: "#fff", minHeight: 0 }}>
          {chatAndStatus}
        </aside>
      )}

      {/* 侧栏放不下：退聊天弹窗（同 P2P 的两态互斥；首次实测前两态都不渲染一帧） */}
      {!docked && dockW !== null && chatOpen && (
        <div className="chat-modal-bg" onClick={() => setChatOpen(false)}>
          <div className="brutal-card chat-modal" onClick={(e) => e.stopPropagation()} style={{ padding: 12 }}>
            {chatAndStatus}
          </div>
        </div>
      )}
    </>
  );
}
