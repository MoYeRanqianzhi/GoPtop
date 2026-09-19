/**
 * ai/client —— AI 分析 Worker 的主线程门面。
 *
 * 为什么必须走 Worker：wasm 是单线程的，一次 1 秒的搜索会把主线程整个冻住——
 * 棋盘不动、按钮点不动、连"AI 思考中"的动画都停了（实测五子棋 1000ms 预算在
 * wasm 里就是 1000ms 的同步占用，中间没有任何可让步的点）。引擎侧不做分片，
 * 隔离只能靠线程。
 */
import type { AnalyzeRequest, AnalyzeResult, WorkerRequest, WorkerResponse } from "./types";

type Pending = {
  resolve: (r: AnalyzeResult) => void;
  reject: (e: Error) => void;
};

export class AiClient {
  private worker: Worker | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();

  /** 懒建 Worker：只有真要分析时才付 wasm 加载与 1.7MB 权重解压的代价。 */
  private ensureWorker(): Worker {
    if (this.worker) return this.worker;
    const w = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (e: MessageEvent<WorkerResponse>) => {
      const msg = e.data;
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.ok) {
        // warmup 的回复没有 result 字段，调用方也不读它的值
        p.resolve(("result" in msg ? msg.result : undefined) as AnalyzeResult);
      } else {
        p.reject(new Error(msg.error));
      }
    };
    w.onerror = (e) => {
      // Worker 自身崩了（wasm 加载失败、引擎 panic 变 unreachable 等）。必须把
      // 挂起的请求全部失败掉：否则调用方的 Promise 永远不 settle，界面上表现为
      // 「AI 思考中」永不消失，而且再也点不动下一步。
      const err = new Error(`AI Worker 崩溃: ${e.message}`);
      for (const p of this.pending.values()) p.reject(err);
      this.pending.clear();
      this.worker = null;
    };
    this.worker = w;
    return w;
  }

  private send(msg: { kind: "analyze"; req: AnalyzeRequest } | { kind: "warmup" }): Promise<AnalyzeResult> {
    const w = this.ensureWorker();
    const id = this.nextId++;
    return new Promise<AnalyzeResult>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      w.postMessage({ ...msg, id } as WorkerRequest);
    });
  }

  analyze(req: AnalyzeRequest): Promise<AnalyzeResult> {
    return this.send({ kind: "analyze", req });
  }

  /** 预热权重（解压约 56ms）。不调也不出错，只是第一步棋会慢一点。 */
  warmup(): Promise<void> {
    return this.send({ kind: "warmup" }).then(() => undefined);
  }

  dispose() {
    this.worker?.terminate();
    this.worker = null;
    for (const p of this.pending.values()) p.reject(new Error("AI 客户端已释放"));
    this.pending.clear();
  }
}
