/**
 * state/gameStore — 对局状态存储（前端唯一真源的 JS 侧镜像）。
 *
 * 说明：
 * - Rust `goptop-core::GameState` 为权威规则，纯 Web 模式下通过 WASM/`gameStore` 的 JS 镜像复用其语义（本文件为轻量 JS 版，便于无需 WASM 构建即可联调）。
 * - Tauri/未来 WASM 模式可将此 store 的 `tryPlay` 委托给 `goptop-core` WASM 调用，接口保持一致。
 * - 与 `net/transport` 解耦：store 仅负责状态与规则，传输由 `transport` 负责收发 `GameMsg`。
 */

import type { Coord, StoneColor } from "../components/BoardSvg";

export type GameKind = "gomoku" | "go";
export type Size = 9 | 13 | 15 | 19;

const DIRS: [number, number][] = [[1, 0], [0, 1], [1, 1], [1, -1]];

/** 五子棋五连判定（以最后一手为中心，四方向计数）。 */
export function checkFive(board: StoneColor[][], last: Coord, color: StoneColor): boolean {
  if (color === "empty") return false;
  const n = board.length;
  for (const [dx, dy] of DIRS) {
    let count = 1;
    for (const dir of [1, -1]) {
      let x = last.x + dx * dir;
      let y = last.y + dy * dir;
      while (x >= 0 && y >= 0 && x < n && y < n && board[y][x] === color) {
        count++;
        x += dx * dir;
        y += dy * dir;
      }
    }
    if (count >= 5) return true;
  }
  return false;
}

/** 创建空棋盘。 */
export function emptyBoard(size: number): StoneColor[][] {
  return Array.from({ length: size }, () => Array.from({ length: size }, () => "empty" as StoneColor));
}

/** 深拷贝棋盘（用于不可变更新）。 */
export function cloneBoard(board: StoneColor[][]): StoneColor[][] {
  return board.map((row) => [...row]);
}

export type GameState = {
  kind: GameKind;
  size: Size;
  board: StoneColor[][];
  toMove: StoneColor;
  winner: StoneColor | null;
  lastMove: Coord | null;
  history: Coord[];
};

export function createInitialState(kind: GameKind, size: Size): GameState {
  return {
    kind,
    size,
    board: emptyBoard(size),
    toMove: "black",
    winner: null,
    lastMove: null,
    history: [],
  };
}

/** 尝试落子（纯本地规则，不含网络）。返回新状态；不合法则返回原状态与错误。 */
export function tryPlace(state: GameState, c: Coord): { next: GameState; error?: string } {
  if (state.winner) return { next: state, error: "game already over" };
  const n = state.size;
  if (c.x < 0 || c.y < 0 || c.x >= n || c.y >= n) return { next: state, error: "out of bounds" };
  if (state.board[c.y][c.x] !== "empty") return { next: state, error: "point already occupied" };
  const nextBoard = cloneBoard(state.board);
  nextBoard[c.y][c.x] = state.toMove;
  const nextHistory = [...state.history, c];
  if (state.kind === "gomoku" && checkFive(nextBoard, c, state.toMove)) {
    return {
      next: {
        ...state,
        board: nextBoard,
        winner: state.toMove,
        lastMove: c,
        history: nextHistory,
      },
    };
  }
  return {
    next: {
      ...state,
      board: nextBoard,
      toMove: state.toMove === "black" ? "white" : "black",
      lastMove: c,
      history: nextHistory,
    },
  };
}

export function resetState(state: GameState): GameState {
  return createInitialState(state.kind, state.size);
}

/** 序列化/反序列化辅助（与 SyncState 对接）。 */
export function stateToSync(state: GameState) {
  return {
    board: state.board,
    toMove: state.toMove,
    winner: state.winner,
    history: state.history,
    lastMove: state.lastMove,
    kind: state.kind,
    size: state.size,
  };
}

/** 供 React `useState<GameState>` 初始化与单例复用。 */
export const gameStore = {
  emptyBoard,
  checkFive,
  cloneBoard,
  createInitialState,
  tryPlace,
  resetState,
  stateToSync,
};
