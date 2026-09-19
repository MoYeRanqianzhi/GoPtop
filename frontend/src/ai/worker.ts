/// <reference lib="webworker" />
/**
 * ai/worker —— AI 引擎的宿主线程（`goptop_ai` wasm 只在这里加载）。
 *
 * 与主线程完全隔离：1 秒的搜索只冻结这个线程，界面照常响应。代价是每次分析要
 * 跨线程传一份局面 JSON（几百字节到几 KB，可忽略）。
 *
 * wasm 初始化失败（产物没构建、权重解压异常）会让 `ready` 变成 rejected 的 Promise，
 * 后续每个请求都会拿到同一个错误——调用方据此降级（隐藏胜率条、关闭人机入口），
 * 而不是让界面卡在"思考中"。
 */
import init, { analyze_json, warmup as wasmWarmup } from "../wasm-ai/goptop_ai";
import type { AnalyzeResult, WorkerRequest, WorkerResponse } from "./types";

let ready: Promise<void> | null = null;

function ensureReady(): Promise<void> {
  if (!ready) {
    ready = init().then(() => {
      wasmWarmup();
    });
  }
  return ready;
}

const ctx = self as unknown as DedicatedWorkerGlobalScope;

ctx.onmessage = async (e: MessageEvent<WorkerRequest>) => {
  const msg = e.data;
  const reply = (r: WorkerResponse) => ctx.postMessage(r);
  try {
    await ensureReady();
    if (msg.kind === "warmup") {
      reply({ id: msg.id, ok: true, warmed: true });
      return;
    }
    const raw = analyze_json(JSON.stringify(msg.req));
    const parsed = JSON.parse(raw) as AnalyzeResult | { error: string };
    if ("error" in parsed) {
      reply({ id: msg.id, ok: false, error: parsed.error });
      return;
    }
    reply({ id: msg.id, ok: true, result: parsed });
  } catch (err) {
    reply({ id: msg.id, ok: false, error: err instanceof Error ? err.message : String(err) });
  }
};
