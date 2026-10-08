/**
 * AgentPage 的纯逻辑回归 + 本阶段两处基建改动的契约钉子。
 *
 * 为什么测函数而不是组件：本仓无 jsdom（vitest 默认 environment=node），React
 * 组件挂载不了（与 AiPage.test.ts 同一条约束）；故把 AgentPage 的可判错逻辑
 * （cfg 组装 / 链接提取 / 上限 clamp / 配置解析 / disabled 公式 / 状态与日志排版）
 * 全部抽成导出纯函数，组件只做拼装。
 *
 * 另两组钉住本阶段的基建面：
 * - `/agent` 路由（net/links 的 parseUrl / parsePastedLink）；
 * - createSession 的 onChange 注入（net/session）——A' 会话的快照通知必须走注入
 *   回调、绝不能碰 window.goptopOnChange 单槽（顶掉主会话订阅的事故形态）。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  agentBoardDisabled,
  agentCmdArgs,
  agentPageMode,
  agentStateLabel,
  buildMcpJson,
  buildStartCfg,
  clampCtxLimit,
  DEFAULT_CTX_LIMIT,
  DEFAULT_LLM_CONFIG,
  extractInviteLink,
  formatAgentEvent,
  normalizeLlmConfig,
  parseAgentReply,
  parseAgentSnap,
} from "./AgentPage";
import { parsePastedLink, parseUrl } from "../net/links";

/* ---------------- agent_start 的 cfg 契约 ---------------- */

describe("buildStartCfg", () => {
  it("字段与后端契约逐字对应（driver/kind/size/myColor/agentName/uiLang）", () => {
    expect(JSON.parse(buildStartCfg({ driver: "builtin", kind: "gomoku", size: 15, myColor: "black", agentName: "老王", uiLang: "简体中文" }))).toEqual({
      driver: "builtin",
      kind: "gomoku",
      size: 15,
      myColor: "black",
      agentName: "老王",
      uiLang: "简体中文",
    });
    expect(JSON.parse(buildStartCfg({ driver: "mcp", kind: "go", size: 9, myColor: "white", agentName: "", uiLang: "zh" }))).toMatchObject({
      driver: "mcp",
      kind: "go",
      size: 9,
      myColor: "white",
    });
  });

  it("空白名字归一成 null（后端回默认「Agent」），纯空白同理", () => {
    expect(JSON.parse(buildStartCfg({ driver: "builtin", kind: "gomoku", size: 15, myColor: "black", agentName: "", uiLang: "zh" })).agentName).toBeNull();
    expect(JSON.parse(buildStartCfg({ driver: "builtin", kind: "gomoku", size: 15, myColor: "black", agentName: "   ", uiLang: "zh" })).agentName).toBeNull();
    // 名字两端空白要修剪
    expect(JSON.parse(buildStartCfg({ driver: "builtin", kind: "gomoku", size: 15, myColor: "black", agentName: " Agent ", uiLang: "zh" })).agentName).toBe("Agent");
  });
});

/* ---------------- 我执白：从 agent_status.detail 提取 Boot 链接 ---------------- */

describe("extractInviteLink", () => {
  it("从散文 detail 里提取含 rtc= 的链接", () => {
    expect(
      extractInviteLink("Agent 已建局，请打开 https://x.dev/u-abc?pwd=k9d2x1&kind=go&size=9&rtc=G1AbCd 完成入局"),
    ).toBe("https://x.dev/u-abc?pwd=k9d2x1&kind=go&size=9&rtc=G1AbCd");
  });

  it("detail 就是链接本体时原样提取", () => {
    expect(extractInviteLink("https://x.dev/u-abc?pwd=p&rtc=G1")).toBe("https://x.dev/u-abc?pwd=p&rtc=G1");
  });

  it("缺 rtc= 的链接不算就绪（pair.rs 同一判据：offer 还没编进去）", () => {
    expect(extractInviteLink("https://x.dev/u-abc?pwd=p&kind=gomoku&size=15")).toBeNull();
    expect(extractInviteLink("正在生成邀请链接…")).toBeNull();
  });

  it("空值安全", () => {
    expect(extractInviteLink(null)).toBeNull();
    expect(extractInviteLink(undefined)).toBeNull();
    expect(extractInviteLink("")).toBeNull();
  });
});

