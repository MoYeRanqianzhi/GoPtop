/**
 * rules —— 规则引擎的前端门面（**双后端**）。
 *
 * - **Web 端**：走 wasm（`goptop-core` 编成 wasm32）。浏览器跑不了原生代码，
 *   wasm 是唯一选择。
 * - **Tauri 系（桌面 / Android）**：走原生 Rust，经 `invoke` 直接调 `goptop-core`。
 *   与 wasm 共用同一份 `json_api` 契约，只是宿主不同。
 *
 * 为什么原生端不该绕道 wasm：`goptop.exe` / `libgoptop_lib.so` 本来就是 Rust
 * 宿主，直接链接即可，绕道 WebView 只是白白多一层边界（并把 wasm 的主线程
 * 限制也一起带进来）。
 *
 * 接口一律 async：IPC 本质异步；Web 端的 wasm 调用虽然同步，但统一成 async
 * 才能让调用方只写一份代码。
 *
 * 数据面：与 net/protocol.ts 同构（小写颜色字符串、行优先二维数组、逻辑尺寸棋盘），
 * board 可直接 setBoard。
 */
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import { harmonyNative, isTauri } from "../net/links";

/** wasm 规则判定结果：ok 时 board 为权威棋盘（围棋含提子效果）。 */
export type PlaceResult =
  | { ok: true; board: StoneColor[][]; captured: Coord[]; toMove: "black" | "white"; winner: StoneColor | null }
  | { ok: false; error: string };

/** undo_last 的返回（无 captured 字段，提子已还原进 board）。 */
export type UndoResult =
  | { ok: true; board: StoneColor[][]; toMove: "black" | "white"; winner: StoneColor | null }
  | { ok: false; error: string };

/** 两个后端共同的行为面。 */
interface Backend {
  newGame(kindJson: string): Promise<boolean>;
  place(x: number, y: number): Promise<PlaceResult | null>;
  pass(): Promise<PlaceResult | null>;
  undo(): Promise<UndoResult | null>;
  reset(): Promise<void>;
  adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]): Promise<void>;
  stateJson(): Promise<unknown | null>;
  dispose(): Promise<void>;
}

/* ---------------- Web 后端（wasm） ---------------- */

/**
 * 懒加载 wasm：原生端根本不需要它，静态 import 会把 135KB 白拉进来。
 *
 * **必须缓存 Promise 而不是模块对象**：`init()` 是异步的，若只缓存模块，
 * 并发调用会双双看到缓存为空、各跑一次 `init()`，wasm 被实例化两遍后内存视图
 * 互相失效——实测表现为 `RuntimeError: memory access out of bounds`，
 * 而且第一个落子仍成功、之后才炸，极具迷惑性。
 */
let wasmReady: Promise<typeof import("../wasm/goptop_core")> | null = null;

function loadWasm(): Promise<typeof import("../wasm/goptop_core")> {
  if (!wasmReady) {
    wasmReady = (async () => {
      const mod = await import("../wasm/goptop_core");
      await mod.default();
      return mod;
    })();
  }
  return wasmReady;
}

class WasmBackend implements Backend {
  private game: import("../wasm/goptop_core").WasmGame | null = null;

  async newGame(kindJson: string): Promise<boolean> {
    const { WasmGame } = await loadWasm();
    // 胶水把 Option<WasmGame> 映射为 undefined：null 与之运行时等价，归一存 null
    this.game = WasmGame.new_game(kindJson) ?? null;
    return this.game !== null;
  }

  async place(x: number, y: number): Promise<PlaceResult | null> {
    if (!this.game) return null;
    return JSON.parse(this.game.try_place(x, y)) as PlaceResult;
  }

  async pass(): Promise<PlaceResult | null> {
    if (!this.game) return null;
    return JSON.parse(this.game.pass()) as PlaceResult;
  }

  async undo(): Promise<UndoResult | null> {
    if (!this.game) return null;
    return JSON.parse(this.game.undo_last()) as UndoResult;
  }

  async reset(): Promise<void> {
    this.game?.reset();
  }

  async adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]): Promise<void> {
    if (!this.game) return;
    // winner 的线上约定是字符串："null"/"" 表示无胜者（wasm 侧 match "null" | "" → None），
    // 形参是 &str 不接受 JS null；传 "empty" 会被判非法而整份快照被丢弃——勿简化成 winner ?? null。
    const ok = this.game.adopt(JSON.stringify(board), toMove, winner ?? "null", JSON.stringify(history));
    if (!ok) console.warn("[rules] adopt 被引擎拒绝：快照与当前对局尺寸/格式不符，已忽略该快照");
  }

  async stateJson(): Promise<unknown | null> {
    if (!this.game) return null;
    return JSON.parse(this.game.state_json()) as unknown;
  }

  async dispose(): Promise<void> {
    this.game = null;
  }
}

/* ---------------- 原生后端（Tauri invoke） ---------------- */

