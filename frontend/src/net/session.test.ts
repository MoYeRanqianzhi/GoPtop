/**
 * net/session 门面单测。
 *
 * - **原生后端**（伪 Tauri 宿主，与 `rules.host-timing.test.ts` 同款思路）：
 *   `session_poll` 的「snapshot:null = 无变化」契约（与 Rust 侧 ab51082 配对）、
 *   `dispose()` 的释放与停泵、建会话失败必须带原文上抛。
 * - **wasm 分派**：init 的 Promise 缓存——StrictMode 双挂载并发两次 createSession
 *   时只许实例化一遍；`dispose()` 走胶水的 free。
 *
 * 为什么在门面层钉死：这些契约的破损形态全是「静默劣化」——缓存被空轮询覆写成
 * null 后上层同步读拿到坏数据、原生会话随每次 WebView 重载泄漏一只（20Hz 泵 +
 * presence + WSS 全部常驻）、dev 端 wasm 双实例化后首次调用即 out of bounds——
 * 浏览器 E2E 很难稳定复现，单测在这里最便宜也最准。
 */
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

/* ---------------- wasm 胶水替身（仅 wasm 分派的用例会碰到） ---------------- */

const transport = vi.hoisted(() => ({
  initCalls: 0,
  freeCalls: 0,
  cfgs: [] as string[],
  /** 阶段⑤：替身产物是否带 agent_* 导出（webAgentAvailable 的「产物未带 agent」用例翻转它）。 */
  hasAgent: true,
  /** new_agent 的入参台账（cfg / 注入回调 / suppressNav）。 */
  detached: [] as { cfg: string; onChange: (() => void) | null; suppressNav: boolean }[],
  /** webAgentCall 的调用台账（cmd + 位置参数）。 */
  agentCalls: [] as { cmd: string; args: unknown[] }[],
  /** 按 cmd 脚本化回执；缺省回 '{"ok":true}'。 */
  agentReplies: {} as Record<string, string>,
  /** 非 null：导出以 rejected Promise 失败（异步导出的折叠用例）。 */
  agentReject: null as string | null,
}));

vi.mock("../wasm/transport/goptop_transport.js", () => {
  const agentExport = (cmd: string) => (...args: unknown[]) => {
    transport.agentCalls.push({ cmd, args });
    if (transport.agentReject !== null) return Promise.reject(new Error(transport.agentReject));
    return transport.agentReplies[cmd] ?? '{"ok":true}';
  };
  // getter 而非求值好的属性：vi.mock 的工厂只跑一遍（resetModules 不重建 mock），
  // 「产物未带 agent」的用例翻转 transport.hasAgent 时必须在**访问时**见 undefined。
  const agent = (cmd: string) => (transport.hasAgent ? agentExport(cmd) : undefined);
  class MockWasmSession {
    constructor(cfgJson: string | null) {
      // new_agent 建的 detached 会话不占主会话的 cfg 台账（null = 结构同型即可）
      if (cfgJson !== null) transport.cfgs.push(cfgJson);
    }
    static new_agent(cfgJson: string, onChange: () => void, suppressNav: boolean) {
      transport.detached.push({ cfg: cfgJson, onChange, suppressNav });
      return new MockWasmSession(null);
    }
    snapshot() { return "null"; }
    agent_id() { return "11"; }
    start_pump() { /* 单测不泵 */ }
    free() { transport.freeCalls++; }
  }
  return {
    default: async () => {
      transport.initCalls++;
    },
    get agent_start() { return agent("agent_start"); },
    get agent_stop() { return agent("agent_stop"); },
    get agent_status() { return agent("agent_status"); },
    get agent_events() { return agent("agent_events"); },
    get agent_bind() { return agent("agent_bind"); },
    get agent_llm_test() { return agent("agent_llm_test"); },
    WasmSession: MockWasmSession,
  };
});

/* ---------------- 伪 Tauri 宿主（与 rules.host-timing.test.ts 同款） ---------------- */

type Reply = string | number | null | undefined;

let sessions: Map<number, { polls: number }>;
let nextId: number;
let calls: string[];
let droppedIds: number[];
let totalPolls: number;
/** 下一拍要回的快照：string = 全量（进前端缓存），null = 无变化（沿用缓存）。
 *  形状与 Rust 侧 `session_poll` 的回包一一对应（JSON null ≠ 字符串 "null"）。 */
let snapReply: string | null;
/** 堆给下一拍的宿主动作（session_poll 一并带回并取走）。 */
let queuedActions: unknown[];
/** 非 null 时 session_new 回这份 JSON（模拟宿主的错误回执，如鸿蒙的 {"error": …}）。 */
let failNewReply: string | null;

