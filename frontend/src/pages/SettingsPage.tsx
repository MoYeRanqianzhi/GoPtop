/**
 * 设置页 `/settings`（原 App.tsx JSX 原样搬出，禁止行为变化）：
 * 默认规则/尺寸、我的身份、服务器选择、STUN 线路。
 */
import { nav } from "../net/links";
import { storeSet } from "../net/store";
import type { GameSession } from "../state/useGameSession";
import { ServerSettings, StunSettings } from "./components";
import { NoticeLine } from "../components/NoticeLine";

export function SettingsPage(props: { s: GameSession }) {
  const { kind, size, phase, tabUser, myHomeUrl, copyFb, notice, showNotice, copyText } = props.s;
  return (
    <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 10 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
        <span className="brutal-label">设置</span>
        <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回菜单页</button>
      </div>
      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>默认规则与尺寸（新对局/开页时使用）</div>
        <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
          当前：{kind === "gomoku" ? "五子棋" : "围棋"} {size}×{size}
          {phase !== "home" && "（对局/等待中，顶部已锁定）"}
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginTop: 6 }}>
          <button className="brutal-btn brutal-btn--sm" onClick={() => {
            storeSet("goptop:defaults", JSON.stringify({ kind, size }));
            showNotice("已保存当前顶部选择为默认值", 1600);
          }} disabled={phase !== "home"} title={phase !== "home" ? "对局/等待中不可保存" : undefined}>
            保存当前选择为默认
          </button>
        </div>
      </div>
      <div>
        <div className="brutal-label" style={{ marginBottom: 6 }}>我的身份</div>
        <div style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 700, color: "var(--muted)", overflowWrap: "anywhere" }}>{tabUser}</div>
        <div style={{ display: "flex", gap: 8, marginTop: 6, flexWrap: "wrap" }}>
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(tabUser)}`)}>打开我的主页</button>
          <button className="brutal-btn brutal-btn--sm" onClick={() => copyText(myHomeUrl, "主页链接已复制")}>复制我的主页</button>
          {copyFb && <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 800, color: "#0a7a2e" }}>{copyFb}</span>}
        </div>
      </div>
      <ServerSettings />
      <StunSettings />
      <NoticeLine text={notice} />
    </div>
  );
}
