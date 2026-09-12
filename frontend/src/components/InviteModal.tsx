/**
 * 对局邀请弹窗（原 App.tsx JSX 原样搬出，禁止行为变化）：服务器挑战与同源挑战统一
 * 形态（req 由 App 计算后传入），必须明确同意/拒绝——不设背景点击关闭，静默忽略
 * 会让挑战方停在「等待对方同意」。
 */
export function InviteModal(props: {
  req: { fromName: string; desc: string } | null;
  onAccept: () => void;
  onReject: () => void;
}) {
  const { req, onAccept, onReject } = props;
  if (!req) return null;
  return (
    <div style={{ position: "fixed", inset: 0, zIndex: 1000, background: "rgba(10,10,10,0.55)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}>
      <div className="brutal-card" style={{ width: "min(440px, 92vw)", background: "#fff", padding: 16, display: "flex", flexDirection: "column", gap: 12 }}>
        <span className="brutal-label">对局邀请</span>
        <div style={{ fontFamily: "var(--font-mono)", fontSize: 14, fontWeight: 800, lineHeight: 1.5 }}>
          {req.fromName} 邀请你加入对局（{req.desc}）
        </div>
        <div style={{ display: "flex", gap: 10 }}>
          <button className="brutal-btn brutal-btn--accent" onClick={onAccept}>同意</button>
          <button className="brutal-btn" onClick={onReject}>拒绝</button>
        </div>
      </div>
    </div>
  );
}
