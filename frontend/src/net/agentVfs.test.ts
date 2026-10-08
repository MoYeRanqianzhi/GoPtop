/**
 * net/agentVfs 单测——`window.goptopVfsCall` 的契约钉子（契约
 * `.agents/plan/2026-10-08-agent-battle-web.md` §4：五 op 形状 / 镜像读写 /
 * write-behind / 坏 payload 折 error）。
 *
 * IndexedDB 用**内存桩**（仓库无 fake-indexeddb 依赖，也不为单测新增 devDep）：
 * 桩只实现 agentVfs.ts 实际用到的最小面（open/transaction/getAll/put/delete，
 * 事件经微任务投递），`rows` 数组即「盘上数据」——重载用例拿它播种新桩，等价于
 * 刷新页面后从 IndexedDB 重新装载。Rust 侧消费的 JSON 形状逐字比对（不是
 * toMatchObject——回执形状是冻结契约，多一个少一个字段都算偏移）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Row = { ns: string; path: string; content: string };

/** 内存 IndexedDB 桩：返回 { rows } 供断言/播种。failOpen=true 模拟存储打不开。 */
function installIdbStub(seed: Row[] = [], opts: { failOpen?: boolean } = {}): { rows: Row[] } {
  const rows: Row[] = seed.map((r) => ({ ...r }));
  const eq = (a: Row, key: [string, string]) => a.ns === key[0] && a.path === key[1];
  const mkReq = (result: unknown) => ({
    onsuccess: null as (() => void) | null,
    onerror: null as (() => void) | null,
    result,
    error: null as unknown,
  });
  const ok = (req: { onsuccess: (() => void) | null }) => queueMicrotask(() => req.onsuccess?.());

  const store = {
    getAll() {
      const req = mkReq(rows.map((r) => ({ ...r })));
      ok(req);
      return req;
    },
    put(row: Row) {
      const i = rows.findIndex((a) => eq(a, [row.ns, row.path]));
      if (i >= 0) rows[i] = { ...row };
      else rows.push({ ...row });
      const req = mkReq(undefined);
      ok(req);
      return req;
    },
    delete(key: [string, string]) {
      const i = rows.findIndex((a) => eq(a, key));
      if (i >= 0) rows.splice(i, 1);
      const req = mkReq(undefined);
      ok(req);
      return req;
    },
  };

  (globalThis as unknown as { indexedDB?: unknown }).indexedDB = {
    open(_name: string, _version: number) {
      const req = { ...mkReq(null), onupgradeneeded: null as (() => void) | null };
      queueMicrotask(() => {
        if (opts.failOpen) {
          req.error = new Error("storage disabled");
          req.onerror?.();
          return;
        }
        req.result = {
          transaction(_store: string, _mode?: string) {
            return { objectStore: () => store };
          },
        };
        ok(req);
      });
      return req;
    },
  };
  return { rows };
}

function installWindow(extra: Record<string, unknown> = {}) {
  (globalThis as unknown as { window: unknown }).window = { ...extra };
}

/** 排空桩的微任务（写盘回调全在 queueMicrotask 里，setTimeout(0) 前必排完）。 */
const flush = () => new Promise<void>((r) => {
  setTimeout(r, 0);
});

/** 每次取新模块：mirrors/db/ready 都是模块级状态。 */
async function freshAgentVfs() {
  vi.resetModules();
  return await import("../net/agentVfs");
}

type VfsCall = (op: string, payloadJson: string) => string;

function vfsCall(): VfsCall {
  const w = (globalThis as unknown as { window: { goptopVfsCall?: VfsCall } }).window;
  if (typeof w.goptopVfsCall !== "function") throw new Error("goptopVfsCall 未安装");
  return w.goptopVfsCall;
}

const j = (v: unknown) => JSON.stringify(v);

let disk: { rows: Row[] } | null = null;

beforeEach(() => {
  vi.resetModules();
  disk = null;
  delete (globalThis as { window?: unknown }).window;
  delete (globalThis as { indexedDB?: unknown }).indexedDB;
});

afterEach(() => {
  vi.resetModules();
  delete (globalThis as { window?: unknown }).window;
  delete (globalThis as { indexedDB?: unknown }).indexedDB;
});

describe("五 op 形状逐字对齐契约", () => {
  it("read/write/list/usage/delete 的成功与未命中回执", async () => {
    installIdbStub();
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    const call = vfsCall();

    // read 未命中：found:false + content:null（字段一个不能少）
    expect(call("read", j({ ns: "builtin", path: "notes/a.md" }))).toBe('{"ok":true,"found":false,"content":null}');
    // write → ok
    expect(call("write", j({ ns: "builtin", path: "notes/a.md", content: "第一版" }))).toBe('{"ok":true}');
    expect(call("read", j({ ns: "builtin", path: "notes/a.md" }))).toBe('{"ok":true,"found":true,"content":"第一版"}');

    // list：字典序 + prefix 过滤
    call("write", j({ ns: "builtin", path: "notes/b.md", content: "b" }));
    call("write", j({ ns: "builtin", path: "top.md", content: "t" }));
    expect(call("list", j({ ns: "builtin", prefix: "" }))).toBe('{"ok":true,"paths":["notes/a.md","notes/b.md","top.md"]}');
    expect(call("list", j({ ns: "builtin", prefix: "notes/" }))).toBe('{"ok":true,"paths":["notes/a.md","notes/b.md"]}');

    // usage：UTF-8 字节（中文每字 3 字节），与 native 的 bytes_used 同口径
    expect(call("usage", j({ ns: "builtin" }))).toBe('{"ok":true,"bytes":11}'); // 9 + 1 + 1

    // delete：首次真删、再删报 false（幂等，不是错误）
    expect(call("delete", j({ ns: "builtin", path: "top.md" }))).toBe('{"ok":true,"deleted":true}');
    expect(call("delete", j({ ns: "builtin", path: "top.md" }))).toBe('{"ok":true,"deleted":false}');
    expect(call("usage", j({ ns: "builtin" }))).toBe('{"ok":true,"bytes":10}');
  });

  it("ns 透传不硬编码：两 ns 镜像互不串", async () => {
    installIdbStub();
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    const call = vfsCall();
    call("write", j({ ns: "builtin", path: "a.md", content: "内置" }));
    call("write", j({ ns: "mcp", path: "a.md", content: "x" }));
    expect(call("read", j({ ns: "builtin", path: "a.md" }))).toBe('{"ok":true,"found":true,"content":"内置"}');
    expect(call("read", j({ ns: "mcp", path: "a.md" }))).toBe('{"ok":true,"found":true,"content":"x"}');
    expect(call("usage", j({ ns: "mcp" }))).toBe('{"ok":true,"bytes":1}');
  });
});

