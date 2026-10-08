/**
 * net/agentVfs —— Agent 记忆库（/memory）的 Web 存储桥：`window.goptopVfsCall`。
 *
 * Rust 侧（goptop-agent 的 store_web.rs，wasm 目标）把 VfsStore 五个同步方法逐字
 * 映射到本钩子上（契约 `.agents/plan/2026-10-08-agent-battle-web.md` §4，冻结面）：
 *
 *   goptopVfsCall(op: "read"|"write"|"delete"|"list"|"usage", payloadJson: string) -> string
 *
 * **同步、JSON 进出、永不 throw**：VfsStore trait 是同步 fn（rusqlite 连接不跨
 * await），Promise 版要么把 trait 改 async（牵动 native 全部实现与 registry/vfs
 * 调用面），要么在 vfs 里 cfg 出第二套 async 路径——两头的代价都远大于在 TS 侧
 * 吸收 IndexedDB 的异步。返回恒为 JSON 文本，任何异常折成 `{"ok":false,"error"}`。
 *
 * op 表（字段名 camelCase，与 store.rs 的 VfsStore 五方法一一对应；配额在 Rust 侧查）：
 * - read   `{"ns","path"}`            → `{"ok":true,"found":true,"content"}` ／ `{"ok":true,"found":false,"content":null}`
 * - write  `{"ns","path","content"}`  → `{"ok":true}`
 * - delete `{"ns","path"}`            → `{"ok":true,"deleted":bool}`
 * - list   `{"ns","prefix"}`          → `{"ok":true,"paths":[...]}`（字典序）
 * - usage  `{"ns"}`                   → `{"ok":true,"bytes":N}`（UTF-8 字节数，与 native 的 bytes_used 同口径）
 *
 * `path` 一律是**存表键形态**（已剥 `/memory/` 前缀、未规范化——规范化在 Rust 侧
 * normalize_path 做），本层原样存取不做任何加工；`ns` 同样透传（web 只有 "builtin"，
 * MCP 不上 web，但实现不硬编码该值）。
 *
 * 数据面 = **内存镜像 + IndexedDB write-behind**（net/store.ts 的既有架构同款：
 * 调用点在同步路径，先改内存再异步落盘）：模块装载即启动从 IndexedDB 全量装载进
 * Map → `agentVfsReady()`；write/delete 先改 Map 再 fire-and-forget 落 IndexedDB
 * （失败 console.warn 留副本不阻塞——会话内一致性由镜像保证，刷新后可能丢最近
 * 写入，记忆是最佳努力持久层，与 store.ts 的兜底哲学一致）。
 */

const DB_NAME = "goptop-agent-vfs";
const STORE = "files";

/** 一行落盘记录（复合主键 [ns, path]）。 */
type VfsRow = { ns: string; path: string; content: string };

/** 内存镜像：ns → (存表键 path → content)。read/list/usage 全部只读这里。 */
const mirrors = new Map<string, Map<string, string>>();

let db: IDBDatabase | null = null;
let readyResolve: () => void;
let readyReject: (reason: string) => void;
/** 装载完成信号；IndexedDB 打不开时 reject 字符串（AgentPage 据此仍可降级评估）。 */
const ready = new Promise<void>((res, rej) => {
  readyResolve = res;
  readyReject = rej;
});

/** Agent 记忆库持久层是否已从 IndexedDB 装载完毕。**消费方（AgentPage）必须
 *  await 它之后才允许 agent_start**——否则首写只进镜像、装载完成后的全量读取
 *  会把镜像里已有的条目误当「盘上没有」。 */
export function agentVfsReady(): Promise<void> {
  return ready;
}

function mirror(ns: string): Map<string, string> {
  let m = mirrors.get(ns);
  if (!m) {
    m = new Map();
    mirrors.set(ns, m);
  }
  return m;
}

function errReply(message: string): string {
  return JSON.stringify({ ok: false, error: message });
}

const textBytes = new TextEncoder();

