/**
 * ai/client —— 门面契约回归（node 环境，桥与 Worker 都是桩）。
 *
 * 钉两件事：
 * - **鸿蒙错误回执必须 reject**：宿主把反序列化失败/未知命令包成带内 {"error":...}
 *   （crates/goptop-ohos 的 dispatch，与 wasm 侧同形）——不检查的话错误会冒充
 *   AnalyzeResult resolve 出去，胜率条写进 undefined、真实错误被掩盖；
 * - **dispose 的取消契约**：在途请求全部 reject、Worker 拆掉、下一次 analyze 懒
 *   重建——AiPage 的重开/换边靠它把 stale 搜索连同回执一起清掉。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HarmonyHostBridge } from "../net/links";
import type { AnalyzeResult, WorkerResponse } from "./types";
import { AiClient } from "./client";

/** 桩桥的开关（vi.mock 的工厂被提升，须经这个容器才能在用例里换桥）。 */
const harmony = vi.hoisted(() => ({ bridge: null as HarmonyHostBridge | null }));

// isTauri 钉 false：bridge 为 null 走 Web Worker 桩，装上桩桥则走鸿蒙轮询
vi.mock("../net/links", () => ({
  isTauri: () => false,
  harmonyHost: () => harmony.bridge,
}));

/** 一份合法回执，两条路径共用（resolve 断言逐字段比对）。 */
const RESULT: AnalyzeResult = { bestMove: [7, 7], winRate: 0.62, depth: 6, nodes: 1234, elapsedMs: 42 };

/** Worker 桩：只记录 postMessage，回执由用例手动投递；terminate 只做标记。 */
class FakeWorker {
  static all: FakeWorker[] = [];
  onmessage: ((e: { data: WorkerResponse }) => void) | null = null;
  onerror: ((e: { message: string }) => void) | null = null;
  sent: Array<{ id: number }> = [];
  terminated = false;

  constructor() {
    FakeWorker.all.push(this);
  }

  postMessage(msg: { id: number }) {
    this.sent.push(msg);
  }

  terminate() {
    this.terminated = true;
  }

  /** 测试手动模拟 Worker 回执。 */
  reply(msg: WorkerResponse) {
    this.onmessage?.({ data: msg });
  }
}

beforeEach(() => {
  FakeWorker.all = [];
  harmony.bridge = null;
  (globalThis as unknown as { Worker: unknown }).Worker = FakeWorker;
});

describe("鸿蒙路径：错误回执必须 reject", () => {
  /** 轮询桩：按次序吐出 polls 的每一项，取完后停在最后一项。 */
  function stubBridge(polls: string[]): HarmonyHostBridge {
    let i = 0;
    return {
      storeLoad: () => "{}",
      storeSet: () => undefined,
      storeRemove: () => undefined,
      call: () => "null",
      aiPost: () => 1,
      aiPoll: () => polls[Math.min(i++, polls.length - 1)],
    };
  }

  it('带内 {"error":...} 回执 reject、错误文本透传，绝不 resolve', async () => {
    harmony.bridge = stubBridge(["", '{"error":"unknown command: ai_analyze"}']);
    const client = new AiClient();
    // 首轮询故意回空串（未完成），第二轮才交出错误回执——锁住「轮询循环里检查」
    // 这个位置，而不是只在首轮生效
    await expect(client.analyze({ state: {}, myColor: "Black", budgetMs: 100, wantMove: true })).rejects.toThrow(
      "unknown command: ai_analyze",
    );
  });

  it("合法回执逐字段 resolve（正常路径不受影响）", async () => {
    harmony.bridge = stubBridge([JSON.stringify(RESULT)]);
    const client = new AiClient();
    await expect(client.analyze({ state: {}, myColor: "White", budgetMs: 100, wantMove: false })).resolves.toEqual(
      RESULT,
    );
  });
});

describe("Web Worker 路径：dispose 的取消契约", () => {
  it("dispose 后在途请求全部 reject、Worker 拆掉，下一次 analyze 懒重建", async () => {
    const client = new AiClient();
    const pending = client.analyze({ state: {}, myColor: "Black", budgetMs: 1, wantMove: true });
    expect(FakeWorker.all.length).toBe(1);

    client.dispose();
    await expect(pending).rejects.toThrow("AI 客户端已释放");
    expect(FakeWorker.all[0].terminated).toBe(true);

    const next = client.analyze({ state: {}, myColor: "Black", budgetMs: 1, wantMove: true });
    expect(FakeWorker.all.length, "dispose 不立刻重建，下一次 analyze 才懒建").toBe(2);
    const w = FakeWorker.all[1];
    w.reply({ id: w.sent[0].id, ok: true, result: RESULT });
    await expect(next).resolves.toEqual(RESULT);
  });
});
