/**
 * 单行提示（原 App.tsx 各页同款 notice 行抽出，禁止行为变化）：
 * 等宽 12px/700 绿字，text 为空不渲染。copyFb 行是 fontSize 11 的另一款样式，不复用本组件。
 */
export function NoticeLine(props: { text: string | null }) {
  return props.text && <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700, color: "#0a7a2e" }}>{props.text}</div>;
}
