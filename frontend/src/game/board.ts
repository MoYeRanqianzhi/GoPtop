/**
 * 棋盘 TS 侧工具 —— 只剩空盘生成；落子规则唯一实现在 Rust core
 * （经 game/rules.ts 的 wasm 绑定执行：五连、围棋提子/禁自杀）。
 *
 * 历史教训：checkFive 曾同时存在三份（App 内联、state/gameStore、game/ 目录）
 * 且语义漂移，围棋规则 TS 版更是从未实现过提子。任何落子判定禁止在 TS
 * 再建副本——需要新规则就改 crates/goptop-core 并重跑 scripts/build-wasm.sh。
 */
import type { StoneColor } from "../net/protocol";

/** 生成 size×size 全空棋盘（渲染层占位）。 */
export function emptyBoard(size: number): StoneColor[][] {
  return Array.from({ length: size }, () => Array.from({ length: size }, () => "empty" as StoneColor));
}
