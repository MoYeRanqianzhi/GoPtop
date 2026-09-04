/**
 * BrutalCard — 新野兽派卡片容器，统一描边与硬阴影。
 */
export function BrutalCard({
  children,
  style,
  className,
}: {
  children: React.ReactNode;
  style?: React.CSSProperties;
  className?: string;
}) {
  return (
    <div className={`brutal-card ${className ?? ""}`} style={style}>
      {children}
    </div>
  );
}
