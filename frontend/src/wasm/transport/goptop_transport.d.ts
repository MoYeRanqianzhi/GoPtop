/* tslint:disable */
/* eslint-disable */

/**
 * wasm 入口：创建会话并接通全部 IO。
 */
export class WasmSession {
    free(): void;
    [Symbol.dispose](): void;
    accept_challenge(): void;
    accept_invite(inviter_id: string, pwd: string | null | undefined, kind: string, size: number, rtc: string | null | undefined, spec: boolean): void;
    accept_receipt(receipt_json: string): void;
    accept_spec_receipt(receipt_json: string): void;
    /**
     * 本会话在 FRONT 注册表的 id（`new_agent` 的会话才有；主会话回空串）。
     */
    agent_id(): string;
    approve_spec(id: string): void;
    back_home(): void;
    confirm_approve(): void;
    confirm_decline(): void;
    confirm_score(): void;
    create_invite(): void;
    disable_spectate(): void;
    /**
     * 事件泵：处理 IO 回调入队的全部事件（JS 以 50ms 定时调用）。
     */
    drain(): void;
    /**
     * ICE 调试（E2E/诊断）：各连接的 tag/ICE 状态/本地与远端候选摘要。
     */
    ice_debug(): string;
    kick_spec(id: string): void;
    mute_spec(id: string, muted: boolean): void;
    /**
     * 构造（cfg_json：{name, serverMode, shareOrigin, kind, size}）。
     */
    constructor(cfg_json: string);
    /**
     * A'（人的 Agent 对局专用会话，JS 所有）——与 [`WasmSession::new`] 逐字同
     * 流程（identity/sessionStorage、presence、Boot、首 drain），三处不同：登记
     * FRONT 注册表（`agent_id` 读号）、emit 走 `on_change`（Some 时）、Nav 受
     * `suppress_nav` 拦。不进全局单槽。同时至多一个 Agent 局（Hub 保证）→
     * 至多一个 A'，旧条目弱引用自动失效。
     */
    static new_agent(cfg_json: string, on_change: Function | null | undefined, suppress_nav: boolean): WasmSession;
    /**
     * 回执解析（粘贴弹窗用）。返回 {ok:true, answer:{...}} 或 {ok:false}。
     */
    parse_answer(text: string): string;
    /**
     * 链接解析（粘贴弹窗分派用；复用 goptop-net links 解析，跨端一致）。
     * 返回 JSON：{ok:true, intent:{mode:"user"|..., ...}} 或 {ok:false}。
     */
    parse_link(text: string): string;
    pass(): void;
    pick_kind(k: string): void;
    pick_size(s: number): void;
    place(x: number, y: number): void;
    reject_challenge(): void;
    reject_spec(id: string): void;
    request_reset(): void;
    request_spec_chat(): void;
    request_swap(): void;
    request_undo(): void;
    resign(): void;
    send_chat(text: string): void;
    server_accept_challenge(): void;
    server_challenge(to: string): void;
    server_reject_challenge(): void;
    set_avatar(data?: string | null): void;
    set_name(name: string): void;
    /**
     * 当前状态快照（UI 渲染契约）。
     */
    snapshot(): string;
    /**
     * 【调试探针】specrtc 解码逐层结果。
     */
    spec_decode_probe(token: string, pwd: string): string;
    /**
     * 安装 50ms 定时泵（App 挂载时调用一次）。
     */
    start_pump(): void;
    /**
     * 【调试探针】观战/服务器内部状态（E2E 诊断用）。
     */
    state_debug(): string;
    /**
     * 当前对局局面的完整序列化（`goptop-ai` 的分析输入）。
     *
     * 与 `goptop-core` 的 `WasmGame::state_json` 同契约：一律由 Rust 序列化，
     * 前端不手工拼。P2P/观战页拿不到本地规则引擎——局面归 `Session.engine` 所有，
     * 而围棋的劫点、提子数只存在于引擎内部，从 TS 侧的状态还原不出来。
     */
    state_json(): string;
    toggle_dead(x: number, y: number): void;
}

/**
 * `agent_bind(sessionId)`：登记人类侧 A' 会话键（配对目标 + 拦截面豁免）。
 *
 * **开局邀请代发**（桌面同款）：登记时若该会话还空置（phase=home）就代发一次
 * `CreateInvite`——A' 建在 /p2p 基座上不会自发邀请，本命令是链路里唯一能替 A'
 * 按下「开启对战」的点；**仅在 home 时发**（我执白方向的 A' 是携链 Boot 的受邀席，
 * 误发会拆掉它正在进行的受理）。会话暂不在注册表也照存登记值（桌面同款不报错）。
 */
export function agent_bind(session_id: string): string;

/**
 * `agent_events(id, since)`：工具日志环（≤200）的增量拉取。`since` 用 u32
 * （wasm-bindgen 的 u64 映射 BigInt，环 ≤200 无需——契约偏差 3）。
 */
export function agent_events(id: number, since: number): string;

