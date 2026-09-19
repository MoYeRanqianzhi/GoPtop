/**
 * WinRateChart —— 胜率走势图（用户拍板 2026-09-19，与红蓝单挑线配套）。
 *
 * 纵轴是「我方胜率」（上=我方优势），横轴是手数。中线是均势参考线。
 *
 * **滑动时间窗口**（用户拍板 2026-09-19）：只画最近若干手，窗口大小按组件实际
 * 宽度算（`pxPerMove` 像素一手）。原先画全部手数，对局一长点就挤成一片黑。
 *
 * 横轴按**手数差**定位而不是按点序号缩放：每手恒占 `100/windowSize` 个单位，
 * 于是窗口推进时整条曲线平滑左移，而不是每次重算范围导致的横向跳动。
 *
 * 用 SVG 且 `preserveAspectRatio="none"` 拉伸填满容器：走势图只关心形状，
 * 不需要等比，这样窄容器下也不会留下空白边。线宽用 `vectorEffect` 钉住，
 * 否则非等比拉伸会把线压扁。
 */
import { useEffect, useRef, useState } from "react";

export function WinRateChart(props: {
  /** 逐手胜率，按手数有序（手数可能不连续——被取消的分析不补点）。 */
  series: { move: number; winRate: number }[];
  /** 满盘（对手棋子）时的高度，默认 64。 */
  height?: number;
  /** 每手占的横向像素，决定窗口放得下多少手。默认 6（约 40 手/250px）。 */
  pxPerMove?: number;
}) {
  const h = props.height ?? 64;
  const pxPerMove = props.pxPerMove ?? 6;
  const boxRef = useRef<HTMLDivElement>(null);
  const [boxW, setBoxW] = useState(0);

  // 窗口大小取决于组件宽度，而宽度由布局（容器查询 / 卡片）决定，只能实测。
  // boxW 为 0 的首次渲染走默认窗口，ResizeObserver 立刻会给出真实值。
  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const sync = () => setBoxW(el.clientWidth);
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const windowSize = boxW > 0 ? Math.max(6, Math.floor(boxW / pxPerMove)) : 40;
  const all = props.series;
  const view = all.length > windowSize ? all.slice(-windowSize) : all;
  const n = view.length;

  let body = null;
  if (n >= 2) {
    const latest = view[n - 1].move;
    // 窗口左端的手数；对局还没铺满窗口时贴 0，曲线自左向右生长
    const head = Math.max(0, latest - windowSize);
    const pts = view
      .map((p) => {
        const x = ((p.move - head) / windowSize) * 100;
        const y = (1 - Math.max(0, Math.min(1, p.winRate))) * h;
        return `${x.toFixed(2)},${y.toFixed(2)}`;
      })
      .join(" ");
    body = (
      <>
        <line x1="0" y1={h / 2} x2="100" y2={h / 2} className="wr-chart-mid" vectorEffect="non-scaling-stroke" />
        <polyline points={pts} className="wr-chart-line" vectorEffect="non-scaling-stroke" />
      </>
    );
  }
  // 不足两手时**不放任何线**（用户拍板 2026-09-19）：滑动窗口下每手恒占
  // 100/windowSize 的宽度，先画一条横贯全宽的直线、下一手再缩成一小段，
  // 视觉上是「突然塌掉」——不如一开始就空着，只留虚线框在那里等着。

  return (
    <div ref={boxRef} className="wr-chart-box">
      <svg
        className="wr-chart"
        viewBox={`0 0 100 ${h}`}
        preserveAspectRatio="none"
        style={{ height: h }}
        role="img"
        aria-label={`胜率走势（最近 ${n} 手，共 ${all.length} 手）`}
      >
        {body}
      </svg>
    </div>
  );
}