/** write-behind：先改镜像后调用；失败只告警（留副本不阻塞，Rust 侧语义不受影响）。 */
function persist(kind: "put" | "delete", row: VfsRow): void {
  if (!db) return;
  try {
    const tx = db.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    const req = kind === "put" ? store.put(row) : store.delete([row.ns, row.path]);
    req.onerror = () => console.warn("[agentVfs] IndexedDB 落盘失败，保留内存副本", req.error);
  } catch (e) {
    console.warn("[agentVfs] IndexedDB 落盘失败，保留内存副本", e);
  }
}

/** 钩子本体（契约见文件头）。所有入参都当外部输入防：坏 JSON/缺字段一律折 error 回执。 */
function vfsCall(op: string, payloadJson: string): string {
  try {
    const p = JSON.parse(payloadJson) as Record<string, unknown>;
    const ns = typeof p.ns === "string" ? p.ns : null;
    const path = typeof p.path === "string" ? p.path : null;
    if (op === "read") {
      if (!ns || !path) return errReply("read 需要 ns 与 path");
      const content = mirror(ns).get(path);
      return content === undefined
        ? JSON.stringify({ ok: true, found: false, content: null })
        : JSON.stringify({ ok: true, found: true, content });
    }
    if (op === "write") {
      if (!ns || !path || typeof p.content !== "string") return errReply("write 需要 ns、path 与 content");
      mirror(ns).set(path, p.content);
      persist("put", { ns, path, content: p.content });
      return JSON.stringify({ ok: true });
    }
    if (op === "delete") {
      if (!ns || !path) return errReply("delete 需要 ns 与 path");
      const deleted = mirror(ns).delete(path);
      if (deleted) persist("delete", { ns, path, content: "" });
      return JSON.stringify({ ok: true, deleted });
    }
    if (op === "list") {
      if (!ns) return errReply("list 需要 ns");
      const prefix = typeof p.prefix === "string" ? p.prefix : "";
      const paths = [...mirror(ns).keys()].filter((k) => k.startsWith(prefix)).sort();
      return JSON.stringify({ ok: true, paths });
    }
    if (op === "usage") {
      if (!ns) return errReply("usage 需要 ns");
      let bytes = 0;
      for (const content of mirror(ns).values()) bytes += textBytes.encode(content).length;
      return JSON.stringify({ ok: true, bytes });
    }
    return errReply(`未知 op: ${op}`);
  } catch (e) {
    return errReply(e instanceof Error ? e.message : String(e));
  }
}

/** 启动装载：打开库 → 全量读进镜像 → ready。失败 reject 字符串（镜像留空，
 *  钩子仍在——会话内读写照常，只是没有历史记忆可读、写入落不了盘）。 */
function load(): void {
  const factory = (globalThis as { indexedDB?: IDBFactory }).indexedDB;
  if (!factory) {
    readyReject("此环境没有 indexedDB");
    return;
  }
  let req: IDBOpenDBRequest;
  try {
    req = factory.open(DB_NAME, 1);
  } catch (e) {
    readyReject(String(e));
    return;
  }
  req.onupgradeneeded = () => {
    req.result.createObjectStore(STORE, { keyPath: ["ns", "path"] });
  };
  req.onerror = () => readyReject(String(req.error?.message ?? req.error ?? "IndexedDB 打开失败"));
  req.onsuccess = () => {
    db = req.result;
    try {
      const greq = db.transaction(STORE, "readonly").objectStore(STORE).getAll();
      greq.onsuccess = () => {
        for (const row of greq.result as VfsRow[]) {
          if (typeof row?.ns === "string" && typeof row?.path === "string" && typeof row?.content === "string") {
            mirror(row.ns).set(row.path, row.content);
          }
        }
        readyResolve();
      };
      greq.onerror = () => readyReject(String(greq.error?.message ?? greq.error ?? "IndexedDB 装载失败"));
    } catch (e) {
      readyReject(String(e));
    }
  };
}

// 模块加载即装钩子并启动装载（与 net/store.ts 的 installHostHooks 同时机同手法）：
// Rust 侧 WebStore 只认 window.goptopVfsCall，装晚了首批 Vfs 调用会扑空。
if (typeof window !== "undefined") {
  (window as unknown as { goptopVfsCall?: typeof vfsCall }).goptopVfsCall = vfsCall;
  load();
}
