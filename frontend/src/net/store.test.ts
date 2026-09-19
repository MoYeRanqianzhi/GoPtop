/**
 * net/store 门面单测：后端探测、平台侧读写、失败回退浏览器存储、首次迁移。
 *
 * 门面是模块级单例（内存表 + ready 标志），所以每个用例都要 `vi.resetModules()`
 * 重新 import，并先把全局环境（window.goptopStore / __TAURI_INTERNALS__）铺好。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

/** 造一个可控的 localStorage 替身（本仓无 jsdom，vitest 默认 environment=node，没有全局 localStorage，必须显式注入）。 */
function stubLocal(init: Record<string, string> = {}) {
  const box = new Map(Object.entries(init));
  const ls = {
    get length() { return box.size; },
    key: (i: number) => [...box.keys()][i] ?? null,
    getItem: (k: string) => (box.has(k) ? box.get(k)! : null),
    setItem: (k: string, v: string) => void box.set(k, v),
    removeItem: (k: string) => void box.delete(k),
    clear: () => box.clear(),
  };
  vi.stubGlobal("localStorage", ls);
  return box;
}

/** 重新 import 门面（拿到干净的模块级状态）。 */
async function freshStore() {
  vi.resetModules();
  return import("./store");
}

beforeEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  delete (globalThis as { window?: unknown }).window;
});

describe("store 门面", () => {
  it("无 Tauri 无鸿蒙桥 → 浏览器后端，读写落 localStorage", async () => {
    const box = stubLocal({ "goptop:name": "旧昵称" });
    const { storeInit, storeGet, storeSet, storeBackend } = await freshStore();
    await storeInit();
    expect(storeBackend()).toBe("browser");
    expect(storeGet("goptop:name")).toBe("旧昵称");
    storeSet("goptop:name", "新昵称");
    expect(box.get("goptop:name")).toBe("新昵称");
  });

  it("鸿蒙桥存在 → harmony 后端，读取走桥、写入落桥", async () => {
    stubLocal({ "goptop:name": "浏览器里的旧值" });
    const saved = new Map<string, string>([["goptop:name", "桥里的值"]]);
    const bridge = {
      load: () => JSON.stringify(Object.fromEntries(saved)),
      set: (k: string, v: string) => void saved.set(k, v),
      remove: (k: string) => void saved.delete(k),
    };
    vi.stubGlobal("window", { goptopStore: bridge });
    const { storeInit, storeGet, storeSet, storeBackend } = await freshStore();
    await storeInit();
    expect(storeBackend()).toBe("harmony");
    expect(storeGet("goptop:name")).toBe("桥里的值");
    storeSet("goptop:defaults", '{"kind":"go"}');
    expect(saved.get("goptop:defaults")).toBe('{"kind":"go"}');
  });

  it("平台侧首次为空 → 把浏览器存储里的旧设置迁移过去（一次性）", async () => {
    stubLocal({ "goptop:name": "老大", "goptop:server-sel": "local" });
    const saved = new Map<string, string>();
    vi.stubGlobal("window", {
      goptopStore: {
        load: () => JSON.stringify(Object.fromEntries(saved)),
        set: (k: string, v: string) => void saved.set(k, v),
        remove: (k: string) => void saved.delete(k),
      },
    });
    const { storeInit, storeGet } = await freshStore();
    await storeInit();
    expect(storeGet("goptop:name")).toBe("老大");
    expect(saved.get("goptop:name")).toBe("老大");
    expect(saved.get("goptop:server-sel")).toBe("local");
  });

  it("平台侧已有数据 → 不迁移（浏览器旧值不得覆盖平台值）", async () => {
    stubLocal({ "goptop:name": "浏览器旧值" });
    vi.stubGlobal("window", {
      goptopStore: {
        load: () => JSON.stringify({ "goptop:name": "平台值" }),
        set: () => undefined,
        remove: () => undefined,
      },
    });
    const { storeInit, storeGet } = await freshStore();
    await storeInit();
    expect(storeGet("goptop:name")).toBe("平台值");
  });

  it("平台读取失败 → 回退浏览器后端（不是崩，也不是空设置）", async () => {
    stubLocal({ "goptop:name": "兜底值" });
    vi.stubGlobal("window", {
      goptopStore: {
        load: () => { throw new Error("桥挂了"); },
        set: () => { throw new Error("桥挂了"); },
        remove: () => { throw new Error("桥挂了"); },
      },
    });
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const { storeInit, storeGet, storeSet, storeBackend } = await freshStore();
    await storeInit();
    expect(storeBackend()).toBe("browser");
    expect(storeGet("goptop:name")).toBe("兜底值");
    storeSet("goptop:name", "改一下");
    expect(globalThis.localStorage.getItem("goptop:name")).toBe("改一下");
  });

  it("平台写入失败 → 该条同时落到浏览器存储兜底", async () => {
    stubLocal();
    vi.stubGlobal("window", {
      goptopStore: {
        load: () => "{}",
        set: () => { throw new Error("写失败"); },
        remove: () => { throw new Error("写失败"); },
      },
    });
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const { storeInit, storeSet, storeRemove } = await freshStore();
    await storeInit();
    storeSet("goptop:name", "兜底");
    expect(globalThis.localStorage.getItem("goptop:name")).toBe("兜底");
    storeRemove("goptop:name");
    expect(globalThis.localStorage.getItem("goptop:name")).toBeNull();
  });

  it("storeRemove 幂等；get 不命中原型链", async () => {
    stubLocal();
    const { storeInit, storeGet, storeRemove } = await freshStore();
    await storeInit();
    expect(storeGet("toString")).toBeNull();
    storeRemove("goptop:不存在");
    expect(storeGet("goptop:不存在")).toBeNull();
  });
});
