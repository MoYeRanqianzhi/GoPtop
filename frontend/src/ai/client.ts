/**
 * ai/client —— AI 分析的门面（**双后端**）。
 *
 * - **Web 端**：引擎跑在 Web Worker 里（`worker.ts`）。wasm 是单线程的，一次
 *   1 秒搜索中间没有任何可让步的点，不隔离就会把主线程整个冻住。
 * - **Tauri 系（桌面 / Android）**：引擎是原生 Rust，经 `invoke("ai_analyze")`
 *   调用，Rust 侧把它派到 blocking 线程池——原生的隔离靠线程池，不需要 Worker，
 *   也不受 wasm 的单线程限制（原生编译还能吃上 noru 的 NEON/AVX2，wasm 只有标量）。
 *
 * 两个后端的对外行为完全一致：Promise 化、失败可捕获。
 */
import { isTauri } from "../net/links";
import type { AnalyzeRequest, AnalyzeResult, WorkerRequest, WorkerResponse } from "./types";

type Pending = {
  resolve: (r: AnalyzeResult) => void;
  reject: (e: Error) => void;
};

export class AiClient {
  private worker: Worker | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private readonly native = isTauri();

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

  async analyze(req: AnalyzeRequest): Promise<AnalyzeResult> {
    if (this.native) {
      const { invoke } = await import("@tauri-apps/api/core");
      const raw = await invoke<string>("ai_analyze", { reqJson: JSON.stringify(req) });
      return JSON.parse(raw) as AnalyzeResult;
    }
    return this.send({ kind: "analyze", req });
  }

  /** 预热权重（解压约 56ms）。不调也不出错，只是第一步棋会慢一点。 */
  async warmup(): Promise<void> {
    if (this.native) {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("ai_warmup");
      return;
    }
    await this.send({ kind: "warmup" });
  }

  dispose() {
    this.worker?.terminate();
    this.worker = null;
    for (const p of this.pending.values()) p.reject(new Error("AI 客户端已释放"));
    this.pending.clear();
  }
}
