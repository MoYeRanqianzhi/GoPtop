/**
 * net/stun — STUN 线路（仅 NAT 地址发现，不转发数据）。
 *
 * 免费公共发现服务商池（2026-09 三地实测存活）：部分默认启用、其余默认关闭，
 * 用户可在设置页逐条开关或添加自定义线路，保存在 localStorage。
 */

export type StunLine = { id: string; label: string; urls: string; builtin: boolean; enabled: boolean };

export const BUILTIN_STUN: Omit<StunLine, "enabled">[] = [
  { id: "miwifi", label: "小米", urls: "stun:stun.miwifi.com:3478", builtin: true },
  { id: "bilibili", label: "哔哩哔哩", urls: "stun:stun.chat.bilibili.com:3478", builtin: true },
  { id: "cloudflare", label: "Cloudflare", urls: "stun:stun.cloudflare.com:3478", builtin: true },
  { id: "google", label: "Google", urls: "stun:stun.l.google.com:19302", builtin: true },
  { id: "twilio", label: "Twilio", urls: "stun:global.stun.twilio.com:3478", builtin: true },
  { id: "nextcloud", label: "Nextcloud", urls: "stun:stun.nextcloud.com:443", builtin: true },
  { id: "sipnet", label: "Sipnet", urls: "stun:stun.sipnet.ru:3478", builtin: true },
  { id: "cope", label: "Cope", urls: "stun:stun.cope.es:3478", builtin: true },
  { id: "wtfismyip", label: "wtfismyip", urls: "stun:stun.wtfismyip.com:3478", builtin: true },
];

/** 默认启用的线路：其余内置项默认关闭，用户可随时打开。 */
const STUN_DEFAULT_ON = new Set(["miwifi", "bilibili", "cloudflare"]);

const STUN_KEY = "goptop:stun";

function defaultStunLines(): StunLine[] {
  return BUILTIN_STUN.map((b) => ({ ...b, enabled: STUN_DEFAULT_ON.has(b.id) }));
}

/** 当前启用的 STUN 线路（含用户自定义）。 */
export function loadStunLines(): StunLine[] {
  try {
    const raw = localStorage.getItem(STUN_KEY);
    if (!raw) return defaultStunLines();
    const arr = JSON.parse(raw) as StunLine[];
    if (!Array.isArray(arr) || arr.length === 0) return defaultStunLines();
    // 与内置线路合并：保留用户新增，补齐新增内置项；旧 id（cn-a/cn-b/foreign）映射到新 id
    const LEGACY: Record<string, string> = { "cn-a": "miwifi", "cn-b": "bilibili", foreign: "google" };
    const byId = new Map(
      arr
        .filter((l) => l && typeof l.urls === "string")
        .map((l) => ({ ...l, id: LEGACY[l.id] ?? l.id }))
        .map((l) => [l.id, l]),
    );
    for (const b of BUILTIN_STUN) {
      if (!byId.has(b.id)) byId.set(b.id, { ...b, enabled: STUN_DEFAULT_ON.has(b.id) });
    }
    return [...byId.values()];
  } catch {
    return defaultStunLines();
  }
}

export function saveStunLines(lines: StunLine[]) {
  try {
    localStorage.setItem(STUN_KEY, JSON.stringify(lines));
  } catch { /* ignore */ }
}

/** 组装 RTCPeerConnection 的 iceServers（只用启用的线路；全关则走纯 host 候选局域网直连）。 */
export function stunServers(): { urls: string }[] {
  return loadStunLines().filter((l) => l.enabled).map((l) => ({ urls: l.urls }));
}