/* ---------------- 上下文上限 clamp ---------------- */

describe("clampCtxLimit", () => {
  it("落在 [8k, 1M] 区间内原样保留", () => {
    expect(clampCtxLimit("64000")).toBe(64000);
    expect(clampCtxLimit(176000)).toBe(176000);
    expect(clampCtxLimit(" 8000 ")).toBe(8000);
  });

  it("越界按边界取，非数值回默认 176000", () => {
    expect(clampCtxLimit("100")).toBe(8000);
    expect(clampCtxLimit("2000000")).toBe(1_000_000);
    expect(clampCtxLimit("abc")).toBe(DEFAULT_CTX_LIMIT);
    expect(clampCtxLimit("")).toBe(DEFAULT_CTX_LIMIT);
    expect(clampCtxLimit(null)).toBe(DEFAULT_CTX_LIMIT);
    // 小数向下取整
    expect(clampCtxLimit("176000.9")).toBe(176000);
  });
});

/* ---------------- LLM 配置解析 ---------------- */

describe("normalizeLlmConfig", () => {
  it("无存储 / 坏 JSON 回默认", () => {
    expect(normalizeLlmConfig(null)).toEqual(DEFAULT_LLM_CONFIG);
    expect(normalizeLlmConfig("{ 不是 JSON")).toEqual(DEFAULT_LLM_CONFIG);
    expect(normalizeLlmConfig("")).toEqual(DEFAULT_LLM_CONFIG);
  });

  it("部分字段覆盖，其余逐项回默认；未知协议回 anthropic", () => {
    expect(
      normalizeLlmConfig(JSON.stringify({ protocol: "openai-chat", model: "gpt-5-mini", enableSubagent: true })),
    ).toEqual({
      protocol: "openai-chat",
      baseUrl: "",
      model: "gpt-5-mini",
      maxOutputTokens: 1024,
      replyLang: "",
      enableSubagent: true,
      effort: "off",
      debug: false,
      stream: false,
    });
    expect(normalizeLlmConfig(JSON.stringify({ protocol: "claude-3" })).protocol).toBe("anthropic");
  });

  it("maxOutputTokens 非正数回默认 1024", () => {
    expect(normalizeLlmConfig(JSON.stringify({ maxOutputTokens: 0 })).maxOutputTokens).toBe(1024);
    expect(normalizeLlmConfig(JSON.stringify({ maxOutputTokens: "很多" })).maxOutputTokens).toBe(1024);
  });
});

/* ---------------- BoardPanel disabled 公式（与 useGameSession.boardDisabled 逐项一致） ---------------- */

describe("agentBoardDisabled", () => {
  const base = { winner: null, role: "invitee", phase: "playing", peerConnected: true, toMove: "black" as const, myColor: "black" as const };

  it("对局中：轮到自己才可落子；没连上/对方回合一律禁用", () => {
    expect(agentBoardDisabled(base)).toBe(false);
    expect(agentBoardDisabled({ ...base, toMove: "white" })).toBe(true);
    expect(agentBoardDisabled({ ...base, peerConnected: false })).toBe(true);
  });

  it("终局与观战恒禁用", () => {
    expect(agentBoardDisabled({ ...base, winner: "black" })).toBe(true);
    expect(agentBoardDisabled({ ...base, role: "spectator" })).toBe(true);
  });

  it("waiting 禁用、home 可点（与主会话公式同一形态）", () => {
    expect(agentBoardDisabled({ ...base, phase: "waiting" })).toBe(true);
    expect(agentBoardDisabled({ ...base, phase: "home" })).toBe(false);
  });
});

/* ---------------- 快照 / 状态 / 日志排版 ---------------- */

