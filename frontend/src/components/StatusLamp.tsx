/**
 * 状态指示灯行（原 App.tsx P2P 等待卡/对局卡同款 JSX 原样抽出，禁止行为变化）：
 * 圆点 + 等宽字体状态文本，颜色由调用方传入（linkLamp）。
 */
export function StatusLamp(props: { color: string; text: string }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
      <span style={{ width: 11, height: 11, borderRadius: 999, background: props.color, border: "2px solid var(--ink)", flexShrink: 0, display: "inline-block" }} />
      <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 800, color: props.color }}>{props.text}</span>
    </div>
  );
}