class NativeBackend implements Backend {
  private id: number | null = null;

  async newGame(kindJson: string): Promise<boolean> {
    const { invoke } = await import("@tauri-apps/api/core");
    // 先摘掉旧实例再 await：invoke 失败会抛（command 未注册、参数名不匹配、Rust panic），
    // 若把赋值留在 await 之后，this.id 会继续指着上一局——尺寸已经不同，之后每次
    // place 都拿旧尺寸的棋盘覆盖 UI（wasm 后端此时成了 null，两端行为分叉）。
    const stale = this.id;
    this.id = null;
    const id = await invoke<number | null>("game_new", { kindJson });
    this.id = id ?? null;
    // 旧对局活在 Rust 的 HashMap 里，不显式 drop 就永久留着——切规则/尺寸每切一次留一份
    if (stale !== null) void invoke("game_drop", { id: stale });
    return this.id !== null;
  }

  private async call<T>(cmd: string, args: Record<string, unknown>): Promise<T | null> {
    if (this.id === null) return null;
    const { invoke } = await import("@tauri-apps/api/core");
    return (await invoke<T>(cmd, { id: this.id, ...args })) as T;
  }

  async place(x: number, y: number): Promise<PlaceResult | null> {
    const raw = await this.call<string>("game_place", { x, y });
    return raw ? (JSON.parse(raw) as PlaceResult) : null;
  }

  async pass(): Promise<PlaceResult | null> {
    const raw = await this.call<string>("game_pass", {});
    return raw ? (JSON.parse(raw) as PlaceResult) : null;
  }

  async undo(): Promise<UndoResult | null> {
    const raw = await this.call<string>("game_undo", {});
    return raw ? (JSON.parse(raw) as UndoResult) : null;
  }

  async reset(): Promise<void> {
    await this.call<void>("game_reset", {});
  }

  async adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]): Promise<void> {
    const ok = await this.call<boolean>("game_adopt", {
      boardJson: JSON.stringify(board),
      toMove,
      winner: winner ?? "null",
      historyJson: JSON.stringify(history),
    });
    if (!ok) console.warn("[rules] adopt 被引擎拒绝：快照与当前对局尺寸/格式不符，已忽略该快照");
  }

  async stateJson(): Promise<unknown | null> {
    const raw = await this.call<string>("game_state_json", {});
    return raw && raw !== "null" ? (JSON.parse(raw) as unknown) : null;
  }

  async dispose(): Promise<void> {
    await this.call<void>("game_drop", {});
    this.id = null;
  }
}

/* ---------------- 鸿蒙后端（NAPI 原生宿主） ---------------- */

/**
 * 鸿蒙壳：规则判定直连 Rust（NAPI），与 Tauri 那条一样是**原生**，不是 wasm。
 *
 * 与 [`NativeBackend`] 的差别只在「地址」：Tauri 走 `invoke` 的异步 IPC，鸿蒙走
 * javaScriptProxy 的**同步**调用。同步是本层刻意保留的——规则命令是微秒级的纯计算，
 * 包成 Promise 只会让调用方多一次无谓的微任务跳转（AI 那条才需要异步，见 ai/client.ts）。
 *
 * 返回值形态：原生侧已经把 json_api 的字符串契约解成了对象（见 crates/goptop-ohos
 * 的分发表），所以这里**不再 JSON.parse 一次**——多解一层会在字符串里再套一层引号。
 */
class HarmonyBackend implements Backend {
  private id: number | null = null;

  /** 桥在页面存活期内不会变，取一次即可；取不到说明壳没带原生模块。 */
  private get bridge() {
    return harmonyNative();
  }

  async newGame(kindJson: string): Promise<boolean> {
    const b = this.bridge;
    if (!b) return false;
    // 与 NativeBackend 同款：先摘旧 id 再取新局，失败时不至于继续用旧局落子
    const stale = this.id;
    this.id = null;
    const id = b.call("game_new", JSON.stringify({ kindJson }));
    this.id = typeof id === "number" ? id : null;
    if (stale !== null) b.call("game_drop", JSON.stringify({ id: stale }));
    return this.id !== null;
  }

  private call<T>(cmd: string, args: Record<string, unknown> = {}): T | null {
    const b = this.bridge;
    if (!b || this.id === null) return null;
    const raw = b.call(cmd, JSON.stringify({ id: this.id, ...args }));
    return (raw ?? null) as T | null;
  }

  async place(x: number, y: number): Promise<PlaceResult | null> {
    return this.call<PlaceResult>("game_place", { x, y });
  }

  async pass(): Promise<PlaceResult | null> {
    return this.call<PlaceResult>("game_pass");
  }

  async undo(): Promise<UndoResult | null> {
    return this.call<UndoResult>("game_undo");
  }

  async reset(): Promise<void> {
    this.call<void>("game_reset");
  }

