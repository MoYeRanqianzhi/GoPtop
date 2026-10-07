/**
 * net/session —— P2P 会话门面（**双后端**，与 `game/rules.ts` 同款思路）。
 *
 * - **Web 端**：`WasmSession`（`goptop-transport` 编成 wasm32）。浏览器跑不了原生
 *   代码，wasm 是唯一选择。
 * - **Tauri 系（桌面 / Android）与鸿蒙原生壳**：会话状态机是原生 Rust
 *   （`goptop-transport-native`），经 IPC 直连——**不再绕道 WebView 里的 wasm**。
 *
 * 两边共用同一份状态机（`goptop-net` 的 Event/Effect）与同一份快照契约
 * （`Session::snapshot`），所以上层 `useGameSession` 只认下面这个接口。
 *
 * 方法名逐字对齐 wasm 绑定（`#[wasm_bindgen]` 的方法名），这样上层那三十几处
 * `cmd((s) => s.pick_kind(k))` 一行都不用改。
 *
 * **接口一律 async**（除 `snapshot` 与 `start_pump`）：IPC 本质异步，wasm 侧那几个
 * 同步调用包一层 Promise 只为让调用方只写一份代码——与 `ai/client.ts`、`game/rules.ts`
 * 同一条约定。
 *
 * 拉模式：原生侧的 `session_poll` 顺带把状态机泵一次并带回快照，与 wasm 侧的
 * 50ms `setInterval` 同构——上层 `start_pump()` 的语义两端一致。
 */
import { harmonyHost, isTauri, nav } from "./links";
import { storeDump, storeRemove, storeSet } from "./store";

/** 会话对外的行为面（= `WasmSession` 的公开方法，含返回类型）。 */
export interface GameSession {
  /** 当前快照（**同步**：`goptopOnChange` 回调里立刻要读，原生侧读的是轮询缓存）。 */
  snapshot(): string;
  state_json(): Promise<string>;
  state_debug(): Promise<string>;
  ice_debug(): Promise<string>;
  parse_link(text: string): Promise<string>;
  parse_answer(text: string): Promise<string>;
  start_pump(): void;
  /**
   * 释放会话：原生侧调 `session_drop` 拆掉 Rust 侧会话表项（50ms 泵、presence
   * 订阅、服务器模式的 WSS 随真正 drop 一并退出），wasm 侧 free 掉绑定对象。
   * 上层卸载时必须调——全仓原本没有任何调用点，表现是每次 WebView 重载
   * （Android 转屏 / dev 刷新）永久漏一个僵尸会话。
   */
  dispose(): Promise<void>;

  create_invite(): void;
  accept_invite(inviterId: string, pwd: string | null, kind: string, size: number, rtc: string | null, spec: boolean): void;
  accept_receipt(receiptJson: string): void;
  accept_spec_receipt(receiptJson: string): void;
  accept_challenge(): void;
  reject_challenge(): void;
  server_challenge(to: string): void;
  server_accept_challenge(): void;
  server_reject_challenge(): void;
  place(x: number, y: number): void;
  pass(): void;
  resign(): void;
  request_undo(): void;
  request_reset(): void;
  request_swap(): void;
  confirm_approve(): void;
  confirm_decline(): void;
  toggle_dead(x: number, y: number): void;
  confirm_score(): void;
  send_chat(text: string): void;
  set_name(name: string): void;
  set_avatar(data: string | null): void;
  pick_kind(k: string): void;
  pick_size(s: number): void;
  approve_spec(id: string): void;
  reject_spec(id: string): void;
  kick_spec(id: string): void;
  mute_spec(id: string, muted: boolean): void;
  disable_spectate(): void;
  request_spec_chat(): void;
  back_home(): void;
}

/* ---------------- Web 后端（wasm） ---------------- */

class WasmSessionAdapter implements GameSession {
  constructor(private readonly s: import("../wasm/transport/goptop_transport.js").WasmSession) {}

  snapshot() { return this.s.snapshot(); }
  async state_json() { return this.s.state_json(); }
  async state_debug() { return this.s.state_debug(); }
  async ice_debug() { return this.s.ice_debug(); }
  async parse_link(t: string) { return this.s.parse_link(t); }
  async parse_answer(t: string) { return this.s.parse_answer(t); }
  start_pump() { this.s.start_pump(); }
  /** 释放 = free 掉 Rust 绑定对象（与胶水的 `Symbol.dispose` 同一语义）；线程态的
   *  收尾在 Rust 侧 drop 里做，本层不持句柄。 */
  async dispose() { this.s.free(); }

