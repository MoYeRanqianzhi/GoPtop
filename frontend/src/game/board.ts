/**
 * 棋盘基础规则（前端内存版）——`checkFive` 与 `emptyBoard` 的唯一实现。
 *
 * 现行生效路径：App.tsx（P2P 对局消息处理）与 pages/LocalPage（本地对战）。
 * 历史教训：checkFive 曾同时存在三份（App 内联、state/gameStore、game/ 目录），
 * 且 gameStore 那份对围棋规则残缺——重复实现被静默漂移过。改规则前先确认
 * 本文件与 Rust 侧 crates/goptop-core 的语义一致（未来规则下沉 WASM 的对齐点）。
 */
import type { Coord, StoneColor } from "../components/BoardSvg";

/** 四连珠方向（右、下、右下、左下），配合 ±dir 双向扫描。 */
export const DIRS: [number, number][] = [[1, 0], [0, 1], [1, 1], [1, -1]];

/** 以 `last` 为中心四方向连珠判定，≥5 连即胜（仅五子棋调用）。 */
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

/** 生成 size×size 全空棋盘。 */
export function emptyBoard(size: number): StoneColor[][] {
  return Array.from({ length: size }, () => Array.from({ length: size }, () => "empty" as StoneColor));
}
