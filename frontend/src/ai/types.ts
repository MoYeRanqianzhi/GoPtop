/**
 * ai/types —— AI 分析层的类型契约。
 *
 * 真源是 crates/goptop-ai/src/lib.rs 的 `AnalyzeRequest` / `AnalyzeResult`
 * （serde rename_all = "camelCase"），改动必须两边同步。
 *
 * 颜色字符串是**大驼峰**（"Black"/"White"）而非前端的 "black"/"white"：这一层直接
 * 由 serde 反序列化成 goptop-core 的 `Stone` 枚举，用类型把大小写钉死，避免拼错时
 * 只在 AI 侧静默变成"未知颜色"。
 */

/** 我方执色（serde 形态，非前端的小写约定）。 */
export type AiColor = "Black" | "White";

/** 分析请求。 */
export type AnalyzeRequest = {
  /**
   * 局面 JSON，由规则引擎的 `WasmGame.state_json()` 给出——不手工构造。
   * 围棋的劫点/提子数/计分态只在 `GameState` 里，前端拼不出来。
   */
  state: unknown;
  /** 胜率以哪一方为准。本地双人/观战没有"我方"，由调用方指定黑方。 */
  myColor: AiColor;
  /** 思考预算（毫秒）。 */
  budgetMs: number;
  /** 是否需要最佳着法；纯胜率分析传 false 可省下选点开销。 */
  wantMove: boolean;
};

/** 分析结果。 */
export type AnalyzeResult = {
  /** 最佳着法 [x, y]；无需着法或无处可下时为 null。 */
  bestMove: [number, number] | null;
  /** 视角方（myColor）胜率，0..1。 */
  winRate: number;
  /** 五子棋为 α-β 深度；围棋为搜索树深度。 */
  depth: number;
  /** 五子棋为节点数；围棋为 playout 数。 */
  nodes: number;
  elapsedMs: number;
};

/** Worker 请求/回复的信封（id 用于把回复配回 Promise）。 */
export type WorkerRequest =
  | { id: number; kind: "analyze"; req: AnalyzeRequest }
  | { id: number; kind: "warmup" };

export type WorkerResponse =
  | { id: number; ok: true; result: AnalyzeResult }
  | { id: number; ok: true; warmed: true }
  | { id: number; ok: false; error: string };
