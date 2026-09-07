/**
 * state/gameStore — 五子棋判定纯函数。
 *
 * 实况说明（勿被旧注释误导）：App.tsx 当前使用自己的内联 checkFive/emptyBoard，
 * 本文件与其是重复实现。保留此处是因为棋类规则即将下沉到 Rust goptop-core（WASM），
 * 届时本文件与 App 内联副本都将被替换——在替换前，两处语义必须保持一致。
 * 旧版含 tryPlace/GameState 等模拟"完整 store"的代码已删除（从未被引用，且其
 * 围棋落子无提子/禁自杀检查，留着会被误当作可用规则）。
 */

import type { Coord, StoneColor } from "../components/BoardSvg";

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
