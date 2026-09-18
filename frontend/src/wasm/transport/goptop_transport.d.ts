/* tslint:disable */
/* eslint-disable */

/**
 * wasm 入口：创建会话并接通全部 IO。
 */
export class WasmSession {
    free(): void;
    [Symbol.dispose](): void;
    accept_challenge(): void;
    accept_invite(inviter_id: string, pwd: string | null | undefined, kind: string, size: number, rtc?: string | null): void;
    accept_receipt(receipt_json: string): void;
    accept_spec_receipt(receipt_json: string): void;
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
    toggle_dead(x: number, y: number): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmsession_free: (a: number, b: number) => void;
    readonly wasmsession_accept_challenge: (a: number) => void;
    readonly wasmsession_accept_invite: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => void;
    readonly wasmsession_accept_receipt: (a: number, b: number, c: number) => void;
    readonly wasmsession_accept_spec_receipt: (a: number, b: number, c: number) => void;
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
    readonly wasmsession_toggle_dead: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_1146: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_1154: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_496: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_496_2: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_495: (a: number, b: number) => void;
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