  create_invite() { this.s.create_invite(); }
  accept_invite(a: string, b: string | null, c: string, d: number, e: string | null, f: boolean) { this.s.accept_invite(a, b, c, d, e, f); }
  accept_receipt(j: string) { this.s.accept_receipt(j); }
  accept_spec_receipt(j: string) { this.s.accept_spec_receipt(j); }
  accept_challenge() { this.s.accept_challenge(); }
  reject_challenge() { this.s.reject_challenge(); }
  server_challenge(to: string) { this.s.server_challenge(to); }
  server_accept_challenge() { this.s.server_accept_challenge(); }
  server_reject_challenge() { this.s.server_reject_challenge(); }
  place(x: number, y: number) { this.s.place(x, y); }
  pass() { this.s.pass(); }
  resign() { this.s.resign(); }
  request_undo() { this.s.request_undo(); }
  request_reset() { this.s.request_reset(); }
  request_swap() { this.s.request_swap(); }
  confirm_approve() { this.s.confirm_approve(); }
  confirm_decline() { this.s.confirm_decline(); }
  toggle_dead(x: number, y: number) { this.s.toggle_dead(x, y); }
  confirm_score() { this.s.confirm_score(); }
  send_chat(t: string) { this.s.send_chat(t); }
  set_name(n: string) { this.s.set_name(n); }
  set_avatar(d: string | null) { this.s.set_avatar(d); }
  pick_kind(k: string) { this.s.pick_kind(k); }
  pick_size(s: number) { this.s.pick_size(s); }
  approve_spec(id: string) { this.s.approve_spec(id); }
  reject_spec(id: string) { this.s.reject_spec(id); }
  kick_spec(id: string) { this.s.kick_spec(id); }
  mute_spec(id: string, m: boolean) { this.s.mute_spec(id, m); }
  disable_spectate() { this.s.disable_spectate(); }
  request_spec_chat() { this.s.request_spec_chat(); }
  back_home() { this.s.back_home(); }
}

/* ---------------- 原生后端（Tauri invoke / 鸿蒙 NAPI） ---------------- */

/**
 * 「怎么把一条命令送过去」——两个原生宿主的唯一差别。
 *
 * Tauri 走 `invoke`（异步 IPC），鸿蒙走 javaScriptProxy（同步）。都包成 Promise，
 * 好让两个宿主共用下面的全部逻辑。
 */
type NativeCall = (cmd: string, argsJson: string) => Promise<string>;

/** 宿主在 Rust 侧做不到、回传给前端执行的平台动作（见 src-tauri/src/session.rs）。 */
type HostAction =
  | { t: "notice"; text: string | null; ms: number | null }
  | { t: "copy"; text: string }
  | { t: "nav"; path: string }
  /** 鸿蒙专有：存储写入回传给 JS 落盘（见 Rust 侧 goptop-ohos/session.rs 的说明）。 */
  | { t: "store"; key: string; value: string | null };

/** 轮询间隔：与 wasm 侧的泵同周期（50ms）。 */
const PUMP_MS = 50;

class NativeSessionAdapter implements GameSession {
  /**
   * 最近一次轮询拿到的快照。**必须缓存**：上层 `snapshot()` 是同步读
   * （`goptopOnChange` 回调里立刻要），而 IPC 是异步的。缓存与真实状态最多差
   * 一个轮询周期（50ms），与 wasm 侧泵的粒度一致。
   */
  private cached = "null";
  private timer: number | null = null;

  private constructor(
    private readonly call: NativeCall,
    private readonly id: number,
  ) {}

  static async create(call: NativeCall, cfgJson: string, href: string): Promise<NativeSessionAdapter> {
    const raw = await call("session_new", JSON.stringify({ cfgJson, href }));
    const id = JSON.parse(raw) as number | null;
    // 建会话失败就抛：静默给个哑会话，上层会以为「连上了但什么都没发生」。
    // 回执原文一并带上：鸿蒙侧的失败形态是 {"error": …}，没有原文设备上无法归因。
    if (typeof id !== "number") throw new Error(`原生会话创建失败: ${raw}`);
    const a = new NativeSessionAdapter(call, id);
    await a.pump();
    return a;
  }

