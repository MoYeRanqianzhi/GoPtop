/**
 * rules —— Rust 规则引擎的前端门面（goptop-core 的 wasm 绑定）。
 *
 * 架构地位（用户拍板：Rust 承接一切核心功能，TS 只做 UI）：五连判定、围棋
 * 提子/禁自杀的唯一实现是 crates/goptop-core，经 wasm 在 Web 与 Tauri WebView
 * 中执行。TS 侧不再有规则副本——checkFive 曾同时存在三份且语义漂移，TS 版
 * 围棋更完全没有提子；本轮起所有落子判定都过这里。
 *
 * 数据面：wasm.rs 的 JSON 约定与 net/protocol.ts 同构（小写颜色字符串、
 * 行优先二维数组、逻辑尺寸棋盘），board 可直接 setBoard。
 * 产物：frontend/src/wasm/（scripts/build-wasm.sh 生成并提交——前端构建
 * 不需要 Rust 工具链，改 core 后才重跑脚本）。
 */
import init, { WasmGame } from "../wasm/goptop_core";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";

/** wasm 规则判定结果：ok 时 board 为权威棋盘（围棋含提子效果）。 */
export type PlaceResult =
  | { ok: true; board: StoneColor[][]; captured: Coord[]; toMove: "black" | "white"; winner: StoneColor | null }
  | { ok: false; error: string };

/** undo_last 的返回（无 captured 字段，提子已还原进 board）。 */
export type UndoResult =
  | { ok: true; board: StoneColor[][]; toMove: "black" | "white"; winner: StoneColor | null }
  | { ok: false; error: string };

// 模块级一次初始化：顶层 await 让所有 import 方拿到引擎前 wasm 必已就绪，
// 调用方无需到处判 await（vite/WebView2 均支持 TLA）。
await init();

export class RulesEngine {
  private game: WasmGame | null = null;

  /** 开新局（kind/size 与前端选择器同源）。Rust 对非法组合是断言即 wasm
   *  trap（会掀翻 React 树），故在边界先挡：gomoku 只接受 15，go 只接受
   *  9/13/19——不合法直接不建引擎（调用方的再次 newGame 会带着正确 size 来）。 */
  newGame(kind: GameKind, size: Size) {
    const valid = kind === "gomoku" ? size === 15 : size === 9 || size === 13 || size === 19;
    if (!valid) return;
    // GameKind serde 外部标签格式：{"Gomoku":{"size":15}} / {"Go":{"size":9}}
    const kindJson = JSON.stringify(kind === "gomoku" ? { Gomoku: { size } } : { Go: { size } });
    // 胶水把 Option<WasmGame> 映射为 undefined：null 与之运行时等价，归一存 null
    this.game = WasmGame.new_game(kindJson) ?? null;
  }

  /** 落子判定：占据/越界/自杀/终局由 Rust 拒绝（ok:false + 稳定 error 码）。 */
  place(x: number, y: number): PlaceResult | null {
    if (!this.game) return null;
    return JSON.parse(this.game.try_place(x, y)) as PlaceResult;
  }

  /** 停一手：引擎翻转行棋方并把 Pass 记入历史（undo/adopt 重放与真实序列一致）。 */
  pass(): PlaceResult | null {
    if (!this.game) return null;
    return JSON.parse(this.game.pass()) as PlaceResult;
  }

  /** 撤销最后一手（Rust 侧弹出一手并重放，提子一并还原）。 */
  undo(): UndoResult | null {
    if (!this.game) return null;
    return JSON.parse(this.game.undo_last()) as UndoResult;
  }

  /** 采纳全量快照（SyncState）：棋盘/行棋方/胜者照收，历史按坐标/"pass"
   *  重建供后续 undo。Rust 侧对非法值（维度/颜色/越界坐标）返回 false；
   *  快照是远端输入（系统边界），拒绝时必须留痕，否则引擎与 TS 静默分叉
   *  （审查 #5 P2-1）。 */
  adopt(board: StoneColor[][], toMove: StoneColor, winner: StoneColor | null, history: (Coord | "pass")[]) {
    if (!this.game) return;
    const ok = this.game.adopt(JSON.stringify(board), toMove, winner ?? "null", JSON.stringify(history));
    if (!ok) console.warn("[rules] adopt 被引擎拒绝：快照与当前对局尺寸/格式不符，已忽略该快照");
  }

  /** 同尺寸重开。 */
  reset() {
    this.game?.reset();
  }
}