function installTauri() {
  (globalThis as unknown as { window: unknown }).window = {
    // 泵的 setInterval/clearInterval 走 window：代理到 globalThis，
    // 这样 vi.useFakeTimers() 换上去的假定时器才接得住
    setInterval: (...a: Parameters<typeof setInterval>) => setInterval(...a),
    clearInterval: (...a: Parameters<typeof clearInterval>) => clearInterval(...a),
    __TAURI_INTERNALS__: {
      invoke: async (cmd: string, args: Record<string, unknown>): Promise<Reply> => {
        calls.push(cmd);
        switch (cmd) {
          case "session_new": {
            if (failNewReply !== null) {
              const r = failNewReply;
              failNewReply = null;
              return r;
            }
            const id = nextId++;
            sessions.set(id, { polls: 0 });
            return id;
          }
          case "session_poll": {
            const e = sessions.get(Number(args.id));
            if (!e) return JSON.stringify({ snapshot: "null", actions: [] });
            e.polls++;
            totalPolls++;
            const actions = queuedActions;
            queuedActions = [];
            return JSON.stringify({ snapshot: snapReply, actions });
          }
          case "session_drop": {
            droppedIds.push(Number(args.id));
            sessions.delete(Number(args.id));
            return undefined;
          }
          case "session_cmd":
            return JSON.stringify({ ok: true });
          default:
            return "null";
        }
      },
    },
  };
}

beforeEach(() => {
  sessions = new Map();
  nextId = 1;
  calls = [];
  droppedIds = [];
  totalPolls = 0;
  snapReply = '{"userId":"u-1"}';
  queuedActions = [];
  failNewReply = null;
  vi.unstubAllGlobals();
  transport.initCalls = 0;
  transport.freeCalls = 0;
  transport.cfgs = [];
  transport.hasAgent = true;
  transport.detached = [];
  transport.agentCalls = [];
  transport.agentReplies = {};
  transport.agentReject = null;
  delete (globalThis as { window?: unknown }).window;
});

afterEach(() => {
  vi.useRealTimers();
});

/** 每次都取一份新门面：session.ts 的 transportReady 是模块级缓存，必须重置。 */
async function freshSession() {
  vi.resetModules();
  return import("./session");
}

