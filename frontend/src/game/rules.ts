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
import { isTauri } from "../net/links";

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
    const id = await invoke<number | null>("game_new", { kindJson });
    this.id = id ?? null;
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

/** 规则引擎门面：按运行环境挑后端（Web → wasm，Tauri 系 → 原生）。 */
export class RulesEngine {
  private backend: Backend = isTauri() ? new NativeBackend() : new WasmBackend();

  /** 开新局（kind/size 与前端选择器同源）。Rust 对非法组合是断言即 trap
   *（会掀翻 React 树），故在边界先挡：gomoku 只接受 15，go 只接受 9/13/19。 */
  async newGame(kind: GameKind, size: Size): Promise<void> {
    const valid = kind === "gomoku" ? size === 15 : size === 9 || size === 13 || size === 19;
    if (!valid) return;
    // GameKind serde 外部标签格式：{"Gomoku":{"size":15}} / {"Go":{"size":9}}
    const kindJson = JSON.stringify(kind === "gomoku" ? { Gomoku: { size } } : { Go: { size } });
    await this.backend.newGame(kindJson);
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
   *  必须是 Rust 侧序列化的结果而非前端手拼：围棋的劫点/提子数只存在于 `GameState`。 */
  async stateJson(): Promise<unknown | null> {
    return this.backend.stateJson();
  }

  /** 释放原生侧的对局实例（wasm 后端是空操作）。 */
  async dispose(): Promise<void> {
    await this.backend.dispose();
  }
}
