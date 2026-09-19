/**
 * net/store —— 平台本地存储门面（用户拍板 2026-09-19：数据必须落在各平台规定的位置）。
 *
 * 各端落盘位置（本文件是**唯一**读写门面，业务模块不许再直接碰 localStorage）：
 * - 桌面壳（Tauri / Windows）：`~/.goptop/store.json`（Rust 侧 `src-tauri/src/store.rs`）；
 * - 移动端（Tauri 的 Android/iOS 后端）：平台给应用的私有数据目录（同一套 Rust 命令）；
 * - 鸿蒙壳（ArkWeb）：应用 `filesDir/store.json`（ArkTS 注入的 `goptopStore` 代理）；
 * - **Web 端：浏览器 localStorage**——这是 Web 端自身的落盘方式，也是其余端
 *   平台存储不可用时的兜底（无 Tauri 运行时、无鸿蒙桥、权限拒绝等）。
 *
 * 为什么装进内存：调用点遍布同步代码路径（昵称、服务器选择、STUN、默认规则、
 * 头像），改成异步会牵动一大片。因此启动时 `storeInit()` 一次性全量装载，
 * 之后 `storeGet` 读内存，`storeSet/Remove` 先改内存再异步落盘；落盘失败退回
 * localStorage 并留告警——宁可留一份浏览器副本，也不能让用户设置凭空消失。
 *
 * 键名约定：全部以 `goptop:` 开头（迁移与快照按此前缀筛选）。
 */

import { isTauri } from "./links";

/** 落盘后端：决定数据最终写到哪里（诊断与测试会读它）。 */
export type StoreBackend = "tauri" | "harmony" | "browser";

const PREFIX = "goptop:";

/** 鸿蒙壳注入的存储代理（ArkTS 侧 javaScriptProxy，见 harmony/.../pages/Index.ets）。 */
type HarmonyStore = {
  load(): string;
  set(key: string, value: string): void;
  remove(key: string): void;
};
declare global {
  interface Window {
    goptopStore?: HarmonyStore;
  }
}

let backend: StoreBackend = "browser";
let mem: Record<string, string> = {};
let ready = false;

/** 当前后端（测试与诊断用）。 */
export function storeBackend(): StoreBackend {
  return backend;
}

/** 内存快照（只读；`window.__store.dump()` 供 E2E 断言，不用于写入）。 */
export function storeDump(): Record<string, string> {
  return { ...mem };
}

function browserAll(): Record<string, string> {
  const out: Record<string, string> = {};
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith(PREFIX)) {
        const v = localStorage.getItem(k);
        if (v !== null) out[k] = v;
      }
    }
  } catch { /* 隐私模式等：当作没有 */ }
  return out;
}

function browserSet(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch { /* 配额/隐私模式 */ }
}

function browserRemove(key: string) {
  try {
    localStorage.removeItem(key);
  } catch { /* ignore */ }
}

/** 平台侧全量读取；返回 null 表示后端不可用（调用方回退浏览器存储）。 */
async function platformLoad(which: StoreBackend): Promise<Record<string, string> | null> {
  try {
    if (which === "harmony") {
      const raw = window.goptopStore?.load() ?? "";
      return raw.trim() ? (JSON.parse(raw) as Record<string, string>) : {};
    }
    const { invoke } = await import("@tauri-apps/api/core");
    const map = await invoke<Record<string, string>>("store_load");
    return map ?? {};
  } catch (e) {
    console.warn("[store] 平台存储不可用，回退浏览器存储", e);
    return null;
  }
}

