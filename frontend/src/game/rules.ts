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
import { harmonyHost, isTauri } from "../net/links";

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
  /** 请求代次：newGame/dispose 各自推高；在途调用醒来时发现自己的代已过期即整体作废。 */
  private gen = 0;

  async newGame(kindJson: string): Promise<boolean> {
    // 代次在**第一个 await 之前**取号：async 函数体跑到首个 await 为止是同步的，
    // 这样两次 newGame 背靠背（快速连点棋种/尺寸）时后启动者必然拿到更高的代次，
    // 先启动者醒来后凭过期代次把刚建出的局就地 drop——否则两次并发都在对方落地前
    // 把 this.id 摘成 null、各自建局、各自不 drop，先落地的那局从此无句柄，在
    // Rust 的 Games HashMap 里活到进程退出（StrictMode 双挂载必现）。
    // 取号也不能晚到 import 之后：dispose 若落在 import 往返里，迟到的 ++ 会把
    // dispose 的推高顶回去，过期判定就失效了。
    const gen = ++this.gen;
    // 先摘掉旧实例再 await：invoke 失败会抛（command 未注册、参数名不匹配、Rust panic），
    // 若把赋值留在 await 之后，this.id 会继续指着上一局——尺寸已经不同，之后每次
    // place 都拿旧尺寸的棋盘覆盖 UI（wasm 后端此时成了 null，两端行为分叉）。
    const stale = this.id;
    this.id = null;
    const { invoke } = await import("@tauri-apps/api/core");
    const id = await invoke<number | null>("game_new", { kindJson });
    if (gen !== this.gen) {
      // 过期回执：这一局没人认领，必须就地释放；启动时摘下的 stale 也仍由本调用
      // 负责——后启动者在它 await 期间读到的 this.id 已是 null，接不到这个包袱
      if (id !== null) void invoke("game_drop", { id });
      if (stale !== null) void invoke("game_drop", { id: stale });
      return this.id !== null;
    }
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
    // 先推高代次：此刻若有 newGame 在途（StrictMode 卸载时序——cleanup 跑在
    // mount1 的 game_new 落地之前，this.id 已被摘成 null，这里无局可 drop），
    // 它醒来后会凭过期代次自行 drop 刚建出的局
    this.gen++;
    await this.call<void>("game_drop", {});
    this.id = null;
  }
}

/* ---------------- 鸿蒙后端（NAPI 原生宿主） ---------------- */

/**
 * 鸿蒙壳：规则判定直连 Rust（NAPI），与 Tauri 那条一样是**原生**，不是 wasm。
 *
 * 与 [`NativeBackend`] 的差别有两处，都是宿主形态决定的，不是重复代码：
 * - **调用是同步的**：javaScriptProxy 只有同步方法。规则命令是微秒级的纯计算，
 *   包成 Promise 只是白多一次微任务跳转（AI 那条才必须异步，见 ai/client.ts）。
 * - **返回值是一整条 JSON 字符串**：鸿蒙侧的 C ABI 只有 `(cmd, argsJson) → 回执`
 *   这一个口子，没法像 Tauri 那样按命令给不同的返回类型，所以回执统一是 JSON 文本
 *   （与 Tauri 侧 `game_place`/`game_state_json` 的形态一致）。
 */
class HarmonyBackend implements Backend {
  private id: number | null = null;

  /** 桥在页面存活期内不会变，取不到说明壳没带原生模块（回落 wasm）。 */
  private get bridge() {
    return harmonyHost();
  }

  /** 发一条命令，返回解析后的回执；空回执/无此局给 null。 */
  private call<T>(cmd: string, args: Record<string, unknown> = {}): T | null {
    const b = this.bridge;
    if (!b) return null;
    const payload = this.id === null ? args : { id: this.id, ...args };
    const raw = b.call(cmd, JSON.stringify(payload));
    return raw && raw !== "null" ? (JSON.parse(raw) as T) : null;
  }

  async newGame(kindJson: string): Promise<boolean> {
    if (!this.bridge) return false;
    // 与 NativeBackend 同款：先摘旧 id 再取新局，失败时不至于继续用旧局落子
    const stale = this.id;
    this.id = null;
    this.id = this.call<number>("game_new", { kindJson });
    // 旧对局活在 Rust 的 HashMap 里，不显式 drop 就永久留着
    if (stale !== null) this.call<void>("game_drop", { id: stale });
    return this.id !== null;
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
  if (harmonyHost()) return new HarmonyBackend();
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
   *
   * 该窗口的回归覆盖只在 native 侧（rules.host-timing.test.ts 的伪宿主）；wasm 侧
   * 同窗口的形态不同（旧局快照照常返回而非 null），vitest 的 node 环境加载不了
   * wasm 工件，暂无用例钉住——动 WasmBackend.newGame 的赋值时机时只能靠真机自测。
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
