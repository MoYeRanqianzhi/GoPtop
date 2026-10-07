/**
 * useGameSession 挂载生命周期的回归测试：StrictMode 双挂载的 adopt guard、
 * 建会话失败的错误态、失败后的重试。
 *
 * 本仓没有 jsdom/RTL（不为此加依赖），这里用 node + react-dom/client 跑**真 React**：
 * 给 react-dom 初始化必碰的那一小面 DOM（nodeType/tagName/namespaceURI/createElement…）
 * 装一个假面，被渲染组件返回 null，挂载 effect 就会真实执行——StrictMode 的
 * setup→cleanup→setup 交错因此是真的，不是手搓模拟。会话门面整体 mock
 * （../net/session），用 gate 控制 createSession 的落地时机来还原 IPC 竞态。
 *
 * 钉死的缺陷形态（修复前全部成立）：
 * - cleanup 只置 disposed、从不 dispose → 每次 WebView 重载/dev 重挂漏一个
 *   带原生泵/presence/WSS 的僵尸会话；
 * - StrictMode 下后落地的孤儿若不补 dispose、或先挂载的 cleanup 误杀后采用的
 *   会话，都会让界面拿到死会话；
 * - createSession 失败零捕获 → unhandled rejection，UI 静默变砖。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import React from "react";

/* ---------------- 会话门面替身（gate 控制 createSession 的落地时机） ---------------- */

type Stub = {
  id: string;
  disposeCalls: number;
  startPumpCalls: number;
  start_pump(): void;
  snapshot(): string;
  dispose(): Promise<void>;
};

const sess = vi.hoisted(() => {
  return {
    seq: 0,
    /** createSession 真正返回出去的句柄（dispose/start_pump 的调用计数在这里）。 */
    handles: [] as { id: string; disposeCalls: number; startPumpCalls: number }[],
    /** 队首 gate 存在时，下一次 createSession 在返回前停在它上面（模拟 IPC 慢于 cleanup）。 */
    gates: [] as Promise<void>[],
    /** 非 null 时下一次 createSession 直接抛这个错。 */
    failNext: null as Error | null,
  };
});

vi.mock("../net/session", () => ({
  createSession: async (): Promise<Stub> => {
    if (sess.failNext) {
      const e = sess.failNext;
      sess.failNext = null;
      throw e;
    }
    const st: Stub = {
      id: `s${++sess.seq}`,
      disposeCalls: 0,
      startPumpCalls: 0,
      start_pump() {
        this.startPumpCalls++;
      },
      // parseSnap 只要求一段 JSON；userId/name 够断言「采用的是哪一个会话」
      snapshot() {
        return JSON.stringify({ userId: this.id, name: `名-${this.id}` });
      },
      async dispose() {
        this.disposeCalls++;
      },
    };
    // 先登记句柄再等 gate：测试按创建顺序拿到「第一次挂载 / 第二次挂载」的句柄
    sess.handles.push(st);
    const gate = sess.gates.shift();
    if (gate) await gate;
    return st;
  },
}));

function stubAt(i: number): Stub {
  const st = sess.handles[i];
  if (!st) throw new Error(`第 ${i + 1} 个会话句柄不存在（一共 ${sess.handles.length} 个）`);
  return st as unknown as Stub;
}

/* ---------------- react-dom 在 node 下的最小 DOM 假面 ---------------- */

function makeNode(type: number): Record<string, unknown> {
  const children: Record<string, unknown>[] = [];
  const node: Record<string, unknown> = {
    nodeType: type,
    nodeName: type === 9 ? "#document" : type === 3 ? "#text" : "div",
    tagName: "DIV",
    namespaceURI: "http://www.w3.org/1999/xhtml",
    parentNode: null,
    style: {},
    attributes: {},
    children,
    addEventListener: () => {},
    removeEventListener: () => {},
    appendChild(c: Record<string, unknown>) {
      children.push(c);
      (c as { parentNode: unknown }).parentNode = node;
      return c;
    },
    removeChild(c: Record<string, unknown>) {
      const i = children.indexOf(c);
      if (i >= 0) children.splice(i, 1);
      return c;
    },
    insertBefore(c: Record<string, unknown>, b: Record<string, unknown> | null) {
      const i = b ? children.indexOf(b) : -1;
      if (i >= 0) children.splice(i, 0, c);
      else children.push(c);
      return c;
    },
    hasChildNodes: () => children.length > 0,
    get firstChild() {
      return children[0] ?? null;
    },
    get lastChild() {
      return children[children.length - 1] ?? null;
    },
    setAttribute(k: string, v: unknown) {
      (node as { attributes: Record<string, unknown> }).attributes[k] = v;
    },
    getAttribute: (k: string) => (node as { attributes: Record<string, unknown> }).attributes[k] ?? null,
    removeAttribute: (k: string) => {
      delete (node as { attributes: Record<string, unknown> }).attributes[k];
    },
    contains: () => false,
    compareDocumentPosition: () => 0,
    get ownerDocument() {
      return doc;
    },
    setTextContent(v: string) {
      children.length = 0;
      children.push(makeText(v));
    },
  };
  return node;
}

function makeText(t: string): Record<string, unknown> {
  const n = makeNode(3);
  n.textContent = t;
  n.nodeValue = t;
  return n;
}