describe("原生后端：session_poll 的「无变化」契约", () => {
  it("snapshot:null＝无变化：缓存保留、onChange 不响，宿主动作照常执行", async () => {
    installTauri();
    const { createSession } = await freshSession();
    const s = await createSession("{}", "http://x/");
    expect(s.snapshot(), "create 的首拍必须给全量，前端才有东西可缓存").toBe('{"userId":"u-1"}');

    const onChange = vi.fn();
    const onNotice = vi.fn();
    const w = (globalThis as unknown as { window: Record<string, unknown> }).window;
    w.goptopOnChange = onChange;
    w.goptopNotice = onNotice;

    // 直接驱动泵（pump 只在原生适配器上，接口面没有；单测要的就是一拍一拍地走）
    const native = s as unknown as { pump(): Promise<void> };
    // 一拍「无变化」+ 一条提示：动作必须执行，缓存与重渲染回调都不能动
    snapReply = null;
    queuedActions = [{ t: "notice", text: "你好", ms: 1000 }];
    await native.pump();
    expect(onNotice).toHaveBeenCalledTimes(1);
    expect(s.snapshot(), "空轮询不得把缓存覆写成 null——上层 snapshot() 是同步读，覆写即坏数据").toBe('{"userId":"u-1"}');
    expect(onChange, "无变化就不该触发重渲染").not.toHaveBeenCalled();

    // 下一拍快照真的变了：缓存更新 + onChange 恰好一次
    snapReply = '{"userId":"u-2"}';
    await native.pump();
    expect(s.snapshot()).toBe('{"userId":"u-2"}');
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});

describe("原生后端：dispose（session_drop 是唯一能真正停掉 Rust 侧泵/presence/WS 的路径）", () => {
  it("dispose 后 session_drop 恰好一次、表项清空、JS 泵停住", async () => {
    installTauri();
    vi.useFakeTimers();
    const { createSession } = await freshSession();
    const s = await createSession("{}", "http://x/");
    s.start_pump();
    // 50ms 周期，160ms 至少 3 拍
    await vi.advanceTimersByTimeAsync(160);
    const pollsAtDrop = totalPolls;
    expect(pollsAtDrop).toBeGreaterThanOrEqual(3);

    await s.dispose();
    expect(droppedIds, "session_drop 要带对 id").toEqual([1]);
    expect(sessions.size, "Rust 侧表项要被拆掉，否则会话连同它的任务常驻").toBe(0);

    // 泵必须停：释放后再过一个周期，一次 poll 都不该有
    await vi.advanceTimersByTimeAsync(160);
    expect(totalPolls).toBe(pollsAtDrop);
  });
});

describe("原生后端：建会话失败", () => {
  it("session_new 回错误形态时要抛出并带原始回执（不能给哑会话静默）", async () => {
    installTauri();
    const { createSession } = await freshSession();
    failNewReply = JSON.stringify({ error: "cfg 解析失败: bad field" });
    await expect(createSession("{}", "http://x/")).rejects.toThrow(/cfg 解析失败/);
    expect(sessions.size).toBe(0);
  });
});

describe("wasm 分派", () => {
  it("并发 createSession（StrictMode 双挂载形态）只实例化一遍 wasm", async () => {
    const { createSession } = await freshSession();
    const [a, b] = await Promise.all([
      createSession('{"kind":"gomoku"}', "http://x/"),
      createSession('{"kind":"gomoku"}', "http://x/"),
    ]);
    expect(
      transport.initCalls,
      "init 无 Promise 缓存时这里是 2：wasm 被实例化两遍后内存视图互相失效，先建的会话首次调用即 out of bounds",
    ).toBe(1);
    expect(transport.cfgs).toEqual(['{"kind":"gomoku"}', '{"kind":"gomoku"}']);
    expect(typeof a.snapshot).toBe("function");
    expect(typeof b.snapshot).toBe("function");
  });

  it("dispose 走胶水的 free（不依赖 GC 的确定性释放）", async () => {
    const { createSession } = await freshSession();
    const s = await createSession("{}", "http://x/");
    expect(transport.freeCalls).toBe(0);
    await s.dispose();
    expect(transport.freeCalls).toBe(1);
  });
});

/* ---------------- 阶段⑤：Agent 出口的 wasm 面（createDetachedSession / agentId / webAgentAvailable / webAgentCall） ---------------- */

/** 纯 web 宿主：无 Tauri、无鸿蒙桥（extra 追加钩子/导出探测所需的面）。 */
function installWeb(extra: Record<string, unknown> = {}) {
  (globalThis as unknown as { window: unknown }).window = { ...extra };
}

describe("createDetachedSession（web 的 A' 专用会话）", () => {
  it("new_agent 承接 cfg/onChange/suppressNav；agentId 供 agent_bind 登记", async () => {
    const { createDetachedSession } = await freshSession();
    const onChange = vi.fn();
    const s = await createDetachedSession("{}", "http://x/", onChange, { suppressNav: true });
    expect(transport.detached).toEqual([{ cfg: "{}", onChange, suppressNav: true }]);
    expect(s.agentId?.(), "wasm A' 在 FRONT 注册表的 id（十进制串）").toBe("11");
    transport.detached[0].onChange?.();
    expect(onChange, "快照通知走注入回调，与原生 A' 同语义").toHaveBeenCalledTimes(1);
  });
});

describe("webAgentAvailable（web 后端可用判定，契约 §5.3.2 四道缺一不可）", () => {
  it("产物带 agent 导出且 Vfs 钩子已装 → true", async () => {
    installWeb({ goptopVfsCall: () => "" });
    const { webAgentAvailable } = await freshSession();
    await expect(webAgentAvailable()).resolves.toBe(true);
  });

  it("缺 Vfs 钩子（agentVfs.ts 未装载）→ false", async () => {
    installWeb();
    const { webAgentAvailable } = await freshSession();
    await expect(webAgentAvailable()).resolves.toBe(false);
  });

  it("产物未带 agent 导出（旧 wasm 产物）→ false", async () => {
    installWeb({ goptopVfsCall: () => "" });
    transport.hasAgent = false;
    const { webAgentAvailable } = await freshSession();
    await expect(webAgentAvailable()).resolves.toBe(false);
  });

  it("桌面壳与鸿蒙壳恒 false（钩子装了也不行：桌面走原生路，鸿蒙壳未验收）", async () => {
    installWeb({ goptopVfsCall: () => "", __TAURI_INTERNALS__: {} });
    const { webAgentAvailable } = await freshSession();
    await expect(webAgentAvailable()).resolves.toBe(false);
    installWeb({ goptopVfsCall: () => "", goptopHost: { call: () => "", aiPost: () => "" } });
    await expect(webAgentAvailable()).resolves.toBe(false);
  });
});

describe("webAgentCall（wasm 导出通道，跨边界不 throw）", () => {
  it("位置参数原样下发、回执原样上交", async () => {
    const { webAgentCall } = await freshSession();
    transport.agentReplies.agent_events = '{"next":3,"items":[]}';
    await expect(webAgentCall("agent_events", ["7", 5])).resolves.toBe('{"next":3,"items":[]}');
    expect(transport.agentCalls).toEqual([{ cmd: "agent_events", args: ["7", 5] }]);
  });

  it("异步导出 rejected 折成 ok:false 回执后 resolve（不向调用方 reject）", async () => {
    const { webAgentCall } = await freshSession();
    transport.agentReject = "循环已终止";
    await expect(webAgentCall("agent_stop", [7])).resolves.toBe('{"ok":false,"error":"循环已终止"}');
  });

  it("产物缺该导出：折 ok:false 回执（wasm 产物未带 agent 时的兜底形态）", async () => {
    installWeb();
    transport.hasAgent = false;
    const { webAgentCall } = await freshSession();
    await expect(webAgentCall("agent_start", ["{}"])).resolves.toBe('{"ok":false,"error":"wasm 产物未导出 agent_start"}');
  });
});
