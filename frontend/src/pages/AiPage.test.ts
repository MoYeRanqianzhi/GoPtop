/**
 * AiPage 的 AI 回调编排（runAiTurn）回归。
 *
 * 为什么测函数而不是组件：本仓无 jsdom（vitest 默认 environment=node），React
 * 组件挂载不了；故把 effect 体内的异步编排抽成可独立驱动的 runAiTurn，组件把
 * 「cancelled || 代次过期」折叠进 isStale 传进来。测试直接操纵 isStale 复刻
 * 「重开/换边发生在分析在途」——AI 执黑思考中点重开时，effect 依赖一个都不变、
 * cleanup 不会执行，只有代次能拦住旧回执把选点落到刚清空的棋盘上。
 */
import { describe, expect, it, vi } from "vitest";
import type { AnalyzeResult } from "../ai/types";
import type { PlaceResult } from "../game/rules";
import type { StoneColor } from "../net/protocol";
import { runAiTurn } from "./AiPage";

const RESULT: AnalyzeResult = { bestMove: [3, 4], winRate: 0.5, depth: 1, nodes: 1, elapsedMs: 1 };

const okPlace = (board: StoneColor[][] = []): Promise<PlaceResult> =>
  Promise.resolve({ ok: true, board, captured: [], toMove: "white", winner: null });

/** 可控的分析桩：analyze 挂起直到用例手动放行。 */
function deferredAnalyze() {
  let release!: (r: AnalyzeResult) => void;
  const promise = new Promise<AnalyzeResult>((r) => {
    release = r;
  });
  return { promise, release };
}

describe("AI 回调编排：代次守卫（重开/换边作废在途回执）", () => {
  it("分析在途时局面被重开：不 place、不改任何状态，thinking 也不碰", async () => {
    let stale = false;
    const { promise, release } = deferredAnalyze();
    const place = vi.fn();
    const setThinking = vi.fn();
    const setAiError = vi.fn();
    const onPlaced = vi.fn();
    const analyze = vi.fn(() => promise);

    const turn = runAiTurn({
      isStale: () => stale,
      getState: () => Promise.resolve({ size: 19 }),
      analyze,
      place,
      setThinking,
      setAiError,
      onPlaced,
    });
    // 等分析真正挂起（复刻：AI 已把请求发出去、用户此刻点了「重开」）
    await vi.waitFor(() => expect(analyze).toHaveBeenCalled());
    stale = true;
    release(RESULT);
    await turn;

    expect(place, "旧局面的选点必须被丢弃，不能落到清空后的棋盘上").not.toHaveBeenCalled();
    expect(onPlaced).not.toHaveBeenCalled();
    expect(setAiError).not.toHaveBeenCalled();
    expect(setThinking, "过期回执连 thinking 都不许碰——新代次才是它的主人").not.toHaveBeenCalled();
  });

  it("取局面返回前已重开：连 analyze 都不起，thinking 收尾一并跳过", async () => {
    const analyze = vi.fn();
    const setThinking = vi.fn();
    await runAiTurn({
      isStale: () => true,
      getState: () => Promise.resolve({ size: 19 }),
      analyze,
      place: vi.fn(),
      setThinking,
      setAiError: vi.fn(),
      onPlaced: vi.fn(),
    });
    expect(analyze).not.toHaveBeenCalled();
    expect(setThinking).not.toHaveBeenCalled();
  });

  it("重开把 Worker terminate 掉导致分析 reject：过期错误不得写进新局面", async () => {
    let stale = false;
    let rejectFn!: (e: Error) => void;
    const setAiError = vi.fn();
    const turn = runAiTurn({
      isStale: () => stale,
      getState: () => Promise.resolve({ size: 19 }),
      analyze: () =>
        new Promise<AnalyzeResult>((_, reject) => {
          rejectFn = reject;
        }),
      place: vi.fn(),
      setThinking: vi.fn(),
      setAiError,
      onPlaced: vi.fn(),
    });
    await vi.waitFor(() => expect(typeof rejectFn).toBe("function"));
    stale = true; // 重开：代次作废 + sharedAiClient().dispose() 使在途 analyze reject
    rejectFn(new Error("AI 客户端已释放"));
    await turn;
    expect(setAiError, "dispose 的取消性 reject 必须被吞掉，不能在空盘上报错").not.toHaveBeenCalled();
  });

  it("正常路径（无重开）：选点经 place 落子并把结果交给 onPlaced", async () => {
    let placed: { res: PlaceResult; move: { x: number; y: number } } | null = null;
    await runAiTurn({
      isStale: () => false,
      getState: () => Promise.resolve({ size: 19 }),
      analyze: () => Promise.resolve(RESULT),
      place: (x, y) => {
        expect([x, y]).toEqual([3, 4]);
        return okPlace([["black"]]);
      },
      setThinking: vi.fn(),
      setAiError: vi.fn(),
      onPlaced: (res, move) => {
        placed = { res, move };
      },
    });
    expect(placed).toEqual({ res: await okPlace([["black"]]), move: { x: 3, y: 4 } });
  });
});
