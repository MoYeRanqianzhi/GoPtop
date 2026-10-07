/**
 * 原生后端（Tauri invoke）的**时序契约**回归测试。
 *
 * 为什么必须在这里测：浏览器 E2E 跑的是 wasm 后端，桌面/Android 的 `invoke` 路径
 * 在 CI 里没有 Tauri 宿主可用。这里用一个「伪宿主」——把 `window.__TAURI_INTERNALS__`
 * 装成带 **IPC 延迟**的假 Rust 侧——把 `isTauri()` 拨到 true，于是 rules.ts 走
 * NativeBackend，而延迟恰好还原了线上真实存在的那段「新局尚未落地」的窗口。
 * 页面里的 effect 是 `void engine.newGame(...)` 不等它、下一个 effect 立刻
 * `stateJson()`，窗口就出在这里。
 *
 * 断言的量都是「能区分对错」的：返回的是新局还是旧局（尺寸比对 null）、
 * 失败后还会不会拿旧 id 落子、实例有没有被 drop。
 */
import { beforeEach, describe, expect, it } from "vitest";

type Reply = string | number | boolean | null | undefined;
type Kind = "gomoku" | "go";
type Sz = 9 | 13 | 15 | 19;

/** 假 Rust 侧的多局实例表（对应 src-tauri/src/rules.rs 的 Games）。 */
let games: Map<number, { size: number; moves: number }>;
let nextId: number;
let ipcDelayMs: number;
let calls: string[];
let failNextNew: boolean;
/** 鸿蒙假宿主用的同款失败开关（宿主对非法组合回 null，不抛异常）。 */
let harmonyFailNextNew: boolean;

beforeEach(() => {
  games = new Map();
  nextId = 1;
  ipcDelayMs = 30;
  calls = [];
  failNextNew = false;
  harmonyFailNextNew = false;
  (globalThis as unknown as { window: unknown }).window = {
    __TAURI_INTERNALS__: {
      invoke: async (cmd: string, args: Record<string, unknown>): Promise<Reply> => {
        calls.push(cmd);
        await new Promise((r) => setTimeout(r, ipcDelayMs));
        switch (cmd) {
          case "game_new": {
            if (failNextNew) {
              failNextNew = false;
              throw new Error("game_new failed");
            }
            const kind = JSON.parse(String(args.kindJson)) as { Gomoku?: { size: number }; Go?: { size: number } };
            const size = kind.Gomoku?.size ?? kind.Go?.size ?? 0;
            const id = nextId++;
            games.set(id, { size, moves: 0 });
            return id;
          }
          // 局面的「尺寸」就是这一局的尺寸：用它区分「拿到新局」与「拿到旧局」
          case "game_state_json": {
            const g = games.get(Number(args.id));
            return g ? JSON.stringify({ size: g.size, moves: g.moves }) : "null";
          }
          case "game_place": {
            const g = games.get(Number(args.id));
            if (!g) return JSON.stringify({ ok: false, error: "no_game" });
            g.moves += 1;
            return JSON.stringify({ ok: true, board: [], captured: [], toMove: "white", winner: null });
          }
          case "game_drop":
            games.delete(Number(args.id));
            return undefined;
          default:
            return undefined;
        }
      },
    },
  };
});

