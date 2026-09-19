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
     * 采纳全量快照（SyncState）：按 `history`（坐标/"pass" 序列）从新局全量重放，
     * 重放终态必须与快照的棋盘/行棋方一致（胜者允许快照多出认输/五连胜者而重放为
     * None 的放宽），一致则采**重放结果**——captures/ko_point/scoring 全部正确；
     * 重放失败或与快照矛盾（远端脏数据/伪造）整体拒绝返回 false，绝不带病采纳。
     * TS 侧只在收到快照时调用。
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
     * 围棋连续双 Pass 自动进入计分态（scoring:true 随回复返回）。
     */
    pass(): string;
    /**
     * 重开（同种类）。
     */
    reset(): void;
    /**
     * 终局区域计分（中国规则数子法）：把 `dead_json`（`[{"x":..,"y":..}]`）视为
     * 死子移除后计分。返回 ok/black/white（含贴目 7.5）/黑地/白地/死子数/winner。
     * scoring 态之外调用也允许（UI 可随时预览形势），死子坐标非法返回 ok:false。
     */
    score(dead_json: string): string;
    /**
     * 当前局面的完整序列化（`goptop-ai` 的分析输入）。
     *
     * AI 要的不只是棋盘：围棋的劫点、提子数、计分态都进搜索，而这些只有
     * `GameState` 持有。让规则引擎自己吐局面，AI 拿到的就与规则真源逐字一致——
     * 前端手工拼 JSON 不可行（`BoardVariant` 的形态是 `{"B15":{"cells":[[...]]}}`，
     * 嵌套且随尺寸变形，拼错只会表现为 AI 下出怪棋，不会报错）。
     *
     * 序列化失败返回 `"null"`：`GameState` 只由可序列化字段组成，正常路径不会走到。
     */
    state_json(): string;
    /**
     * 落子（唯一规则入口）：合法则更新内部棋盘并返回权威棋盘/提子/胜负/劫点/计分态；
     * 非法（占据/越界/自杀/劫/计分态/终局）返回 ok:false，内部状态不变。
     * 越界/占据/劫/终局判定都在 GameState::try_play 内。
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
    readonly wasmgame_score: (a: number, b: number, c: number, d: number) => void;
    readonly wasmgame_state_json: (a: number, b: number) => void;
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
