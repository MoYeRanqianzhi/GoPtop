/**
 * 人机对战页 `/ai` —— 本地离线 AI 对手（用户拍板 2026-09-19 独立成页）。
 *
 * 与本地对战页的关键差别：落子不再是"谁点谁下"，而是轮到 AI 时由后台引擎决定。
 * 但**AI 的着法仍然走规则引擎**（`RulesEngine.place`）——AI 只提供坐标，合法性、
 * 提子、胜负判定一律由 Rust 规则真源裁决。这样 AI 与真人走的是同一条路径，
 * 不存在"AI 能下出真人下不了的棋"这类分叉。
 *
 * 搜索在 Web Worker 里跑（见 ai/client），主线程只等一个 Promise，界面全程可交互。
 */
import { useEffect, useRef, useState } from "react";
import type { Coord, GameKind, Size, StoneColor } from "../net/protocol";
import { emptyBoard } from "../game/board";
import { RulesEngine } from "../game/rules";
import { Settings } from "lucide-react";
import { sharedAiClient, useWinRate } from "../ai/useWinRate";
import { BoardPanel } from "./components";

/** 难度档位 → 每步思考预算（毫秒）。实测五子棋 1000ms 到 depth 7；围棋 1000ms 约 3 万 playout。 */
const LEVELS = [
  { key: "fast", label: "快", budgetMs: 300 },
  { key: "normal", label: "标准", budgetMs: 1000 },
  { key: "strong", label: "强", budgetMs: 3000 },
] as const;

/** 胜率刷新用的预算：比选点短，避免每手都要等两次长搜索。 */
const ODDS_BUDGET_MS = 500;

const aiColorName = (c: StoneColor) => (c === "black" ? "Black" : "White");
const other = (c: StoneColor): StoneColor => (c === "black" ? "white" : "black");