/** 每次都取一份新门面：后端在构造时按 `isTauri()` 选定。 */
async function engine() {
  const { RulesEngine } = await import("./rules");
  return new RulesEngine();
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 复刻页面时序：effect A 的 `void newGame(...)` 之后，同一个 commit 里立刻取局面。 */
async function stateRightAfter(eng: Awaited<ReturnType<typeof engine>>, kind: Kind, size: Sz) {
  void eng.newGame(kind, size);
  return (await eng.stateJson()) as { size?: number } | null;
}

/** 等对局真正落地（不依赖被测的 booting），供其余用例构造前置状态。 */
async function boot(eng: Awaited<ReturnType<typeof engine>>, kind: Kind, size: Sz) {
  void eng.newGame(kind, size);
  await sleep(ipcDelayMs * 5);
}

describe("原生后端：newGame 未 await 时的取局面", () => {
  it("开局后立刻取局面，拿到的是这一局（不是 null）", async () => {
    const eng = await engine();
    const st = await stateRightAfter(eng, "go", 9);
    expect(st, "窗口期返回 null → AI 落子 effect 会静默 return，AI 永不走棋").not.toBeNull();
    expect(st?.size).toBe(9);
  });

  it("切尺寸后立刻取局面，不得拿回上一局的快照", async () => {
    const eng = await engine();
    await boot(eng, "go", 9);
    const st = await stateRightAfter(eng, "go", 19);
    expect(st?.size, "取到的是上一局（尺寸不符）→ AI 照着幽灵局面选点").toBe(19);
  });

  it("game_new 抛错后不得继续用旧 id 落子", async () => {
    const eng = await engine();
    await boot(eng, "gomoku", 15);
    expect(await eng.place(1, 1), "前置：正常局应当能落子").not.toBeNull();

    failNextNew = true;
    void eng.newGame("go", 9);
    // 同一 tick 内接住 booting 的失败（否则 Node 会报 unhandled rejection）；
    // 本用例只关心 place 是否还在用旧 id，故不锁定失败向上传播的形态。
    await eng.stateJson().catch(() => undefined);
    expect(await eng.place(2, 2), "旧 id 还留着 → place 拿回上一局尺寸的棋盘覆盖 UI").toBeNull();
  });
});

describe("原生后端：Rust 侧实例生命周期", () => {
  it("换局要 drop 旧实例（否则 HashMap 里逐局累积）", async () => {
    const eng = await engine();
    await boot(eng, "gomoku", 15);
    const firstId = nextId - 1;
    await boot(eng, "go", 9);
    await sleep(ipcDelayMs * 5);
    expect(calls.filter((c) => c === "game_drop").length).toBe(1);
    expect(games.has(firstId)).toBe(false);
  });

  it("dispose() 释放当前实例（页面离页调它）", async () => {
    const eng = await engine();
    await boot(eng, "go", 9);
    await eng.dispose();
    await sleep(ipcDelayMs * 5);
    expect(games.size).toBe(0);
  });

  it("背靠背并发 newGame 只留一局（快速连点棋种/尺寸）", async () => {
    const eng = await engine();
    await boot(eng, "gomoku", 15);
    const firstId = nextId - 1;
    void eng.newGame("go", 9);
    void eng.newGame("go", 19);
    await sleep(ipcDelayMs * 5);
    // 两次并发都在对方落地前把 id 摘成 null：没有代次的话各自建局、各自不 drop，
    // Rust 侧存活 3 局（旧局 + 两个新局），先落地者永远无人认领
    expect(games.size, "并发建局必须只留最后一次请求的局").toBe(1);
    expect(games.has(firstId), "启动并发前的旧局也要被 drop").toBe(false);
    const st = (await eng.stateJson()) as { size?: number } | null;
    expect(st?.size, "存活局必须是后一次请求的尺寸").toBe(19);
  });

  it("dispose 撞上在途 newGame 不泄漏（StrictMode 卸载时序）", async () => {
    const eng = await engine();
    void eng.newGame("go", 9);
    // dispose 落地时 this.id 仍是 null（newGame 还在 IPC 往返中），无局可 drop——
    // 在途 newGame 必须凭过期代次自弃刚建出的局，否则 dev 下每次进页漏一局
    await eng.dispose();
    await sleep(ipcDelayMs * 5);
    expect(games.size).toBe(0);
  });
});

/* ---------------- 鸿蒙后端（同步桥） ----------------
 *
 * HarmonyBackend 是同一份时序契约的第二份实现，且桥形态独有：同步返回、回执是
 * 一整条 JSON 文本（单层编码：game_new 回 JSON.stringify(id)，state 文本原样）。
 * 这里的假桥与 crates/goptop-ohos 的 dispatch 同款（无此局给 no_game 而非 null）。
 * 注意 window 不能带 __TAURI_INTERNALS__，否则 pickBackend 会选 NativeBackend。
 */
describe("鸿蒙后端：同步桥的同一份时序契约", () => {
  /** 装鸿蒙假宿主（同步分发到与 native 用例同一张 games 表）。在外层 beforeEach 之后跑，整体替换 window。 */
  beforeEach(() => {
    (globalThis as unknown as { window: unknown }).window = {
      goptopHost: {
        storeLoad: () => "{}",
        storeSet: () => undefined,
        storeRemove: () => undefined,
        call: (cmd: string, argsJson: string): string => {
          calls.push(cmd);
          const args = JSON.parse(argsJson) as Record<string, unknown>;
          switch (cmd) {
            case "game_new": {
              if (harmonyFailNextNew) {
                harmonyFailNextNew = false;
                return "null";
              }
              const kind = JSON.parse(String(args.kindJson)) as { Gomoku?: { size: number }; Go?: { size: number } };
              const size = kind.Gomoku?.size ?? kind.Go?.size ?? 0;
              const id = nextId++;
              games.set(id, { size, moves: 0 });
              return JSON.stringify(id);
            }
            case "game_state_json": {
              const g = games.get(Number(args.id));
              return g ? JSON.stringify({ size: g.size, moves: g.moves }) : "null";
            }
            case "game_place": {
              const g = games.get(Number(args.id));
              if (!g) return JSON.stringify({ ok: false, error: "no_game" });
              g.moves += 1;
              return JSON.stringify({ ok: true, board: [], captured: [], toMove: "white", winner: null });
            }
            case "game_drop":
              games.delete(Number(args.id));
              return "null";
            default:
              return "null";
          }
        },
        aiPost: () => 0,
        aiPoll: () => "",
      },
    };
  });

  it("开局后立刻取局面，拿到的是这一局（不是 null）", async () => {
    const eng = await engine();
    const st = await stateRightAfter(eng, "go", 9);
    expect(st, "同步桥没有建局窗口，stateJson 仍必须等到 newGame 落地（booting）").not.toBeNull();
    expect(st?.size).toBe(9);
  });

  it("切尺寸后立刻取局面，不得拿回上一局的快照", async () => {
    const eng = await engine();
    await boot(eng, "go", 9);
    const st = await stateRightAfter(eng, "go", 19);
    expect(st?.size, "取到的是上一局（尺寸不符）→ AI 照着幽灵局面选点").toBe(19);
  });

  it("game_new 失败（回 null）后不得继续用旧 id 落子", async () => {
    const eng = await engine();
    await boot(eng, "gomoku", 15);
    expect((await eng.place(1, 1))?.ok, "前置：正常局应当能落子").toBe(true);

    harmonyFailNextNew = true;
    void eng.newGame("go", 9);
    expect(await eng.stateJson(), "失败后 this.id 必须已摘成 null").toBeNull();
    // 旧 id 若还留着，载荷会带 id、宿主回 ok:true——这里必须走 no_game
    expect((await eng.place(2, 2))?.ok ?? false, "不得拿旧 id 落子").toBe(false);
  });

  it("背靠背并发 newGame 不泄漏（同步体无 await，不可能交错）", async () => {
    const eng = await engine();
    await boot(eng, "gomoku", 15);
    void eng.newGame("go", 9);
    void eng.newGame("go", 19);
    await sleep(0);
    // 桥是同步的：第一次 newGame 在第二次启动前已完整落地并 drop 旧局
    expect(games.size).toBe(1);
    const st = (await eng.stateJson()) as { size?: number } | null;
    expect(st?.size).toBe(19);
  });
});
