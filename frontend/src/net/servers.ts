/**
 * net/servers — 信令服务器列表与选择（可选模式；单选切换，不用开关）。
 *
 * 两种使用方式：
 * - 无服务器（`none`）：纯 P2P，URL 信令 + 回执，无任何在线设施。
 * - 选中某台服务器：连上后获得在线名册与数据兜底中转（P2P 未建立时走 relay，
 *   默认直连优先）。**同一时刻只连一台**，不同服务器之间不能对战。
 * 官方服务器是内置默认项；用户可自行添加更多服务器（如自建部署）。
 */

import { storeGet, storeSet } from "./store";

export type ServerEntry = { id: string; label: string; url: string; builtin: boolean };

/** 内置官方服务器。换地址只改这一处。 */
export const BUILTIN_SERVERS: ServerEntry[] = [
  { id: "official", label: "官方服务器", url: "wss://goptopserver.meowoo.org/ws", builtin: true },
];

/** 「无服务器」的特殊选择值：不连任何服务器，行为与旧版完全一致。 */
export const SERVER_NONE = "none";

const SERVERS_KEY = "goptop:servers";
const SERVER_SEL_KEY = "goptop:server-sel";

/** 用户自定义服务器（builtin 项不入此列）。 */
export function loadServers(): ServerEntry[] {
  try {
    const raw = storeGet(SERVERS_KEY);
    const arr = raw ? (JSON.parse(raw) as ServerEntry[]) : [];
    return Array.isArray(arr) ? arr.filter((s) => s && typeof s.url === "string" && !s.builtin) : [];
  } catch {
    return [];
  }
}

export function saveServers(list: ServerEntry[]) {
  try {
    storeSet(SERVERS_KEY, JSON.stringify(list.filter((s) => !s.builtin)));
  } catch { /* ignore */ }
}

/** 当前选中的服务器（ServerSelection）："none" 或某个服务器 id。默认官方服务器。 */
export function loadServerSelection(): string {
  try {
    const raw = storeGet(SERVER_SEL_KEY);
    if (raw === SERVER_NONE) return SERVER_NONE;
    if (raw) {
      const all = [...BUILTIN_SERVERS, ...loadServers()];
      if (all.some((s) => s.id === raw)) return raw;
    }
  } catch { /* ignore */ }
  return BUILTIN_SERVERS[0]?.id ?? SERVER_NONE;
}

export function saveServerSelection(sel: string) {
  try {
    storeSet(SERVER_SEL_KEY, sel);
  } catch { /* ignore */ }
}

/** 当前选中服务器的连接地址；无服务器返回 null。 */
export function selectedServerUrl(): string | null {
  const sel = loadServerSelection();
  if (sel === SERVER_NONE) return null;
  return [...BUILTIN_SERVERS, ...loadServers()].find((s) => s.id === sel)?.url ?? null;
}