describe("parseAgentSnap", () => {
  it("合法快照解析、\"null\" 与坏文本回 null", () => {
    const snap = parseAgentSnap('{"kind":"gomoku","size":15,"toMove":"black","moveCount":1}');
    expect(snap).toMatchObject({ kind: "gomoku", size: 15, moveCount: 1 });
    expect(parseAgentSnap("null")).toBeNull();
    expect(parseAgentSnap("")).toBeNull();
    expect(parseAgentSnap("{ 坏")).toBeNull();
    expect(parseAgentSnap(undefined)).toBeNull();
  });
});

describe("agentStateLabel / formatAgentEvent", () => {
  it("六态人话标签；未知值原样透出", () => {
    expect(agentStateLabel("pairing")).toBe("配对中");
    expect(agentStateLabel("waiting_mcp")).toBe("等待 MCP Agent 接入");
    expect(agentStateLabel("thinking")).toBe("Agent 思考中");
    expect(agentStateLabel("waiting")).toBe("等待你的行动");
    expect(agentStateLabel("done")).toBe("已终局");
    expect(agentStateLabel("error")).toBe("出错");
    expect(agentStateLabel("future_state")).toBe("future_state");
  });

  it("日志两行式：操作行 + 换行详情；llm 成功行不上屏（null）、失败行红色头", () => {
    // submit：head=路径，detail=暂存内容头（换行显示）
    expect(formatAgentEvent({ ts: 1, tool: "submit", ok: true, ms: 412, summary: "/game/in/move", detail: "7,7" })).toBe("Submit(/game/in/move)\n  7,7");
    expect(formatAgentEvent({ ts: 2, tool: "read", ok: true, ms: 3, summary: "/game/board", detail: null })).toBe("Read(/game/board)");
    expect(formatAgentEvent({ ts: 3, tool: "write", ok: true, ms: 3, summary: "/memory/notes/style.md", detail: null })).toBe("Write(/memory/notes/style.md)");
    // edit 失败：detail 是报错文本，换行显示
    expect(formatAgentEvent({ ts: 4, tool: "edit", ok: false, ms: 3, summary: "/memory/x", detail: "old_string not found" })).toBe("Edit(/memory/x)\n  old_string not found");
    expect(formatAgentEvent({ ts: 5, tool: "grep", ok: true, ms: 3, summary: "● @ /game/history", detail: null })).toBe("Grep(● @ /game/history)");
    expect(formatAgentEvent({ ts: 6, tool: "wait_events", ok: true, ms: 3000, summary: "timeout=25", detail: null })).toBe("Wait(timeout=25)");
    // 测试模式（llm-config.debug）：思维链/输出同为两行式（头 + 换行缩进全文）
    expect(formatAgentEvent({ ts: 7, tool: "thinking", ok: true, ms: 0, summary: "", detail: "对手第 8 行有活三，应挡 (8,7)" })).toBe("思考\n  对手第 8 行有活三，应挡 (8,7)");
    expect(formatAgentEvent({ ts: 8, tool: "say", ok: true, ms: 0, summary: "", detail: "好棋。" })).toBe("输出\n  好棋。");
    // llm：成功行不渲染（null），失败行红色头 + 换行报错
    expect(formatAgentEvent({ ts: 9, tool: "llm", ok: true, ms: 900, summary: "第 41 次模型调用", detail: null })).toBe("");
    expect(formatAgentEvent({ ts: 10, tool: "llm", ok: false, ms: 900, summary: "第 42 次模型调用", detail: "HTTP 404: <非 JSON 响应：5663 字节>" })).toBe("LLM 调用失败\n  HTTP 404: <非 JSON 响应：5663 字节>");
  });
});

/* ---------------- .mcp.json 连接串 ---------------- */

describe("buildMcpJson", () => {
  it("url + Bearer 头，结构可被 MCP 客户端直接消费", () => {
    const parsed = JSON.parse(buildMcpJson("http://127.0.0.1:9537/mcp", "tok123")) as {
      mcpServers: { goptop: { url: string; headers: { Authorization: string } } };
    };
    expect(parsed.mcpServers.goptop.url).toBe("http://127.0.0.1:9537/mcp");
    expect(parsed.mcpServers.goptop.headers.Authorization).toBe("Bearer tok123");
  });
});