/**
 * `agent_llm_test()`：发一次最小真请求验证配置，`"ok"` 或人话错误（异步导出，
 * 真请求最坏 60s 超时 × 3 次尝试）。**不硬编码任何端点/key**：配置全部读 store 链。
 */
export function agent_llm_test(): Promise<string>;

/**
 * `agent_start(cfgJson) -> {"ok":true,"id":N} | {"ok":false,"error"}`。
 *
 * 单局互斥（any_live）与参数校验的文案与桌面逐字一致；任务在 spawn_local 里
 * 自转，本导出只登记就返回。
 */
export function agent_start(cfg_json: string): string;

/**
 * `agent_status(id)`：状态 JSON（与桌面 agent_status 逐字同形：
 * state/detail/stagedMove/llmCalls/tokensIn/tokensOut/compactions）。
 * 运行不在表里回 `{"ok":false,"error":"run not found"}`。
 */
export function agent_status(id: number): string;

/**
 * `agent_stop(id) -> {"ok":true}`（异步导出回 Promise；永不 reject）。
 *
 * 顺序有契约（不可换，与桌面逐字）：认输先落地（对面要看到终局有因，而不是
 * 看到断线），`RESIGN_SETTLE_MS` 定拍给数据面留发送窗口，再置 abort——配对
 * 轮询与循环的既有检查点随即收口，最后拆泵/撤防/摘表。
 */
export function agent_stop(id: number): Promise<string>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmsession_free: (a: number, b: number) => void;
    readonly agent_bind: (a: number, b: number, c: number) => void;
    readonly agent_events: (a: number, b: number, c: number) => void;
    readonly agent_llm_test: () => number;
    readonly agent_start: (a: number, b: number, c: number) => void;
    readonly agent_status: (a: number, b: number) => void;
    readonly agent_stop: (a: number) => number;
    readonly wasmsession_accept_challenge: (a: number) => void;
    readonly wasmsession_accept_invite: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => void;
    readonly wasmsession_accept_receipt: (a: number, b: number, c: number) => void;
    readonly wasmsession_accept_spec_receipt: (a: number, b: number, c: number) => void;
    readonly wasmsession_agent_id: (a: number, b: number) => void;
    readonly wasmsession_approve_spec: (a: number, b: number, c: number) => void;
    readonly wasmsession_back_home: (a: number) => void;
    readonly wasmsession_confirm_approve: (a: number) => void;
    readonly wasmsession_confirm_decline: (a: number) => void;
    readonly wasmsession_confirm_score: (a: number) => void;
    readonly wasmsession_create_invite: (a: number) => void;
    readonly wasmsession_disable_spectate: (a: number) => void;
    readonly wasmsession_drain: (a: number) => void;
    readonly wasmsession_ice_debug: (a: number, b: number) => void;
    readonly wasmsession_kick_spec: (a: number, b: number, c: number) => void;
    readonly wasmsession_mute_spec: (a: number, b: number, c: number, d: number) => void;
    readonly wasmsession_new: (a: number, b: number) => number;
    readonly wasmsession_new_agent: (a: number, b: number, c: number, d: number) => number;
    readonly wasmsession_parse_answer: (a: number, b: number, c: number, d: number) => void;
    readonly wasmsession_parse_link: (a: number, b: number, c: number, d: number) => void;
    readonly wasmsession_pass: (a: number) => void;
    readonly wasmsession_pick_kind: (a: number, b: number, c: number) => void;
    readonly wasmsession_pick_size: (a: number, b: number) => void;
    readonly wasmsession_place: (a: number, b: number, c: number) => void;
    readonly wasmsession_reject_challenge: (a: number) => void;
    readonly wasmsession_reject_spec: (a: number, b: number, c: number) => void;
    readonly wasmsession_request_reset: (a: number) => void;
    readonly wasmsession_request_spec_chat: (a: number) => void;
    readonly wasmsession_request_swap: (a: number) => void;
    readonly wasmsession_request_undo: (a: number) => void;
    readonly wasmsession_resign: (a: number) => void;
    readonly wasmsession_send_chat: (a: number, b: number, c: number) => void;
    readonly wasmsession_server_accept_challenge: (a: number) => void;
    readonly wasmsession_server_challenge: (a: number, b: number, c: number) => void;
    readonly wasmsession_server_reject_challenge: (a: number) => void;
    readonly wasmsession_set_avatar: (a: number, b: number, c: number) => void;
    readonly wasmsession_set_name: (a: number, b: number, c: number) => void;
    readonly wasmsession_snapshot: (a: number, b: number) => void;
    readonly wasmsession_spec_decode_probe: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly wasmsession_start_pump: (a: number) => void;
    readonly wasmsession_state_debug: (a: number, b: number) => void;
    readonly wasmsession_state_json: (a: number, b: number) => void;
    readonly wasmsession_toggle_dead: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_1939: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_1941: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_503: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_503_2: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_502: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number, c: number) => void;
    readonly __wbindgen_export5: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
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
