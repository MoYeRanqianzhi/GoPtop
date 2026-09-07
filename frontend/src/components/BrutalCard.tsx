/**
 * BrutalCard — 新野兽派卡片容器，统一描边与硬阴影。
 * 注：App.tsx 目前用内联 div（样式多态更多），本组件暂无调用者，保留作为
 * 统一容器的入口；若持续零引用应在后续清理轮删除。
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
