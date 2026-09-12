/**
 * 在线用户页 `/users`（原 App.tsx JSX 原样搬出，禁止行为变化）：名册 + 挑战入口。
 */
import { nav } from "../net/links";
import type { GameSession } from "../state/useGameSession";
import { PeerList } from "./components";
import { NoticeLine } from "../components/NoticeLine";

export function UsersPage(props: { s: GameSession }) {
  const { peers, phase, kind, size, notice, serverMode, serverChallengePeer, acceptInvite } = props.s;
  return (
    <div className="brutal-card" style={{ padding: "10px 12px", background: "#fff", display: "flex", flexDirection: "column", gap: 8 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, flexWrap: "wrap" }}>
        <span className="brutal-label">在线用户（{peers.length}）</span>
        <button className="brutal-btn brutal-btn--sm" onClick={() => nav("/")}>回菜单页</button>
      </div>
      {phase !== "home" && (
        <div style={{ fontFamily: "var(--font-mono)", fontSize: 12, fontWeight: 700 }}>
          你当前{phase === "waiting" ? "正在等待对手" : "正在对局中"}。
          <button className="brutal-btn brutal-btn--sm" style={{ marginLeft: 8 }} onClick={() => nav("/p2p")}>前往 P2P 页</button>
        </div>
      )}
      <PeerList
        peers={peers}
        emptyHint="暂无其他在线用户。"
        actionLabel={() => "挑战"}
        onAction={(p) => {
          // 服务器模式必须走服务器信令（跨设备可达）；acceptInvite 的 presence
          // 挑战只在同源 BroadcastChannel 有效，曾导致服务器模式下挑战发不出去。
          // serverChallengePeer 自带 phase 守卫（非主页发起时提示而非静默无效）
          if (serverMode) serverChallengePeer(p.id);
          else if (phase === "home") acceptInvite(p.id, null, kind, size);
        }}
        extraAction={(p) => (
          <button className="brutal-btn brutal-btn--sm" onClick={() => nav(`/${encodeURIComponent(p.id)}`)}>主页</button>
        )}
      />
      <NoticeLine text={notice} />
    </div>
  );
}
