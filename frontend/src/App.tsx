import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * App — Phase 0 探针页。
 *
 * - 展示 brutal 风格的标题与按钮，验证 `brutal.css` 生效。
 * - `Greet` 按钮验证 `frontend ↔ src-tauri` 的 `invoke` 链路。
 * - 后续 Phase 2 起替换为路由：`/` 大厅与 `/game/:ticket` 对局。
 */
export default function App() {
  const [greetMsg, setGreetMsg] = useState("");
  const [name, setName] = useState("");

  async function greet() {
    try {
      const msg = await invoke<string>("greet", { name });
      setGreetMsg(msg);
    } catch (e) {
      // 纯 Web 模式（`npm run dev` 不经 Tauri）下 invoke 不可用，给出友好提示而非报错。
      setGreetMsg(`(pure web) invoke unavailable — ${String(e)}`);
    }
  }

  return (
    <main style={{ padding: 32, maxWidth: 960, margin: "0 auto" }}>
      <div className="brutal-card" style={{ padding: 24, marginBottom: 24 }}>
        <h1 className="brutal-title" style={{ fontSize: 40, margin: 0 }}>
          GoPtop
        </h1>
        <p style={{ margin: "8px 0 0", fontWeight: 700 }}>
          P2P 围棋 · 五子棋 — 硬核新野兽派 · 官方 relay 真 P2P（无自建服务器）
        </p>
      </div>

      <div className="brutal-card" style={{ padding: 20, display: "flex", gap: 12, alignItems: "center", flexWrap: "wrap" }}>
        <input
          className="brutal-input"
          placeholder="Enter a name..."
          value={name}
          onChange={(e) => setName(e.currentTarget.value)}
        />
        <button className="brutal-btn brutal-btn--primary" onClick={greet}>
          Greet (invoke)
        </button>
        <button
          className="brutal-btn"
          onClick={() => {
            setName("");
            setGreetMsg("");
          }}
        >
          Clear
        </button>
      </div>

      {greetMsg && (
        <div className="brutal-card" style={{ marginTop: 16, padding: 16, background: "#fff" }}>
          <code style={{ fontFamily: "var(--font-display)", fontWeight: 700 }}>{greetMsg}</code>
        </div>
      )}

      <div style={{ marginTop: 16, fontSize: 13, color: "var(--muted)" }}>
        Phase 0 — 工程基座验证：`cargo check --workspace` · `npm run dev` · `npm run tauri dev`
      </div>
    </main>
  );
}