  /**
   * 泵一次：取快照 + 执行宿主动作。与 wasm 侧 `WasmSession::drain` 同义。
   *
   * 快照契约（与 Rust 侧 `session_poll` 配对）：`snapshot` 为 null（或缺失）表示
   * **自上次 poll 以来无变化**——保留缓存、不触发 onChange；不判这一层的表现是
   * 每个空轮询都把缓存覆写成 null，上层同步读 `snapshot()` 拿到坏数据。
   * 宿主动作与快照无关，空轮询也照常执行。
   */
  async pump(): Promise<void> {
    let raw: string;
    try {
      raw = await this.call("session_poll", JSON.stringify({ id: this.id }));
    } catch {
      // 会话已被释放（切页面竞态）或宿主重启：停泵即可，不要每 50ms 抛一次
      this.stop_pump();
      return;
    }
    const r = JSON.parse(raw) as { snapshot: string | null; actions: HostAction[] };
    for (const a of r.actions) this.applyHostAction(a);
    if (typeof r.snapshot !== "string") return;
    const changed = r.snapshot !== this.cached;
    this.cached = r.snapshot;
    // 变了才通知，与 wasm 侧「有变化才 Emit」一致（上层 setSnap 会触发重渲染）
    if (changed) (window as unknown as Record<string, (() => void) | undefined>).goptopOnChange?.();
  }

  /**
   * 执行宿主动作。**钩子是同一份**（`window.goptopNotice` / `goptopCopy`）——
   * 它们在 `useGameSession` 里挂载，与 wasm 侧共用，因此提示条与剪贴板在两端是
   * 同一套行为，不会出现「原生端提示不出来」这类分叉。
   */
  private applyHostAction(a: HostAction): void {
    const w = window as unknown as Record<string, unknown>;
    if (a.t === "notice") {
      (w.goptopNotice as ((s: string) => void) | undefined)?.(JSON.stringify({ text: a.text, ms: a.ms }));
    } else if (a.t === "copy") {
      (w.goptopCopy as ((s: string) => void) | undefined)?.(JSON.stringify({ text: a.text, ok: "已复制" }));
    } else if (a.t === "nav") {
      // 与 wasm 侧 `Effect::Nav` 同款：pushState + popstate
      nav(a.path);
    } else if (a.t === "store") {
      // 鸿蒙侧 Rust 读不到 JS 的存储门面，写入只能回传过来由**本层落盘**——
      // 保证全程只有 JS 一个写者（两个写者会互相覆盖，且不报错）。
      if (a.value === null) storeRemove(a.key);
      else storeSet(a.key, a.value);
    }
  }

