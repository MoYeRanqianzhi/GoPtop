/**
 * URL 展示行（原 App.tsx 等待卡邀请行 / 对局卡观战行同款 JSX 原样抽出，禁止行为变化）：
 * 左侧 code 等宽链接（flex:1 / minWidth:0，窄屏优先挤压），右侧按钮区由 children 传入。
 */
import type { ReactNode } from "react";

export function UrlRow(props: { url: string | null; children: ReactNode }) {
  return (
    <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
      {/* 窄屏时优先挤压 URL 栏（minWidth 0），按钮不换行 */}
      <code style={{ flex: 1, minWidth: 0, border: "3px solid var(--ink)", padding: "7px 10px", fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, background: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{props.url}</code>
      {props.children}
    </div>
  );
}