const doc: Record<string, unknown> = {
  nodeType: 9,
  documentElement: null as unknown,
  body: null as unknown,
  addEventListener: () => {},
  removeEventListener: () => {},
  createElement: (tag: string) => {
    const n = makeNode(1);
    n.tagName = String(tag).toUpperCase();
    return n;
  },
  createTextNode: (t: string) => makeText(t),
  createComment: () => makeNode(8),
  getElementsByTagName: () => [],
  getElementById: () => null,
  activeElement: null,
  hasFocus: () => true,
};
doc.documentElement = makeNode(1);
doc.body = makeNode(1);

const g = globalThis as unknown as Record<string, unknown>;
g.document = doc;
g.window = globalThis;
// react-dom 的 commit 前置检查（getActiveElementDeep）会 instanceof 它
g.HTMLIFrameElement = class HTMLIFrameElement {};
// parseUrl/shareOrigin 读 window.location
g.location = new URL("http://localhost/");
// node 的 globalThis 不是 EventTarget：补上 popstate 监听/派发用的最小面
const listeners = new Map<string, Set<(e: unknown) => void>>();
g.addEventListener = (type: string, fn: (e: unknown) => void) => {
  if (!listeners.has(type)) listeners.set(type, new Set());
  listeners.get(type)!.add(fn);
};
g.removeEventListener = (type: string, fn: (e: unknown) => void) => {
  listeners.get(type)?.delete(fn);
};
g.dispatchEvent = (e: { type: string }) => {
  for (const fn of [...(listeners.get(e.type) ?? [])]) fn(e);
  return true;
};
// nav() 只在宿主动作里碰到；给个空实现防意外
g.history = { pushState: () => {}, replaceState: () => {} };

/* ---------------- 挂载探针 ---------------- */

const flush = () => new Promise((r) => setTimeout(r, 25));

const roots: { unmount(): void }[] = [];

/** strict=true 包一层 StrictMode（还原 main.tsx 的 dev 形态），false 为生产单挂载。 */
async function openProbe(strict: boolean) {
  const { createRoot } = await import("react-dom/client");
  const { useGameSession } = await import("./useGameSession");
  const renders: ReturnType<typeof useGameSession>[] = [];
  const Probe = () => {
    renders.push(useGameSession());
    return null;
  };
  const root = createRoot(makeNode(1) as unknown as Element);
  roots.push(root);
  root.render(strict ? React.createElement(React.StrictMode, null, React.createElement(Probe)) : React.createElement(Probe));
  await flush();
  return {
    root,
    last: () => renders[renders.length - 1]!,
  };
}

beforeEach(() => {
  sess.seq = 0;
  sess.handles = [];
  sess.gates = [];
  sess.failNext = null;
  for (const k of ["goptopOnChange", "goptopNotice", "goptopCopy", "__session"]) delete g[k];
});

afterEach(() => {
  for (const r of roots.splice(0)) r.unmount();
  vi.restoreAllMocks();
});

/* ---------------- 用例 ---------------- */

describe("useGameSession 挂载生命周期", () => {
  it("StrictMode 双挂载：第一次的孤儿会话被补释放，第二次正常接线、卸载时才释放", async () => {
    // 让第一次 createSession 慢于 StrictMode 的 cleanup——线上真实竞态形态
    let release!: () => void;
    sess.gates.push(new Promise<void>((r) => (release = r)));
    const probe = await openProbe(true);
    await flush();

    release();
    await flush();

    expect(sess.handles, "两次挂载各发起一次 createSession").toHaveLength(2);
    const first = stubAt(0);
    const second = stubAt(1);
    expect(first.disposeCalls, "孤儿会话必须补释放——它已带原生泵/presence 常驻").toBe(1);
    expect(second.disposeCalls, "活会话不得被第一次挂载的 cleanup 误杀").toBe(0);
    expect(second.startPumpCalls).toBe(1);
    expect((g.__session as Stub)?.id).toBe(second.id);
    expect(probe.last().tabUser, "采用的是第二次的会话").toBe(second.id);
    expect(probe.last().bootErr).toBeNull();

    probe.root.unmount();
    await flush();
    expect(second.disposeCalls, "卸载要释放活会话").toBe(1);
    expect(g.goptopOnChange, "卸载要摘掉本挂载装的三个钩子").toBeUndefined();
    expect(g.goptopNotice).toBeUndefined();
    expect(g.goptopCopy).toBeUndefined();
  });

  it("createSession 失败：bootErr 呈现原文、不留哑会话（不能静默变砖）", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    sess.failNext = new Error('原生会话创建失败: {"error":"boom"}');
    const probe = await openProbe(false);
    await flush();

    expect(probe.last().bootErr, "失败必须进错误态").toContain("boom");
    expect(errSpy, "原始错误要留痕（设备上唯一可归因的线索）").toHaveBeenCalled();
    expect(sess.handles, "失败的 createSession 不得产生被采用的会话").toHaveLength(0);
    expect(g.__session).toBeUndefined();

    probe.root.unmount();
    await flush();
  });

  it("retryBoot：失败后重跑挂载流程，成功后错误态清空、会话接线、卸载可释放", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    sess.failNext = new Error("boom-1");
    const probe = await openProbe(false);
    await flush();
    expect(probe.last().bootErr).toContain("boom-1");

    probe.last().retryBoot();
    await flush();

    expect(sess.handles, "重试应新建恰好一个会话").toHaveLength(1);
    const st = stubAt(0);
    expect(st.startPumpCalls).toBe(1);
    expect((g.__session as Stub)?.id).toBe(st.id);
    expect(probe.last().bootErr, "成功后错误态要清空").toBeNull();

    probe.root.unmount();
    await flush();
    expect(st.disposeCalls, "重试出的会话卸载时同样要释放").toBe(1);
    expect(errSpy).toHaveBeenCalled();
  });
});
