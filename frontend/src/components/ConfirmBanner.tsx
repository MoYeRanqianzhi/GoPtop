/**
 * 协商/审批横幅（原 App.tsx JSX 原样搬出，禁止行为变化）：
 * 悔棋、重开、换棋、错钥匙连接、观战发言批准，同意/拒绝由对方拍板。
 */
import type { ConfirmRequest } from "../state/sessionContext";

export function ConfirmBanner(props: { req: ConfirmRequest | null; onApprove: () => void; onDecline: () => void }) {
  const { req, onApprove, onDecline } = props;
  return req ? (
    <div className="brutal-card" style={{ padding: "10px 12px", background: "#fffbeb", display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", zIndex: 50 }}>
      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800 }}>
        {req.kind === "undo" && `${req.fromName} 请求悔棋，是否同意？`}
        {req.kind === "reset" && `${req.fromName} 请求重开对局，是否同意？`}
        {req.kind === "swap" && `${req.fromName} 请求换棋（黑白互换并重开），是否同意？`}
        {req.kind === "wrong-pwd" && `${req.fromName} 请求连接（邀请钥匙不正确），是否接受？`}
        {req.kind === "spec-chat" && `${req.fromName} 申请参与聊天，是否同意？`}
      </span>
      <span style={{ flex: 1 }} />
      <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={onApprove}>同意</button>
      <button className="brutal-btn brutal-btn--sm" onClick={onDecline}>拒绝</button>
    </div>
  ) : null;
}
