/* tslint:disable */
/* eslint-disable */

/**
 * 分析一个局面，返回 `AnalyzeResult` 的 JSON。
 *
 * 请求格式见 [`AnalyzeRequest`]；解析失败返回 `{"error":"..."}`（而不是抛异常，
 * 让 Worker 侧统一按结果对象处理）。
 */
export function analyze_json(req_json: string): string;

/**
 * 预热：加载并把权重解压进内存。Worker 启动时调用一次，
 * 免得第一步棋才付 14MB 解压 + 反序列化的代价（实测约 56ms）。
 */
export function warmup(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly analyze_json: (a: number, b: number, c: number) => void;
    readonly warmup: () => void;
    readonly noru_accumulator_clone: (a: number, b: number) => number;
    readonly noru_accumulator_copy_from: (a: number, b: number) => number;
    readonly noru_accumulator_forward: (a: number, b: number, c: number) => number;
    readonly noru_accumulator_free: (a: number) => void;
    readonly noru_accumulator_new: (a: number, b: number) => number;
    readonly noru_accumulator_refresh: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly noru_accumulator_swap: (a: number) => number;
    readonly noru_accumulator_update: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => number;
    readonly noru_accumulator_update_undo: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => number;
    readonly noru_free_bytes: (a: number, b: number) => void;
    readonly noru_last_error: () => number;
    readonly noru_trainer_adam_step: (a: number, b: number, c: number) => number;
    readonly noru_trainer_backward_bce: (a: number, b: number) => number;
    readonly noru_trainer_backward_raw_mse: (a: number, b: number) => number;
    readonly noru_trainer_forward: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly noru_trainer_free: (a: number) => void;
    readonly noru_trainer_load_fp32: (a: number, b: number, c: number) => number;
    readonly noru_trainer_new: (a: number, b: number, c: number, d: number, e: number, f: bigint, g: number) => number;
    readonly noru_trainer_quantize: (a: number, b: number) => number;
    readonly noru_trainer_save_fp32: (a: number, b: number, c: number) => number;
    readonly noru_trainer_zero_grad: (a: number) => number;
    readonly noru_weights_free: (a: number) => void;
    readonly noru_weights_load: (a: number, b: number, c: number) => number;
    readonly noru_weights_save: (a: number, b: number, c: number) => number;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number, b: number, c: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
