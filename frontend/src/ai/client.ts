/**
 * ai/client —— AI 分析的门面（**三后端**：Web Worker / Tauri invoke / 鸿蒙 NAPI）。
 *
 * - **Web 端**：引擎跑在 Web Worker 里（`worker.ts`）。wasm 是单线程的，一次
 *   1 秒搜索中间没有任何可让步的点，不隔离就会把主线程整个冻住。
 * - **Tauri 系（桌面 / Android）**：引擎是原生 Rust，经 `invoke("ai_analyze")`
 *   调用，Rust 侧把它派到 blocking 线程池——原生的隔离靠线程池，不需要 Worker，
 *   也不受 wasm 的单线程限制（原生编译还能吃上 noru 的 NEON/AVX2，wasm 只有标量）。
 * - **鸿蒙原生壳**：同样是原生 Rust（NAPI），但 javaScriptProxy 只有同步方法且
 *   **不支持返回 Promise**，所以走「后台算 + 轮询票号」——细节见下面的 `analyzeHarmony`。
 *
 * 三个后端的对外行为完全一致：Promise 化、失败可捕获。
 */
import { harmonyHost, isTauri } from "../net/links";
import type { AnalyzeRequest, AnalyzeResult, WorkerRequest, WorkerResponse } from "./types";

/** AI 轮询间隔：分析预算是 0.3~3 秒，50ms 的粒度用户感知不到，轮询本身也几乎无成本。 */
const HARMONY_POLL_MS = 50;
/** 轮询总上限：预算最长 3 秒，留足余量后仍无结果就判失败（避免 Promise 永不 settle）。 */
const HARMONY_POLL_TIMEOUT_MS = 15000;

type Pending = {
  resolve: (r: AnalyzeResult) => void;
  reject: (e: Error) => void;
};

/**
 * 鸿蒙原生壳的分析：`aiPost` 起后台任务并拿票号，再用定时器轮询 `aiPoll`。
 *
 * **为什么不在 JS 侧也开 Worker**：鸿蒙壳里 Worker 走的是 ArkWeb 的 worker，而
 * 计算实际发生在 Rust（NAPI 的 async work 线程池）——再包一层 Worker 只是多一次
 * 跨线程搬运，隔离本来就由原生侧提供了。
 *
 * 轮询而不是回调：javaScriptProxy 的方法**不支持传函数**（函数不会被 marshalling），
 * 原生侧没法反过来调进 JS。轮询用到的全是同步方法，没有依赖任何「某版本才有的
 * marshalling 行为」。
 */
function analyzeHarmony(reqJson: string): Promise<AnalyzeResult> {
  const bridge = harmonyHost();
  if (!bridge) return Promise.reject(new Error("鸿蒙原生宿主不可用"));
  let ticket: number;
  try {
    ticket = bridge.aiPost(reqJson);
  } catch (e: unknown) {
    return Promise.reject(e instanceof Error ? e : new Error(String(e)));
  }
  if (typeof ticket !== "number") return Promise.reject(new Error("鸿蒙原生宿主未返回票号"));
  return new Promise<AnalyzeResult>((resolve, reject) => {
    const started = Date.now();
    const timer = setInterval(() => {
      let raw = "";
      try {
        raw = bridge.aiPoll(ticket);
      } catch (e: unknown) {
        clearInterval(timer);
        reject(e instanceof Error ? e : new Error(String(e)));
        return;
      }
      if (raw) {
        clearInterval(timer);
        try {
          resolve(JSON.parse(raw) as AnalyzeResult);
        } catch (e: unknown) {
          reject(new Error(`分析回执不是合法 JSON: ${String(e)}`));
        }
        return;
      }
      // 超时兜底：原生侧万一没交付结果，Promise 也不能永远挂着——
      // 挂着的表现是「AI 思考中」永不消失且棋盘再也点不动
      if (Date.now() - started > HARMONY_POLL_TIMEOUT_MS) {
        clearInterval(timer);
        reject(new Error("AI 分析超时（原生宿主未在预期时间内返回结果）"));
      }
    }, HARMONY_POLL_MS);
  });
}

export class AiClient {
  private worker: Worker | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private readonly native = isTauri();
  private readonly harmony = !isTauri() && harmonyHost() !== null;

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
    if (this.harmony) return analyzeHarmony(JSON.stringify(req));
    return this.send({ kind: "analyze", req });
  }

  /**
   * 预热权重（解压约 56ms）。不调也不出错，只是第一步棋会慢一点。
   *
   * 鸿蒙走向同步 `call("ai_warmup")`：56ms 在可接受范围内，且**必须**在起搜索之前
   * 完成——若也丢给后台线程，第一次 analyze 会与解压抢同一份 OnceLock，白白多等。
   */
  async warmup(): Promise<void> {
    if (this.native) {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("ai_warmup");
      return;
    }
    if (this.harmony) {
      harmonyHost()?.call("ai_warmup", "{}");
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