/* ---------------- 阶段⑤ Web 启用态（native / webOk 双路翻转） ---------------- */

describe("agentPageMode：启用与降级横幅", () => {
  it("桌面原生或 web 后端其一就绪即整页可用", () => {
    expect(agentPageMode(true, false)).toEqual({ enabled: true, banner: false });
    expect(agentPageMode(false, true)).toEqual({ enabled: true, banner: false });
    expect(agentPageMode(true, true)).toEqual({ enabled: true, banner: false });
  });

  it("两头都不可用才降级：横幅在场、全部禁用（鸿蒙壳 / 产物未带 agent）", () => {
    expect(agentPageMode(false, false)).toEqual({ enabled: false, banner: true });
  });
});

describe("agentCmdArgs：wasm 导出的位置参数表（契约 §5.3.1 命令映射）", () => {
  it("六命令逐一映射；MCP 命令折空参（web 永不该走到）", () => {
    const args = { cfgJson: "{}", id: 7, since: 5, sessionId: "11" };
    expect(agentCmdArgs("agent_start", args)).toEqual(["{}"]);
    expect(agentCmdArgs("agent_stop", args)).toEqual([7]);
    expect(agentCmdArgs("agent_status", args)).toEqual([7]);
    expect(agentCmdArgs("agent_events", args)).toEqual([7, 5]);
    expect(agentCmdArgs("agent_bind", args)).toEqual(["11"]);
    expect(agentCmdArgs("agent_llm_test", args)).toEqual([]);
    expect(agentCmdArgs("agent_mcp_set", args)).toEqual([]);
  });
});

describe("parseAgentReply：web 回执归一成桌面 invoke 等价值", () => {
  it("agent_start：ok 回执抽 id；失败与缺 id 上抛", () => {
    expect(parseAgentReply<number>("agent_start", '{"ok":true,"id":9}')).toBe(9);
    expect(() => parseAgentReply("agent_start", '{"ok":false,"error":"cfg 解析失败"}')).toThrow("cfg 解析失败");
    expect(() => parseAgentReply("agent_start", '{"ok":true}')).toThrow(/缺 id/);
  });

  it("agent_bind：ok 回执 resolve、失败形态上抛", () => {
    expect(parseAgentReply("agent_bind", '{"ok":true}')).toBeUndefined();
    expect(() => parseAgentReply("agent_bind", '{"ok":false,"error":"run not found"}')).toThrow("run not found");
    expect(() => parseAgentReply("agent_bind", "不是 JSON")).toThrow(/回执异常/);
  });

  it("agent_status / agent_events：成功回执原串透传（与桌面逐字同形）；被折的失败上抛", () => {
    const status = '{"state":"thinking","detail":null,"llmCalls":3,"tokensIn":10,"tokensOut":20,"compactions":0}';
    expect(parseAgentReply<string>("agent_status", status)).toBe(status);
    const events = '{"next":4,"items":[{"ts":1,"tool":"read","ok":true,"ms":2,"summary":"x"}]}';
    expect(parseAgentReply<string>("agent_events", events)).toBe(events);
    expect(() => parseAgentReply("agent_status", '{"ok":false,"error":"run not found"}')).toThrow("run not found");
  });

  it("agent_llm_test：人话文本原样；导出异常折成的 ok:false 抽 error 当文本（桌面展示契约）", () => {
    expect(parseAgentReply<string>("agent_llm_test", "ok")).toBe("ok");
    expect(parseAgentReply<string>("agent_llm_test", "HTTP 401：key 无效")).toBe("HTTP 401：key 无效");
    expect(parseAgentReply<string>("agent_llm_test", '{"ok":false,"error":"wasm 循环未初始化"}')).toBe("wasm 循环未初始化");
  });
});

/* ---------------- /agent 路由（net/links） ---------------- */

