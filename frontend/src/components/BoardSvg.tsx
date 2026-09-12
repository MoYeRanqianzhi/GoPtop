/**
 * BoardSvg — 棋盘 SVG 组件（Gomoku 15 / Go 9/13/19 通用）
 * 纸面暖黄、粗黑网格、坐标外移防重合、星位实心、棋子纯色无边框无阴影、悬停橙色高亮。
 *
 * StoneColor/Coord 的唯一真源在 net/protocol（线格式同构，统一防漂移）；
 * 此处 re-export 兼容既有下游 import。
 */
import type { Coord, StoneColor } from "../net/protocol";

export type { Coord, StoneColor } from "../net/protocol";

function goStarPoints(size: number): Coord[] {
  if (size === 19) return [
    { x: 3, y: 3 }, { x: 9, y: 3 }, { x: 15, y: 3 },
    { x: 3, y: 9 }, { x: 9, y: 9 }, { x: 15, y: 9 },
    { x: 3, y: 15 }, { x: 9, y: 15 }, { x: 15, y: 15 },
  ];
  if (size === 13) return [
    { x: 3, y: 3 }, { x: 9, y: 3 },
    { x: 3, y: 9 }, { x: 9, y: 9 },
    { x: 6, y: 6 },
  ];
  if (size === 9) return [
    { x: 2, y: 2 }, { x: 6, y: 2 },
    { x: 2, y: 6 }, { x: 6, y: 6 },
    { x: 4, y: 4 },
  ];
  return [];
}

function gomokuStarPoints(): Coord[] {
  // 15 路标准五星，四角 + 天元
  return [
    { x: 3, y: 3 }, { x: 11, y: 3 },
    { x: 7, y: 7 },
    { x: 3, y: 11 }, { x: 11, y: 11 },
  ];
}

export function BoardSvg({
  size,
  board,
  onPlace,
  lastMove,
  hover,
  onHover,
  disabled,
  kind,
}: {
  size: number;
  board: StoneColor[][];
  onPlace: (c: Coord) => void;
  lastMove: Coord | null;
  hover: Coord | null;
  onHover: (c: Coord | null) => void;
  disabled?: boolean;
  kind: "gomoku" | "go";
}) {
  const padding = 30;
  const cell = 36;
  const extent = (size - 1) * cell;
  const vb = padding * 2 + extent;
  const stars = kind === "go" ? goStarPoints(size) : gomokuStarPoints();

  function coordFromEvent(e: React.PointerEvent<SVGSVGElement> | React.MouseEvent<SVGSVGElement>): Coord | null {
    const rect = e.currentTarget.getBoundingClientRect();
    const sx = vb / rect.width;
    const sy = vb / rect.height;
    const x = (e.clientX - rect.left) * sx - padding;
    const y = (e.clientY - rect.top) * sy - padding;
    const gx = Math.round(x / cell);
    const gy = Math.round(y / cell);
    if (gx < 0 || gy < 0 || gx >= size || gy >= size) return null;
    const dx = Math.abs(x - gx * cell);
    const dy = Math.abs(y - gy * cell);
    if (dx > cell * 0.45 || dy > cell * 0.45) return null;
    return { x: gx, y: gy };
  }

  const colLabels = Array.from({ length: size }, (_, i) => {
    const letters = "ABCDEFGHJKLMNOPQRST";
    return letters[i] ?? String(i + 1);
  });

  return (
    <div className="brutal-card board-paper" style={{ overflow: "hidden" }}>
      <svg
        viewBox={`0 0 ${vb} ${vb}`}
        width="100%"
        style={{ display: "block", userSelect: "none", touchAction: "manipulation", width: "100%", height: "100%" }}
        onPointerMove={(e) => {
          if (disabled) return;
          const c = coordFromEvent(e);
          if (!c) { onHover(null); return; }
          if (board[c.y]?.[c.x] !== "empty") { onHover(null); return; }
          onHover(c);
        }}
        onPointerLeave={() => onHover(null)}
        onClick={(e) => {
          if (disabled) return;
          const c = coordFromEvent(e);
          if (!c) return;
          if (board[c.y]?.[c.x] !== "empty") return;
          onPlace(c);
        }}
        role="grid"
        aria-label={`${kind} board ${size}x${size}`}
      >
        {/* 外框 */}
        <rect x={padding - 10} y={padding - 10} width={extent + 20} height={extent + 20} fill="none" stroke="#0A0A0A" strokeWidth={3} />
        {/* 网格 — 粗黑 */}
        {Array.from({ length: size }, (_, i) => {
          const p = padding + i * cell;
          return (
            <g key={`grid-${i}`} stroke="#0A0A0A" strokeWidth={i === 0 || i === size - 1 ? 2.2 : 1.6} shapeRendering="crispEdges">
              <line x1={padding} y1={p} x2={padding + extent} y2={p} />
              <line x1={p} y1={padding} x2={p} y2={padding + extent} />
            </g>
          );
        })}
        {/* 坐标 — 14/24 外移，避免与边线重合；左右数字各外移 22 */}
        {colLabels.map((ch, i) => (
          <g key={`label-${i}`}>
            <text x={padding + i * cell} y={padding - 14} textAnchor="middle" fontFamily="var(--font-mono)" fontWeight={700} fontSize={11} fill="#0A0A0A">{ch}</text>
            <text x={padding + i * cell} y={padding + extent + 24} textAnchor="middle" fontFamily="var(--font-mono)" fontWeight={700} fontSize={11} fill="#0A0A0A">{ch}</text>
            <text x={padding - 24} y={padding + i * cell + 4} textAnchor="middle" fontFamily="var(--font-mono)" fontWeight={700} fontSize={11} fill="#0A0A0A">{size - i}</text>
            <text x={padding + extent + 24} y={padding + i * cell + 4} textAnchor="middle" fontFamily="var(--font-mono)" fontWeight={700} fontSize={11} fill="#0A0A0A">{size - i}</text>
          </g>
        ))}
        {/* 星位 */}
        {stars.map((s) => (
          <circle key={`star-${s.x}-${s.y}`} cx={padding + s.x * cell} cy={padding + s.y * cell} r={5.5} fill="#0A0A0A" />
        ))}
        {/* 悬停高亮 — 橙色 */}
        {hover && (
          <g opacity={0.95}>
            <circle cx={padding + hover.x * cell} cy={padding + hover.y * cell} r={16} fill="none" stroke="#FF8C1A" strokeWidth={2} strokeDasharray="4 4" />
            <circle cx={padding + hover.x * cell} cy={padding + hover.y * cell} r={10} fill="#FF8C1A" opacity={0.28} />
          </g>
        )}
        {/* 棋子 — 纯色无边框无阴影，橙色标记末手 */}
        {board.map((row, y) =>
          row.map((s, x) => {
            if (s === "empty") return null;
            const cx = padding + x * cell;
            const cy = padding + y * cell;
            const isLast = lastMove?.x === x && lastMove?.y === y;
            return (
              <g key={`stone-${x}-${y}`}>
                <circle cx={cx} cy={cy} r={15} fill={s === "black" ? "#0A0A0A" : "#FFFFFF"} />
                {isLast && <circle cx={cx} cy={cy} r={6.5} fill="none" stroke="#FF8C1A" strokeWidth={2.5} />}
              </g>
            );
          }),
        )}
      </svg>
    </div>
  );
}
