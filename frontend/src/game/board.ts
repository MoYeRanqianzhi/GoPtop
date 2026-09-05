/**
 * game/board — 棋盘类型与工具（与 `goptop-core` 对齐）。
 *
 * 前端权威规则在 `state/gameStore.ts` 与 Rust `goptop-core`；此模块仅暴露类型与空板工厂
 * 供非 React 场景复用。
 */
import type { StoneColor } from "../components/BoardSvg";

export type BoardCoord = { x: number; y: number };

export function emptyBoard(size: number): StoneColor[][] {
  return Array.from({ length: size }, () => Array.from({ length: size }, () => "empty" as StoneColor));
}

export function cloneBoard(board: StoneColor[][]): StoneColor[][] {
  return board.map((row) => [...row]);
}