export function AiPage(props: { kind: GameKind; size: Size }) {
  const { kind, size } = props;
  const rulesRef = useRef(new RulesEngine());
  const [board, setBoard] = useState<StoneColor[][]>(() => emptyBoard(size));
  const [toMove, setToMove] = useState<StoneColor>("black");
  const [winner, setWinner] = useState<StoneColor | null>(null);
  const [lastMove, setLastMove] = useState<Coord | null>(null);
  const [hover, setHover] = useState<Coord | null>(null);
  const [history, setHistory] = useState<Coord[]>([]);
  /** AI 执哪一方；默认白（人类执黑先行）。 */
  const [aiColor, setAiColor] = useState<StoneColor>("white");
  const [levelKey, setLevelKey] = useState<(typeof LEVELS)[number]["key"]>("normal");
  const [thinking, setThinking] = useState(false);
  const [aiError, setAiError] = useState<string | null>(null);
  /** 设置面板开合（用户拍板 2026-09-19）：难度/换边/悔棋/重开全收进面板，
   *  状态行只留一个入口——四五个按钮平铺在状态行上会把棋盘挤窄。 */
  const [panelOpen, setPanelOpen] = useState(false);

  const humanColor = other(aiColor);
  const budgetMs = LEVELS.find((l) => l.key === levelKey)?.budgetMs ?? 1000;

  // 顶部切换规则/尺寸时重建引擎并重置（与本地页同款）
  useEffect(() => {
    rulesRef.current.newGame(kind, size);
    setBoard(emptyBoard(size));
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
    setAiError(null);
  }, [kind, size]);

  /** 重置到"轮到人类"的初始局面。 */
  function resetBoard(nextAi: StoneColor = aiColor) {
    rulesRef.current.reset();
    setBoard(emptyBoard(size));
    // 人类执黑时黑先；AI 执黑时黑先但那一步该 AI 走，effect 会自动接手
    setToMove("black");
    setWinner(null);
    setLastMove(null);
    setHistory([]);
    setHover(null);
    setAiError(null);
    setAiColor(nextAi);
  }

  function place(c: Coord) {
    if (winner || thinking) return;
    if (toMove !== humanColor) return; // 轮到 AI 时人类点不动棋盘
    if (board[c.y][c.x] !== "empty") return;
    const res = rulesRef.current.place(c.x, c.y);
    if (!res?.ok) return;
    setBoard(res.board);
    setLastMove(c);
    setHistory((h) => [...h, c]);
    if (res.winner) setWinner(res.winner);
    else setToMove(res.toMove);
  }

  /** 撤销一个回合：撤到重新轮到人类为止（否则 AI 会立刻把同一手补回来，悔棋等于没悔）。 */
  function undo() {
    if (thinking) return;
    let res = rulesRef.current.undo();
    if (!res?.ok) return;
    let h = history.slice(0, -1);
    if (res.toMove === aiColor && h.length > 0) {
      const res2 = rulesRef.current.undo();
      if (res2?.ok) {
        res = res2;
        h = h.slice(0, -1);
      }
    }
    setBoard(res.board);
    setHistory(h);
    setToMove(res.toMove);
    setWinner(res.winner);
    setLastMove(h.length > 0 ? h[h.length - 1] : null);
    setAiError(null);
  }

  // AI 走子：轮到 AI 且未终局时触发一次搜索
  useEffect(() => {
    if (winner || toMove !== aiColor) return;
    const state = rulesRef.current.stateJson();
    if (!state) return;
    let cancelled = false;
    setThinking(true);
    sharedAiClient()
      .analyze({ state, myColor: aiColorName(aiColor), budgetMs, wantMove: true })
      .then((r) => {
        if (cancelled) return;
        if (!r.bestMove) {
          // 无着可下（引擎认为终局）——不静默卡住，交给用户重开
          setAiError("AI 判定无处可下，请重开一局");
          return;
        }
        const [x, y] = r.bestMove;
        const res = rulesRef.current.place(x, y);
        if (!res?.ok) {
          // 引擎侧已有合法性兜底，走到这里说明兜底也失效了——必须留痕，
          // 否则表现为"AI 不动了"，看日志才知道原因
          setAiError(`AI 给出非法着法 (${x},${y})：${res?.error ?? "未知"}`);
          return;
        }
        setBoard(res.board);
        setLastMove({ x, y });
        setHistory((h) => [...h, { x, y }]);
        if (res.winner) setWinner(res.winner);
        else setToMove(res.toMove);
        setAiError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        console.error("[ai] 分析失败", e);
        setAiError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setThinking(false);
      });
    return () => {
      cancelled = true;
    };
    // aiColor/budgetMs 变化要重算当前局面（换边后可能立刻轮到新 AI）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toMove, aiColor, winner, budgetMs, size]);

  const odds = useWinRate({
    getState: () => rulesRef.current.stateJson(),
    myColor: aiColorName(humanColor),
    moveCount: history.length,
    budgetMs: ODDS_BUDGET_MS,
    enabled: !winner && !aiError,
  });

  const statusText = winner
    ? `${winner === "black" ? "黑" : "白"} 胜`
    : thinking
      ? "AI 思考中…"
      : `${toMove === "black" ? "黑" : "白"} 落子`;

  return (
    <div className="play-stack">
      <BoardPanel
        kind={kind} size={size} board={board} toMove={toMove} winner={winner}
        lastMove={lastMove} hover={hover} onHover={setHover}
        disabled={!!winner || thinking || toMove !== humanColor} onPlace={place}
        statusText={statusText}
        statusNote={
          /* wasm trap 的原文（"unreachable"）对用户没有意义，界面上给一句人话，
             真正的原因（引擎 panic）留给控制台 */
          aiError ? "AI 引擎不可用（详见控制台）" : thinking ? "计算中" : `你执${humanColor === "black" ? "黑" : "白"}`
        }
        moveCount={history.length}
        /* 悔棋/重开已移入设置面板（用户拍板：状态行只留一个设置入口，避免挤） */
        onUndo={null}
        onReset={null}
        odds={{
          winRate: odds.winRate,
          series: odds.series,
          thinking: odds.thinking,
          myLabel: "我",
          oppLabel: "AI",
        }}
        chatButton={
          <button
            className="brutal-btn brutal-btn--sm"
            onClick={() => setPanelOpen((v) => !v)}
            title="对局设置（难度 / 先后手 / 悔棋 / 重开）"
            aria-label="对局设置"
          >
            <Settings size={15} strokeWidth={2.5} style={{ display: "block" }} />
          </button>
        }
      />

      {/* 设置面板：复用聊天弹窗的遮罩与卡片样式（fixed 定位，不占 play-stack 布局）。
          难点/先后手改完即生效；换边与重开会重置棋盘，所以顺手收起面板。 */}
      {panelOpen && (
        <div className="chat-modal-bg" onClick={() => setPanelOpen(false)}>
          <div
            className="brutal-card chat-modal"
            onClick={(e) => e.stopPropagation()}
            /* .chat-modal 的固定高度是给聊天记录列表用的；设置面板内容少，
               照搬会撑出一大片空白，这里按内容自适应 */
            style={{ height: "auto", maxHeight: "88dvh", padding: 14, display: "flex", flexDirection: "column", gap: 14 }}
          >
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
              <div className="brutal-label">对局设置</div>
              <button className="brutal-btn brutal-btn--sm" onClick={() => setPanelOpen(false)}>收起</button>
            </div>

            <div>
              <div className="brutal-label" style={{ marginBottom: 6 }}>难度 · 每步思考时长</div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                {LEVELS.map((l) => (
                  <button
                    key={l.key}
                    className={`brutal-btn brutal-btn--sm${l.key === levelKey ? " brutal-btn--primary" : ""}`}
                    onClick={() => setLevelKey(l.key)}
                    title={`每步思考 ${l.budgetMs} 毫秒`}
                  >
                    {l.label} {l.budgetMs / 1000}s
                  </button>
                ))}
              </div>
            </div>

            <div>
              <div className="brutal-label" style={{ marginBottom: 6 }}>先后手</div>
              <button
                className="brutal-btn brutal-btn--sm"
                style={{ width: "100%" }}
                onClick={() => { resetBoard(other(aiColor)); setPanelOpen(false); }}
                title="交换先后手；换成 AI 执黑时它会立刻落第一手"
              >
                你执{humanColor === "black" ? "黑" : "白"} · 点此换边
              </button>
            </div>

            <div style={{ display: "flex", gap: 8 }}>
              <button
                className="brutal-btn brutal-btn--sm"
                style={{ flex: 1 }}
                disabled={history.length === 0 || !!winner || thinking}
                onClick={() => { undo(); setPanelOpen(false); }}
                title={history.length === 0 ? "还没有可悔的棋" : "退回一个回合（你的一手 + AI 的一手）"}
              >
                悔棋
              </button>
              <button
                className="brutal-btn brutal-btn--sm brutal-btn--primary"
                style={{ flex: 1 }}
                onClick={() => { resetBoard(); setPanelOpen(false); }}
              >
                重开
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
