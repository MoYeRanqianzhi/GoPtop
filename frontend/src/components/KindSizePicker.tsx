/**
 * 顶部棋种/尺寸选择器（原 App.tsx header 内 JSX 原样搬出，typeOpen 状态随组件内聚，
 * 禁止行为变化）。大屏直接展开（.hp-setup），窄屏收纳进「类型」弹出面板
 * （.hp-type-btn，显隐由 styles/brutal.css 的媒体查询控制）；对局设置组点击任意处
 * 收起弹出面板，弹出面板内部 stopPropagation。对局/等待中锁定（locked）时全部按钮
 * 禁用。children 渲染在选择器之后的同组位置（App 传「菜单」按钮）。
 */
import { useState } from "react";
import type { ReactNode } from "react";
import type { GameKind, Size } from "../net/protocol";

export function KindSizePicker(props: {
  kind: GameKind;
  size: Size;
  onPickKind: (k: GameKind) => void;
  onPickSize: (s: Size) => void;
  locked: boolean;
  lockedTitle: string | undefined;
  children?: ReactNode;
}) {
  const { kind, size, onPickKind, onPickSize, locked, lockedTitle } = props;
  const [typeOpen, setTypeOpen] = useState(false);
  return (
    <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }} onClick={() => setTypeOpen(false)}>
      {/* 对局设置组：大屏直接展开，窄屏收纳进「类型」弹出面板（.hp-setup 隐藏 / .hp-type-btn 显示） */}
      <div className="hp-setup" style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
        <div style={{ display: "flex", gap: 6 }}>
          <button
            className={`brutal-btn brutal-btn--sm ${kind === "gomoku" ? "brutal-btn--active" : ""}`}
            onClick={() => onPickKind("gomoku")}
            aria-pressed={kind === "gomoku"}
            title={lockedTitle}
            disabled={locked}
          >
            五子棋
          </button>
          <button
            className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
            onClick={() => onPickKind("go")}
            aria-pressed={kind === "go"}
            title={lockedTitle}
            disabled={locked}
          >
            围棋
          </button>
        </div>
        <div style={{ width: 1, height: 26, background: "var(--ink)", opacity: 0.18 }} />
        {/* 尺寸表必须与 RulesEngine 的合法性门逐项一致（game/rules.ts 的 15 / 9|13|19，
            Rust 同源门在 goptop-core game.rs 的 is_valid）；下方弹出面板是同一份列表，一起改 */}
        <div style={{ display: "flex", gap: 6 }}>
          {(kind === "gomoku" ? [15] : [9, 13, 19]).map((s) => (
            <button
              key={s}
              className={`brutal-btn brutal-btn--sm ${size === s ? "brutal-btn--active" : ""}`}
              onClick={() => onPickSize(s as Size)}
              aria-pressed={size === s}
              disabled={locked}
              title={lockedTitle ?? undefined}
            >
              {s}×{s}
            </button>
          ))}
        </div>
      </div>
      <div style={{ position: "relative" }} onClick={(e) => e.stopPropagation()}>
        <button className="brutal-btn brutal-btn--sm hp-type-btn" onClick={() => setTypeOpen((o) => !o)} aria-expanded={typeOpen}>
          类型
        </button>
        {typeOpen && (
          <div
            style={{
              position: "absolute", top: "calc(100% + 6px)", right: 0, zIndex: 900,
              background: "#fff", border: "3px solid var(--ink)", boxShadow: "4px 4px 0 var(--ink)",
              padding: 10, display: "flex", flexDirection: "column", gap: 8, minWidth: 200,
            }}
          >
            <div style={{ display: "flex", gap: 6 }}>
              <button
                className={`brutal-btn brutal-btn--sm ${kind === "gomoku" ? "brutal-btn--active" : ""}`}
                onClick={() => { onPickKind("gomoku"); setTypeOpen(false); }}
                aria-pressed={kind === "gomoku"}
                disabled={locked}
                title={lockedTitle}
              >
                五子棋
              </button>
              <button
                className={`brutal-btn brutal-btn--sm ${kind === "go" ? "brutal-btn--active" : ""}`}
                onClick={() => { onPickKind("go"); setTypeOpen(false); }}
                aria-pressed={kind === "go"}
                disabled={locked}
                title={lockedTitle}
              >
                围棋
              </button>
            </div>
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
              {(kind === "gomoku" ? [15] : [9, 13, 19]).map((s) => (
                <button
                  key={s}
                  className={`brutal-btn brutal-btn--sm ${size === s ? "brutal-btn--active" : ""}`}
                  onClick={() => { onPickSize(s as Size); setTypeOpen(false); }}
                  aria-pressed={size === s}
                  disabled={locked}
                  title={lockedTitle ?? undefined}
                >
                  {s}×{s}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
      {props.children}
    </div>
  );
}