/** 平台侧落盘；失败则把这一条同时写进浏览器存储兜底（留副本好过丢设置）。 */
function platformSave(which: StoreBackend, key: string, value: string | null) {
  const fallback = () => {
    if (value === null) browserRemove(key);
    else browserSet(key, value);
  };
  if (which === "harmony") {
    // 鸿蒙桥是同步调用：失败当场兜底
    try {
      if (value === null) window.goptopStore?.remove(key);
      else window.goptopStore?.set(key, value);
    } catch (e) {
      console.warn("[store] 鸿蒙桥写入失败，浏览器存储兜底", e);
      fallback();
    }
    return;
  }
  // Tauri invoke 是异步的：写盘结果晚于调用点，只有失败路径需要补兜底
  void (async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      if (value === null) await invoke("store_remove", { key });
      else await invoke("store_set", { key, value });
    } catch (e) {
      console.warn("[store] 平台存储写入失败，浏览器存储兜底", e);
      fallback();
    }
  })();
}

/** 后端探测：鸿蒙桥 → Tauri → 浏览器（浏览器是 Web 端本体，也是其余端的兜底）。 */
function detect(): StoreBackend {
  if (typeof window !== "undefined" && window.goptopStore) return "harmony";
  if (isTauri()) return "tauri";
  return "browser";
}

/**
 * 启动装载：必须在 React 渲染前 await（main.tsx 负责），否则首帧会读到空设置
 * ——昵称会重生成、服务器选择会跳回默认，用户看到的是「设置丢了」。
 *
 * 迁移：平台侧还没有任何 `goptop:` 键、而浏览器存储里有（旧版本把设置存在
 * WebView 的 localStorage 里）时，把旧数据搬到平台侧。只在首次发生一次。
 */
export async function storeInit(): Promise<void> {
  if (ready) return;
  backend = detect();
  if (backend === "browser") {
    mem = browserAll();
    ready = true;
    return;
  }
  const remote = await platformLoad(backend);
  if (remote === null) {
    backend = "browser";
    mem = browserAll();
    ready = true;
    return;
  }
  const hasPlatformData = Object.keys(remote).some((k) => k.startsWith(PREFIX));
  const legacy = hasPlatformData ? {} : browserAll();
  mem = { ...remote, ...legacy };
  ready = true;
  for (const [k, v] of Object.entries(legacy)) platformSave(backend, k, v);
}

export function storeGet(key: string): string | null {
  // hasOwnProperty 而非 `in`：`in` 会命中原型链（"toString" 之类）返回函数
  return Object.prototype.hasOwnProperty.call(mem, key) ? mem[key] : null;
}

export function storeSet(key: string, value: string): void {
  mem[key] = value;
  if (backend === "browser") browserSet(key, value);
  else platformSave(backend, key, value);
}

export function storeRemove(key: string): void {
  delete mem[key];
  if (backend === "browser") browserRemove(key);
  else platformSave(backend, key, null);
}

/**
 * 给 wasm 传输层装宿主存储钩子（goptop-transport 的 `storage_get/set` 会优先调用它们）。
 *
 * 为什么必须装：设置类数据的读写有两条入口——TS 侧 UI（本文件）与 wasm 状态机
 * （昵称保存走 `Effect::SetStorage`，服务器/STUN/头像在建会话时读取）。wasm 自己
 * 只能碰 localStorage，不装钩子的话它会把数据写到浏览器里、绕开平台存储，
 * 于是「桌面端设置落在 WebView 的 localStorage 而不是 ~/.goptop」。
 * 装上之后两条入口共用同一份内存表与同一套落盘位置。
 */
function installHostHooks() {
  const w = window as unknown as {
    goptopStorageGet?: (key: string) => string | null;
    goptopStorageSet?: (key: string, value: string | null) => void;
  };
  w.goptopStorageGet = (key: string) => storeGet(key);
  w.goptopStorageSet = (key: string, value: string | null) => {
    if (value === null) storeRemove(key);
    else storeSet(key, value);
  };
}

// E2E / 诊断读取面（只读；写入一律走上面的函数）。与 window.__session 同性质。
if (typeof window !== "undefined") {
  (window as unknown as { __store?: unknown }).__store = {
    backend: storeBackend,
    dump: storeDump,
  };
  installHostHooks();
}
