/* tslint:disable */
/* eslint-disable */

/**
 * 行为化绑定：一个实例持一局 `GameState`。前端每个对局页面/本地页各持一个。
 */
export class WasmGame {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * 采纳全量快照（SyncState）：棋盘/行棋方/胜者直接采用快照（不经规则——
     * 快照可能来自任何合法序列），历史按坐标/"pass" 序列重建（黑白交替、
     * 黑先，Pass 原样保留），供后续 undo_last 重放。TS 侧只在收到快照时调用。
     */
    adopt(board_json: string, to_move: string, winner: string, history_json: string): boolean;
    /**
     * 当前逻辑尺寸。
     */
    boardSize(): number;
    /**
     * 创建对局（静态工厂；wasm-bindgen 不允许构造函数返回 Option）。
     * kind_json 见模块注释。非法尺寸组合返回 None 而非触达核心层断言——
     * release 下 panic="abort"，断言即 wasm trap（白屏），边界必须自己挡
     * （2026-09-14 审查 #5 P2-3）。
     */
    static new_game(kind_json: string): WasmGame | undefined;
    /**
     * 停一手：引擎翻转行棋方并把 Pass 记入历史，保证后续 undo/adopt 重放
     * 与真实序列一致（收到 Move{Pass} 或未来本地停一手都走这里）。
     */
    pass(): string;
    /**
     * 重开（同种类）。
     */
    reset(): void;
    /**
     * 落子（唯一规则入口）：合法则更新内部棋盘并返回权威棋盘/提子/胜负；
     * 非法（占据/越界/自杀/终局）返回 ok:false，内部状态不变。
     * 越界/占据/终局判定都在 GameState::try_play 内。
     */
    try_place(x: number, y: number): string;
    /**
     * 撤销最后一手：弹出一手并全量重放（≤361 步，开销可忽略），
     * 返回回退后的权威棋盘。空历史返回 ok:false。
     */
    undo_last(): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmgame_free: (a: number, b: number) => void;
    readonly wasmgame_adopt: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => number;
    readonly wasmgame_boardSize: (a: number) => number;
    readonly wasmgame_new_game: (a: number, b: number) => number;
    readonly wasmgame_pass: (a: number, b: number) => void;
    readonly wasmgame_reset: (a: number) => void;
    readonly wasmgame_try_place: (a: number, b: number, c: number, d: number) => void;
    readonly wasmgame_undo_last: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
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
