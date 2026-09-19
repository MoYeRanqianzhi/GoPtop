/**
 * 菜单页 `/`（原 App.tsx JSX 原样搬出，禁止行为变化）：五种玩法入口 + 主页链接 + 通知行。
 */
import { nav } from "../net/links";
import type { GameSession } from "../state/useGameSession";
import { NoticeLine } from "../components/NoticeLine";

export function MenuPage(props: { s: GameSession }) {
  const { tabUser, peers, myHomeUrl, copyFb, notice, copyText } = props.s;
  return (
    <div className="brutal-card" style={{ padding: "16px 14px", background: "#fff", display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
        <span className="brutal-label">开始 · 选一个玩法</span>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>{tabUser}</span>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/local")}>本地对战</button>
        <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/ai")}>人机对战</button>
        <button className="brutal-btn brutal-btn--accent" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/p2p")}>P2P 对战</button>
        <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/users")}>在线用户（{peers.length}）</button>
        <button className="brutal-btn" style={{ padding: "16px 8px", width: "100%" }} onClick={() => nav("/settings")}>设置</button>
      </div>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
        <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(tabUser)}`)}>我的主页</button>
        <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
        {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
      </div>
      <NoticeLine text={notice} />
    </div>
  );
}