describe("路由：/agent", () => {
  afterEach(() => vi.unstubAllGlobals());

  function at(href: string) {
    vi.stubGlobal("window", { location: { href, origin: "https://x.dev" } });
    return parseUrl();
  }

  it("parseUrl：/agent → agent 模式，其余静态页不受影响", () => {
    expect(at("https://x.dev/agent")).toEqual({ mode: "agent" });
    expect(at("https://x.dev/agent/extra")).toEqual({ mode: "menu" });
    expect(at("https://x.dev/ai")).toEqual({ mode: "ai" });
  });

  it("parsePastedLink：完整 URL 识别为 agent 模式；裸「agent」文本不误判（仍走收紧守卫）", () => {
    expect(parsePastedLink("https://x.dev/agent")).toEqual({ mode: "agent" });
    expect(parsePastedLink("agent")).toBeNull();
  });
});

/* ---------------- createSession 的 onChange 注入（net/session 基建钉子） ---------------- */

describe("createSession onChange 注入", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  /** 伪 Tauri 宿主（与 net/session.test.ts 同款）：session_poll 按脚本回快照。 */
  function installTauri() {
    (globalThis as unknown as { window: unknown }).window = {
      setInterval: (...a: Parameters<typeof setInterval>) => setInterval(...a),
      clearInterval: (...a: Parameters<typeof clearInterval>) => clearInterval(...a),
      __TAURI_INTERNALS__: {
        invoke: async (cmd: string, _args: Record<string, unknown>): Promise<unknown> => {
          if (cmd === "session_new") return 7;
          if (cmd === "session_poll") {
            const r = replies.shift();
            return JSON.stringify({ snapshot: r === undefined ? null : r, actions: [] });
          }
          if (cmd === "session_drop") return undefined;
          return "null";
        },
      },
    };
  }

  let replies: (string | null)[];
  /** 每次取新门面：session.ts 的 transportReady 是模块级缓存。 */
  async function freshSessionModule() {
    vi.resetModules();
    return await import("../net/session");
  }

  it("注入的 onChange 接收快照变化；window.goptopOnChange 单槽不被碰", async () => {
    installTauri();
    replies = ['{"userId":"u-a"}', '{"userId":"u-b"}'];
    const { createSession } = await freshSessionModule();
    const onChange = vi.fn();
    const slot = vi.fn();
    (globalThis as unknown as { window: Record<string, unknown> }).window.goptopOnChange = slot;

    const s = await createSession("{}", "http://x/", onChange);
    // create 内的首拍（null → 首帧）走注入回调，slot 一次都不许响
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(slot).not.toHaveBeenCalled();

    const native = s as unknown as { pump(): Promise<void> };
    await native.pump(); // 快照变化 → 注入回调 +1
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(slot, "A' 的通知绝不能顶掉主会话的单槽订阅").not.toHaveBeenCalled();
    expect(s.nativeId?.(), "nativeId 供 agent_bind 登记配对目标").toBe(7);
  });

  it("不传 onChange：回落 window 单槽（主会话零行为变化）", async () => {
    installTauri();
    replies = ['{"userId":"u-a"}', '{"userId":"u-b"}'];
    const { createSession } = await freshSessionModule();
    const slot = vi.fn();
    (globalThis as unknown as { window: Record<string, unknown> }).window.goptopOnChange = slot;

    const s = await createSession("{}", "http://x/");
    expect(slot).toHaveBeenCalledTimes(1);
    await (s as unknown as { pump(): Promise<void> }).pump();
    expect(slot).toHaveBeenCalledTimes(2);
  });

  it("无变化拍不触发注入回调（与既有「无变化」契约一致）", async () => {
    installTauri();
    replies = ['{"userId":"u-a"}', null];
    const { createSession } = await freshSessionModule();
    const onChange = vi.fn();
    const s = await createSession("{}", "http://x/", onChange);
    await (s as unknown as { pump(): Promise<void> }).pump();
    expect(onChange, "snapshot:null = 沿用缓存，不该触发重渲染").toHaveBeenCalledTimes(1);
  });
});
