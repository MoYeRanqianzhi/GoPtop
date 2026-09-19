/**
 * WinRateChart —— 胜率走势图（用户拍板 2026-09-19，与红蓝单挑线配套）。
 *
 * 每手一个点，纵轴是「我方胜率」（上=我方优势），横轴是手数。中线是均势参考线。
 *
 * 用 SVG 且 `preserveAspectRatio="none"` 拉伸填满容器：走势图只关心形状，
 * 不需要等比，这样窄容器下也不会留下空白边。线宽用 `vectorEffect` 钉住，
 * 否则非等比拉伸会把线压扁。
 */
export function WinRateChart(props: {
  /** 逐手胜率，按手数有序（手数可能不连续——被取消的分析不补点）。 */
  series: { move: number; winRate: number }[];
  /** 满盘（对手棋子）时的高度，默认 64。 */
  height?: number;
}) {
  const h = props.height ?? 64;
  const n = props.series.length;
  if (n < 2) {
    // 初始态画一条均势直线（用户拍板 2026-09-19），不显示占位文字：文字既多一行，
    // 又让卡片在「有数据」前后跳高度——底部卡的等高是靠内容量稳定撑住的。
    return (
      <svg
        className="wr-chart"
        viewBox={`0 0 100 ${h}`}
        preserveAspectRatio="none"
        style={{ height: h }}
        aria-label="胜率走势（尚未开始）"
      >
        <line x1="0" y1={h / 2} x2="100" y2={h / 2} className="wr-chart-line" vectorEffect="non-scaling-stroke" />
      </svg>
    );
  }
  // 横轴按**手数**定位而非点序号：中间有手数缺失时，曲线不该把缺口抹平
  const first = props.series[0].move;
  const span = Math.max(1, props.series[n - 1].move - first);
  const pts = props.series
    .map((p) => {
      const x = ((p.move - first) / span) * 100;
      const y = (1 - Math.max(0, Math.min(1, p.winRate))) * h;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
  return (
    <svg
      className="wr-chart"
      viewBox={`0 0 100 ${h}`}
      preserveAspectRatio="none"
      style={{ height: h }}
      role="img"
      aria-label={`胜率走势，共 ${n} 手`}
    >
      <line x1="0" y1={h / 2} x2="100" y2={h / 2} className="wr-chart-mid" vectorEffect="non-scaling-stroke" />
      <polyline points={pts} className="wr-chart-line" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}
