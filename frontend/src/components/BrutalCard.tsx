/**
 * BrutalCard — 新野兽派卡片容器，统一描边与硬阴影。
 * 棋盘、信息面板等均用此包裹，保持视觉一致。
 */
export function BrutalCard({
  children,
  style,
}: {
  children: React.ReactNode;
  style?: React.CSSProperties;
}) {
  return (
    <div className="brutal-card" style={style}>
      {children}
    </div>
  );
}
