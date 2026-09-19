/**
 * WinRateBar —— 红蓝单挑线（用户拍板 2026-09-19）。
 *
 * 红色＝我方、蓝色＝对手，同一条线上左红右蓝，分界线随胜率实时移动。
 * 与走势图的分工：这条给「此刻」，走势图给「一路怎么走过来的」。
 */

/** 未就绪时的占位。**不能拿 50% 顶上**：那会被读成「均势」，与「还没算出来」是
 *  完全不同的信息，而用户正是靠这条线判断形势的。 */
export function WinRateBar(props: {
  /** 我方胜率 0..1；null 表示尚未分析出结果（按均势渲染）。 */
  winRate: number | null;
  /** 左端标签（我方）。 */
  myLabel: string;
  /** 右端标签（对手）。 */
  oppLabel: string;
  /** 正在分析：数字保持上一次的值，只弱化显示，避免每手闪烁。 */
  thinking?: boolean;
}) {
  // 尚无结果时按 50/50 渲染（用户拍板 2026-09-19）：初始态就该是一条中分的红蓝线。
  // 原先画的是「形势计算中…」占位文字，既多一行、又让卡片在「有结果」前后跳高度。
  const pct = props.winRate === null
    ? 50
    : Math.max(0, Math.min(100, Math.round(props.winRate * 100)));
  const opp = 100 - pct;
  return (
    <div className="wr-bar" data-thinking={props.thinking ? "1" : "0"} data-pending={props.winRate === null ? "1" : "0"}>
      <div className="wr-legend">
        <span className="wr-label wr-label--mine">
          {props.myLabel} <b>{pct}%</b>
        </span>
        <span className="wr-label wr-label--opp">
          <b>{opp}%</b> {props.oppLabel}
        </span>
      </div>
      <div className="wr-track" role="img" aria-label={`${props.myLabel}胜率 ${pct}%`}>
        <div className="wr-fill wr-fill--mine" style={{ width: `${pct}%` }} />
        <div className="wr-fill wr-fill--opp" style={{ width: `${opp}%` }} />
        {/* 分界刻度：胜率贴到 0/100 时这条线就是唯一还能看见的边界 */}
        <div className="wr-edge" style={{ left: `${pct}%` }} />
      </div>
    </div>
  );
}