  private stop_pump() {
    if (this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
  }

  /**
   * 释放会话：停 JS 泵 + 调 `session_drop` 拆 Rust 侧表项。后半步不能省——
   * Rust 侧只有 `NativeSession` 真正被 drop 才会停 tokio 泵、收 presence/WS 任务，
   * 只停 JS 泵的话会话照旧常驻（这正是前端从未调 `session_drop` 的那个泄漏）。
   */
  async dispose(): Promise<void> {
    this.stop_pump();
    try {
      await this.call("session_drop", JSON.stringify({ id: this.id }));
    } catch (e) {
      // 宿主可能正在卸载/重启：释放失败没有 UI 可报，留痕即可
      console.warn("[session] session_drop 失败", e);
    }
  }

  /** 发一条 UI 命令；发完立刻泵一次（wasm 侧 `cmd` 也是同步泵，不必等下一个周期）。 */
  private cmd(c: unknown): void {
    void this.call("session_cmd", JSON.stringify({ id: this.id, cmdJson: JSON.stringify(c) }))
      .then((raw) => {
        // 命令被拒（拼错标签 / 会话已释放）不能静默——静默的表现是「这个按钮没反应」
        try {
          const r = JSON.parse(raw) as { ok: boolean; error?: string };
          if (!r.ok) console.warn("[session] 命令被拒:", r.error, c);
        } catch { /* 回执不是 JSON，交给下一行统一处理 */ }
        return this.pump();
      })
      .catch((e: unknown) => console.warn("[session] 命令发送失败", e));
  }

  snapshot() { return this.cached; }
  async state_json() { return await this.call("session_state_json", JSON.stringify({ id: this.id })); }
  async state_debug() { return await this.call("session_state_debug", JSON.stringify({ id: this.id })); }
  async ice_debug() { return await this.call("session_ice_debug", JSON.stringify({ id: this.id })); }
  async parse_link(t: string) { return await this.call("session_parse_link", JSON.stringify({ text: t })); }
  async parse_answer(t: string) { return await this.call("session_parse_answer", JSON.stringify({ text: t })); }

  start_pump() {
    if (this.timer !== null) return;
    this.timer = window.setInterval(() => void this.pump(), PUMP_MS);
  }

  /* UI 命令（标签即 `UiCommand` 的 serde 形态，见 goptop-net 的说明） */
  create_invite() { this.cmd("createInvite"); }
  accept_invite(inviterId: string, pwd: string | null, kind: string, size: number, rtc: string | null, spec: boolean) {
    this.cmd({ acceptInvite: { inviterId, pwd, kind, size, rtc, spec } });
  }
  accept_receipt(j: string) { this.cmd({ acceptReceipt: JSON.parse(j) }); }
  accept_spec_receipt(j: string) { this.cmd({ acceptSpecReceipt: JSON.parse(j) }); }
  accept_challenge() { this.cmd("acceptChallenge"); }
  reject_challenge() { this.cmd("rejectChallenge"); }
  server_challenge(to: string) { this.cmd({ serverChallenge: to }); }
  server_accept_challenge() { this.cmd("serverAcceptChallenge"); }
  server_reject_challenge() { this.cmd("serverRejectChallenge"); }
  place(x: number, y: number) { this.cmd({ place: { x, y } }); }
  pass() { this.cmd("pass"); }
  resign() { this.cmd("resign"); }
  request_undo() { this.cmd("requestUndo"); }
  request_reset() { this.cmd("requestReset"); }
  request_swap() { this.cmd("requestSwap"); }
  confirm_approve() { this.cmd("confirmApprove"); }
  confirm_decline() { this.cmd("confirmDecline"); }
  toggle_dead(x: number, y: number) { this.cmd({ toggleDead: { x, y } }); }
  confirm_score() { this.cmd("confirmScore"); }
  send_chat(t: string) { this.cmd({ sendChat: t }); }
  set_name(n: string) { this.cmd({ setName: n }); }
  set_avatar(d: string | null) { this.cmd({ setAvatar: d }); }
  pick_kind(k: string) { this.cmd({ pickKind: k }); }
  pick_size(s: number) { this.cmd({ pickSize: s }); }
  approve_spec(id: string) { this.cmd({ approveSpec: id }); }
  reject_spec(id: string) { this.cmd({ rejectSpec: id }); }
  kick_spec(id: string) { this.cmd({ kickSpec: id }); }
  mute_spec(id: string, m: boolean) { this.cmd({ muteSpec: [id, m] }); }
  disable_spectate() { this.cmd("disableSpectate"); }
  request_spec_chat() { this.cmd("requestSpecChat"); }
  back_home() { this.cmd("backHome"); }
}

/* ---------------- 后端分派 ---------------- */

/** 原生传输的调用通道：桌面/Android 走 Tauri，鸿蒙走 NAPI 代理；都不是则 null。 */
function nativeCall(): NativeCall | null {
  if (isTauri()) {
    return async (cmd, argsJson) => {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke<string>(cmd, JSON.parse(argsJson) as Record<string, unknown>);
    };
  }
  const h = harmonyHost();
  if (h) {
    return async (cmd, argsJson) => {
      let args = JSON.parse(argsJson) as Record<string, unknown>;
      // **建会话时必须带上设置整表**：鸿蒙没有反向通道（ArkTS 不能同步调回 JS），
      // Rust 侧的 `Host::storage_get` 读不到 JS 的存储门面，身份/STUN 线路只能这样传进去。
      // 漏传的表现是「每启一次换一个身份」，界面上完全看不出来。
      if (cmd === "session_new") args = { ...args, settings: storeDump() };
      return h.call(cmd, JSON.stringify(args));
    };
  }
  return null;
}

/**
 * 懒加载 wasm 传输层。**必须缓存 Promise 而不是模块对象**（与 `game/rules.ts`
 * 的 `loadWasm` 同款，那里记着同一场事故）：胶水的 `__wbg_init` 只有「wasm 已就绪」
 * 这一道同步检查、没有 Promise 缓存，并发调用会双双看到未初始化、各实例化一遍，
 * 模块级内存视图被后完成者覆写——先建的那个会话内部指针随即指向错误实例，
 * 首次调用即 `out of bounds`。React StrictMode 的双挂载恰好在同一 tick 里
 * 并发两次 `createSession`，所以这道缓存对 web dev 不是理论问题。
 */
let transportReady: Promise<typeof import("../wasm/transport/goptop_transport.js")> | null = null;

function loadTransport(): Promise<typeof import("../wasm/transport/goptop_transport.js")> {
  if (!transportReady) {
    transportReady = (async () => {
      const mod = await import("../wasm/transport/goptop_transport.js");
      await mod.default();
      return mod;
    })();
  }
  return transportReady;
}

/**
 * 建会话：**能跑原生代码的平台就不该跑 wasm**（与 `game/rules.ts` 的 `pickBackend` 同口径）。
 *
 * wasm 分支用**动态 import**：静态 import 会让三端都把 `goptop_transport_bg.wasm`
 * 拉下来——那正是本轮要消掉的东西（安卓实测的资源时间线里它一直在）。
 */
export async function createSession(cfgJson: string, href: string): Promise<GameSession> {
  const call = nativeCall();
  if (call) return await NativeSessionAdapter.create(call, cfgJson, href);
  const mod = await loadTransport();
  return new WasmSessionAdapter(new mod.WasmSession(cfgJson));
}