describe("坏 payload 折 error（永不 throw）", () => {
  it("坏 JSON / 缺字段 / 未知 op 一律回 {ok:false,error}，调用本身不抛", async () => {
    installIdbStub();
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    const call = vfsCall();

    // 坏 JSON 的 error 文案是引擎的 SyntaxError 原文（跨 Node 版本措辞不同），只钉形状
    const bad = JSON.parse(call("read", "不是 JSON")) as { ok: boolean; error: string };
    expect(bad.ok).toBe(false);
    expect(bad.error.length, "error 必须是人话原文").toBeGreaterThan(0);
    expect(call("write", j({ ns: "builtin" }))).toBe('{"ok":false,"error":"write 需要 ns、path 与 content"}');
    expect(call("write", j({ ns: "builtin", path: "a.md", content: 42 }))).toBe('{"ok":false,"error":"write 需要 ns、path 与 content"}');
    expect(call("delete", j({ path: "a.md" }))).toBe('{"ok":false,"error":"delete 需要 ns 与 path"}');
    expect(call("list", j({}))).toBe('{"ok":false,"error":"list 需要 ns"}');
    expect(call("usage", j({}))).toBe('{"ok":false,"error":"usage 需要 ns"}');
    expect(call("nope", j({}))).toBe('{"ok":false,"error":"未知 op: nope"}');
    // 折 error 之后镜像无半截写入
    expect(call("usage", j({ ns: "builtin" }))).toBe('{"ok":true,"bytes":0}');
  });
});

describe("内存镜像 + IndexedDB write-behind", () => {
  it("启动从 IndexedDB 全量装载进镜像", async () => {
    disk = installIdbStub([{ ns: "builtin", path: "notes/old.md", content: "旧记忆" }]);
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    expect(vfsCall()("read", j({ ns: "builtin", path: "notes/old.md" }))).toBe('{"ok":true,"found":true,"content":"旧记忆"}');
  });

  it("写先改镜像后落盘；刷新（重装模块+盘上数据播种）后仍在", async () => {
    disk = installIdbStub();
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    vfsCall()("write", j({ ns: "builtin", path: "notes/keep.md", content: "坚持" }));
    // 镜像立即可读（不依赖落盘完成）
    expect(vfsCall()("read", j({ ns: "builtin", path: "notes/keep.md" }))).toBe('{"ok":true,"found":true,"content":"坚持"}');
    await flush();
    expect(disk.rows, "write-behind 要把记录送到 IndexedDB").toEqual([
      { ns: "builtin", path: "notes/keep.md", content: "坚持" },
    ]);

    // 模拟刷新：盘上数据播种新桩 → 重新装载 → 可见
    installIdbStub(disk.rows);
    installWindow();
    const fresh = await freshAgentVfs();
    await fresh.agentVfsReady();
    expect(vfsCall()("read", j({ ns: "builtin", path: "notes/keep.md" }))).toBe('{"ok":true,"found":true,"content":"坚持"}');
  });

  it("delete 同步落盘（镜像删了盘上也删）", async () => {
    disk = installIdbStub([{ ns: "builtin", path: "a.md", content: "x" }]);
    installWindow();
    const { agentVfsReady } = await freshAgentVfs();
    await agentVfsReady();
    vfsCall()("delete", j({ ns: "builtin", path: "a.md" }));
    await flush();
    expect(disk.rows).toEqual([]);
  });
});

describe("IndexedDB 打不开（隐私模式等）：最佳努力持久层", () => {
  it("agentVfsReady reject 字符串；钩子仍在，镜像读写照常", async () => {
    installIdbStub([], { failOpen: true });
    installWindow();
    const m = await freshAgentVfs();
    await expect(m.agentVfsReady()).rejects.toBe("storage disabled");
    const call = vfsCall();
    expect(call("write", j({ ns: "builtin", path: "a.md", content: "会话内" }))).toBe('{"ok":true}');
    expect(call("read", j({ ns: "builtin", path: "a.md" }))).toBe('{"ok":true,"found":true,"content":"会话内"}');
  });
});

describe("无 window 环境（SSR/node 守卫）", () => {
  it("不装钩子、不启动装载，模块导入不炸", async () => {
    delete (globalThis as { window?: unknown }).window;
    await freshAgentVfs();
    expect((globalThis as { window?: unknown }).window).toBeUndefined();
  });
});
