/**
 * 信令弹窗（原 App.tsx JSX 原样搬出，禁止行为变化）：信令消息统一经弹窗收发——
 * 粘贴邀请（paste-invite）/ 输入回执（paste-answer）/ 展示回执（receipt）。
 * 背景点击关闭；输入框自动聚焦，Enter 提交。
 */
type ModalKind = "paste-invite" | "paste-answer" | "receipt";

export function PasteModal(props: {
  modal: ModalKind | null;
  modalInput: string;
  modalErr: string | null;
  setModal: (m: ModalKind | null) => void;
  setModalInput: (v: string) => void;
  setModalErr: (v: string | null) => void;
  submitModal: () => void;
  answerBackUrl: string | null;
  copyFb: string | null;
  copyText: (t: string, okMsg: string) => void;
}) {
  const { modal, modalInput, modalErr, setModal, setModalInput, submitModal, answerBackUrl, copyFb, copyText } = props;
  return modal ? (
    <div
      style={{ position: "fixed", inset: 0, zIndex: 1000, background: "rgba(10,10,10,0.55)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}
      onClick={() => setModal(null)}
    >
      <div
        className="brutal-card"
        style={{ width: "min(560px, 92vw)", maxHeight: "80vh", overflow: "auto", background: "#fff", padding: 16, display: "flex", flexDirection: "column", gap: 10 }}
        onClick={(e) => e.stopPropagation()}
      >
        {modal === "receipt" ? (
          <>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
              <span className="brutal-label">回执已生成 · 发给邀请者</span>
              <button className="brutal-btn brutal-btn--sm" onClick={() => setModal(null)}>关闭</button>
            </div>
            <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 600, lineHeight: 1.5 }}>
              把下面的回执链接发给邀请者即可开局。
            </div>
            <textarea
              value={answerBackUrl ?? ""}
              readOnly
              rows={3}
              onFocus={(e) => e.currentTarget.select()}
              style={{ border: "3px solid var(--ink)", padding: "8px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fffbeb", overflowWrap: "anywhere", resize: "vertical" }}
            />
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
              <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={() => answerBackUrl && copyText(answerBackUrl, "回执链接已复制")}>复制回执链接</button>
              {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
            </div>
          </>
        ) : (
          <>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
              <span className="brutal-label">
                {modal === "paste-invite" ? "粘贴邀请链接（对方发来的）"
                  : "输入回执（受邀者发来的回执链接）"}
              </span>
              <button className="brutal-btn brutal-btn--sm" onClick={() => setModal(null)}>关闭</button>
            </div>
            <input
              autoFocus
              value={modalInput}
              onChange={(e) => setModalInput(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) submitModal(); }}
              placeholder={modal === "paste-invite" ? "粘贴邀请链接或主页链接"
                : "粘贴受邀者发来的回执链接"}
              style={{ border: "3px solid var(--ink)", padding: "9px 10px", fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, background: "#fff" }}
            />
            {modalErr && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#b00020" }}>{modalErr}</div>}
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
              <button className="brutal-btn brutal-btn--sm brutal-btn--accent" onClick={submitModal}>
                {modal === "paste-answer" ? "确认回执" : "连接"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  ) : null;
}
