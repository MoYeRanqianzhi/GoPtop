/**
 * ai/useWinRate —— 把「局面变化 → 后台分析 → 胜率 + 走势」接成一个 hook。
 *
 * 设计要点：
 * - **全页共享一个 Worker**：每个 AiClient 都会另加载一份 wasm 与 1.7MB 权重，
 *   开两个就是双倍内存与双倍解压时间。全局单例。
 * - **每手触发一次，不排队**：新的一手到来时旧请求的结果直接丢弃（用 cancelled
 *   标记），否则用户悔棋后会看到上一手的胜率覆盖当前局面。
 * - **失败降级而非卡死**：wasm 没构建、引擎 panic 都走 error 分支，界面隐藏胜率
 *   区块，绝不让「思考中」永远转下去。
 */
import { useEffect, useRef, useState } from "react";
import { AiClient } from "./client";
import type { AiColor } from "./types";

let shared: AiClient | null = null;

/** 全局共享的 AI 客户端（多个 hook 实例复用同一个 Worker）。 */
export function sharedAiClient(): AiClient {
  if (!shared) shared = new AiClient();
  return shared;
}

export type WinRatePoint = { move: number; winRate: number };

export type WinRateState = {
  /** 我方胜率 0..1；null 表示尚无结果。 */
  winRate: number | null;
  /**
   * 逐手胜率，**按手数有序**而非按完成顺序追加。
   *
   * 每一手的分析是异步的，而快速连续落子（AI 秒回）会让前一手的结果在被取消时
   * 丢掉——按完成顺序追加的话，点会与手数错位，走势图的横轴就不再是手数了。
   */
  series: WinRatePoint[];
  /** 正在分析（数字保持上一次的值）。 */
  thinking: boolean;
  /** 分析不可用时的原因（引擎未构建、崩溃等）。 */
  error: string | null;
};

export function useWinRate(opts: {
  /**
   * 取当前局面的 JSON（由规则引擎的 state_json() 给出）。
   * 只在 moveCount 变化时调用，不需要保证引用稳定。
   */
  getState: () => unknown | null;
  myColor: AiColor;
  /** 手数：变化即触发一次分析；归零则清空走势（重开）。 */
  moveCount: number;
  /** 每步思考预算（毫秒）。 */
  budgetMs: number;
  enabled: boolean;
}): WinRateState {
  const [winRate, setWinRate] = useState<number | null>(null);
  const [series, setSeries] = useState<WinRatePoint[]>([]);
  const [thinking, setThinking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 取局面的函数每次渲染都是新引用，放进依赖会让 effect 每帧重跑；
  // 真正的事件源是 moveCount，用 ref 拿最新的取法。
  const getStateRef = useRef(opts.getState);
  getStateRef.current = opts.getState;

  useEffect(() => {
    // 手数归零＝重开或退到空盘：走势必须清空，且**必须就此返回**。
    // 少了这个 return 的话，紧接着又会把 move=0 当成一手指去分析并记进序列，
    // 窗口坐标 (move - head)/windowSize 随之算出负数（实测曲线首点跑到 x=-35）。
    // 空盘也不需要分析——50/50 是绘制层的初始态，不是搜索结果。
    if (opts.moveCount === 0) {
      setSeries([]);
      setWinRate(null);
      setThinking(false);
      return;
    }
    if (!opts.enabled) return;
    const state = getStateRef.current();
    if (!state) return;

    let cancelled = false;
    const move = opts.moveCount;
    setThinking(true);
    sharedAiClient()
      .analyze({ state, myColor: opts.myColor, budgetMs: opts.budgetMs, wantMove: false })
      .then((r) => {
        if (cancelled) return;
        setWinRate(r.winRate);
        // 按手数归位：同手数重算（悔棋后重下）覆盖旧值，避免曲线出现重复点
        setSeries((s) => {
          const next = s.filter((p) => p.move !== move);
          next.push({ move, winRate: r.winRate });
          next.sort((a, b) => a.move - b.move);
          return next;
        });
        setError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setThinking(false);
      });
    return () => {
      cancelled = true;
    };
    // getState 走 ref；myColor/enabled/budgetMs 变化时重算当前局面是正确行为
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.moveCount, opts.myColor, opts.enabled, opts.budgetMs]);

  return { winRate, series, thinking, error };
}