  async adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]): Promise<void> {
    const ok = this.call<boolean>("game_adopt", {
      boardJson: JSON.stringify(board),
      toMove,
      winner: winner ?? "null",
      historyJson: JSON.stringify(history),
    });
    if (!ok) console.warn("[rules] adopt 被引擎拒绝：快照与当前对局尺寸/格式不符，已忽略该快照");
  }

  async stateJson(): Promise<unknown | null> {
    return this.call<unknown>("game_state_json");
  }

  async dispose(): Promise<void> {
    this.call<void>("game_drop");
    this.id = null;
  }
}

/**
 * 挑后端：**能跑原生代码的平台就不该跑 wasm**。
 * - Tauri（桌面/Android）：`__TAURI_INTERNALS__` → invoke；
 * - 鸿蒙原生壳：`window.goptopNative` → javaScriptProxy；
 * - 其余（浏览器，以及没带原生模块的老鸿蒙壳）→ wasm。
 */
function pickBackend(): Backend {
  if (isTauri()) return new NativeBackend();
  if (harmonyNative()) return new HarmonyBackend();
  return new WasmBackend();
}

/** 规则引擎门面：按运行环境挑后端（Web → wasm；Tauri 系 / 鸿蒙原生壳 → 直连 Rust）。 */
export class RulesEngine {
  private backend: Backend = pickBackend();

  /**
   * 最近一次 `newGame` 的落地 Promise（两个后端都异步：wasm 要等模块 init，
   * native 要等 IPC 往返）。
   *
   * **为什么必须留一手**：调用方是 effect，一律 `void engine.newGame(...)` 不等它；
   * 而同一个 commit 里的下一个 effect 立刻就要 `stateJson()`。不等就会撞上
   * 「新局还没建好」，且两个后端各有各的错法：
   * - native：`newGame` 已把 `id` 摘成 null（见那里的说明），此刻 `stateJson()`
   *   返回 null，而 AI 落子 effect 拿到 null 只是静默 return——AI 执黑时切尺寸，
   *   空棋盘、状态行停在「黑 落子」，棋盘因 `toMove !== humanColor` 恒禁用且不再
   *   恢复（重开也不重跑该 effect）。
   * - wasm：`this.game` 还是**上一局**（赋值在 `await loadWasm()` 之后），
   *   `stateJson()` 会取回旧局的快照（实测：9 路切 19 路时取回 size=9），
   *   AI 于是照着幽灵局面选点——不报错、不卡死，只是下错。
   */
  private booting: Promise<void> = Promise.resolve();

  /** 开新局（kind/size 与前端选择器同源）。Rust 对非法组合是断言即 trap
   *（会掀翻 React 树），故在边界先挡：gomoku 只接受 15，go 只接受 9/13/19。 */
  async newGame(kind: GameKind, size: Size): Promise<void> {
    const valid = kind === "gomoku" ? size === 15 : size === 9 || size === 13 || size === 19;
    if (!valid) return;
    // GameKind serde 外部标签格式：{"Gomoku":{"size":15}} / {"Go":{"size":9}}
    const kindJson = JSON.stringify(kind === "gomoku" ? { Gomoku: { size } } : { Go: { size } });
    this.booting = this.backend.newGame(kindJson).then(() => {});
  }

  /** 落子判定：占据/越界/自杀/终局由 Rust 拒绝（ok:false + 稳定 error 码）。 */
  async place(x: number, y: number): Promise<PlaceResult | null> {
    return this.backend.place(x, y);
  }

  /** 停一手：引擎翻转行棋方并把 Pass 记入历史（undo/adopt 重放与真实序列一致）。 */
  async pass(): Promise<PlaceResult | null> {
    return this.backend.pass();
  }

  /** 撤销最后一手（Rust 侧弹出一手并重放，提子一并还原）。 */
  async undo(): Promise<UndoResult | null> {
    return this.backend.undo();
  }

  /** 同尺寸重开。 */
  async reset(): Promise<void> {
    await this.backend.reset();
  }

  /** 采纳全量快照（SyncState）：见后端实现里的守卫说明。 */
  async adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]): Promise<void> {
    await this.backend.adopt(board, toMove, winner, history);
  }

  /** 当前局面的完整 JSON，交给 AI 分析用。
   *  必须是 Rust 侧序列化的结果而非前端手拼：围棋的劫点/提子数只存在于 `GameState`。
   *  **先等 newGame 落地**（见 `booting` 的说明）。 */
  async stateJson(): Promise<unknown | null> {
    await this.booting;
    return this.backend.stateJson();
  }

  /** 释放对局实例。native 侧必须显式调（实例活在 Rust 的 HashMap 里，不 drop 就
   *  随每次进出页面/切尺寸永久累积）；wasm 侧只是丢引用，Rust 堆内存由胶水的
   *  FinalizationRegistry 在 GC 时回收，无需也不该手动 free。 */
  async dispose(): Promise<void> {
    await this.backend.dispose();
  }
}
