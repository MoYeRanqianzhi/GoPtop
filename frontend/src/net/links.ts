/**
 * net/links — URL 模型（路径风格）、链接解析与站内导航。
 *
 * - `/`              菜单页（本地对战 / P2P 对战 / 在线用户 / 设置）
 * - `/local`         本地对战
 * - `/p2p`           P2P 对战大厅
 * - `/users`         在线用户
 * - `/settings`      设置（含 STUN 线路：国服A区/国服B区/外服/自定义）
 * - `/<userId>`      用户主页；邀请链接自动含信令：
 *   `?pwd=&kind=&size=`（同源直传）或 `?pwd=&kind=&size=&rtc=<offer>`（跨设备一键直连）
 * - `/watch/<game>`  观战
 * - 兼容旧链接：`?room=` 视为观战旧房间；`?u=` 视为用户主页；`?watch=` 视为观战。
 *
 * 观战口径：服务器模式观战（spec 链接 `/<userId>?pwd=<specPwd>&spec=1`）已实现，
 * 跨设备可用；无服务器模式 `/watch` 仍限同源（别的设备打开收不到棋局）。
 *
 * - 跨设备信令：用户只传一次邀请链接。offer/answer 编码进邀请 URL 的 `&rtc=` 参数
 *   （同源页面间经 Presence 自动回传）；跨设备时受邀者把回执链接发给邀请者，邀请者在
 *   等待页点「输入回执」粘贴即可——全程弹窗粘贴，不依赖页面导航，Tauri 桌面壳同样可用。
 * - 回执/邀请链接的解析与域名无关：粘贴文本只取路径与查询参数，任意域名都能识别。
 */
import type { GameKind, Size } from "./protocol";

export type UrlIntent =
  | { mode: "menu" }
  | { mode: "local" }
  | { mode: "p2p" }
  | { mode: "users" }
  | { mode: "settings" }
  /** 用户主页/邀请/观战链接。`spec=1` 表示观战（pwd 为观战钥匙，整局有效）；
   *  `rtc` 为无服务器跨设备信令（加密 offer）；服务器模式不带 rtc，pwd 经服务器
   *  交互校验（错误转弹窗询问）。 */
  | { mode: "user"; userId: string; pwd: string | null; kind: GameKind; size: Size; rtc: string | null; spec: boolean }
  | { mode: "watch"; gameId: string };

function kindSizeFromParams(sp: URLSearchParams): { kind: GameKind; size: Size } {
  const kind: GameKind = sp.get("kind") === "go" ? "go" : "gomoku";
  const sizeRaw = Number(sp.get("size"));
  const size = ([9, 13, 15, 19] as number[]).includes(sizeRaw) ? (sizeRaw as Size) : undefined;
  return { kind, size: size ?? (kind === "go" ? 19 : 15) };
}

/** 解析当前 URL 为路由意图（路径优先，旧 query 兼容）。 */
export function parseUrl(): UrlIntent {
  try {
    const url = new URL(window.location.href);
    const sp = url.searchParams;
    const segs = url.pathname.split("/").filter(Boolean);
    if (segs.length === 0) {
      // 旧链接兼容
      const room = sp.get("room");
      if (room) return { mode: "watch", gameId: room };
      const u = sp.get("u");
      if (u) {
        const { kind, size } = kindSizeFromParams(sp);
        return { mode: "user", userId: u, pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc"), spec: sp.get("spec") === "1" };
      }
      const watch = sp.get("watch");
      if (watch) return { mode: "watch", gameId: watch };
      return { mode: "menu" };
    }
    const [first, second] = segs;
    if (first === "local" && segs.length === 1) return { mode: "local" };
    if (first === "p2p" && segs.length === 1) return { mode: "p2p" };
    if (first === "users" && segs.length === 1) return { mode: "users" };
    if (first === "settings" && segs.length === 1) return { mode: "settings" };
    if (first === "watch" && second) return { mode: "watch", gameId: decodeURIComponent(second) };
    if (segs.length === 1) {
      const { kind, size } = kindSizeFromParams(sp);
      return { mode: "user", userId: decodeURIComponent(first), pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc"), spec: sp.get("spec") === "1" };
    }
    return { mode: "menu" };
  } catch {
    return { mode: "menu" };
  }
}

/** 邀请链接：`<分享域名>/<inviterId>?pwd=<pwd>&kind=&size=[&rtc=<inviteOffer>]`。
 *  `rtc` 为邀请者预生成的直连 offer（跨设备一键直连用）；同源页面间不需要它，
 *  邀请者建邀请时后台自动生成、生成后自动补进链接，无需用户手动复制 offer。 */
export function inviteToUrl(inviterId: string, pwd: string, kind: GameKind, size: Size, rtcOffer?: string | null): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(inviterId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("kind", kind);
  url.searchParams.set("size", String(size));
  if (rtcOffer) url.searchParams.set("rtc", rtcOffer);
  return url.toString();
}

/** 回执链接：受邀者把自动生成的 answer 编进 URL 发回邀请者，邀请者在等待页点「输入回执」
 *  粘贴即可完成直连（不再依赖邀请者用浏览器打开链接——Tauri 里没有地址栏可粘贴）。
 *  同源页面间 answer 经 Presence 自动回传，此链接仅跨设备时需要复制一次。
 *  game/kind/size 一并编入：邀请者凭回执即可加入受邀者的 game channel 并对齐规则。 */
export function answerToUrl(inviterId: string, pwd: string, rtcAns: string, gameId?: string | null, kind?: GameKind | null, size?: Size | null): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(inviterId)}`;
  url.searchParams.set("pwd", pwd);
  url.searchParams.set("rtcAns", rtcAns);
  if (gameId) url.searchParams.set("game", gameId);
  if (kind) url.searchParams.set("kind", kind);
  if (size) url.searchParams.set("size", String(size));
  return url.toString();
}

/** 用户主页链接：`<分享域名>/<userId>`（无 pwd，只看到主页、可手动挑战）。 */
export function userToUrl(userId: string): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(userId)}`;
  return url.toString();
}

/** 观战链接：`<分享域名>/<userId>?pwd=<specPwd>&spec=1`（观战钥匙整局有效，
 *  服务器模式点开即连房主；pwd 错/无 → 房主聊天区私有申请）。 */
export function specLinkUrl(userId: string, specPwd: string): string {
  const url = new URL(shareOrigin());
  url.pathname = `/${encodeURIComponent(userId)}`;
  url.searchParams.set("pwd", specPwd);
  url.searchParams.set("spec", "1");
  return url.toString();
}

/** 观战链接：`<分享域名>/watch/<gameId>`。 */
export function watchToUrl(gameId: string): string {
  const url = new URL(shareOrigin());
  url.pathname = `/watch/${encodeURIComponent(gameId)}`;
  return url.toString();
}

/** 回菜单页。 */
export function homeUrl(): string {
  return `${window.location.origin}/`;
}

/* ---------------- 分享基地址（自动决定，用户不可见不可设） ----------------
 *
 * 分享类链接（邀请/主页/观战/回执）发给对方后要在浏览器打开，所以基地址必须是
 * 公网可访问的站点。两处自动决定，无需任何用户设置：
 * - Web（http/https）：自动用当前站点 origin；
 * - 非 Web（Tauri 桌面壳等）：自动用云端部署地址 SHARE_ORIGIN_NATIVE。
 * 换部署域名 = 改 SHARE_ORIGIN_NATIVE 一处常量，全局生效。
 */

/** 判断当前是否运行在 Tauri 桌面壳（非 Web）。 */
export function isTauri(): boolean {
  try {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  } catch {
    return false;
  }
}

/** 非 Web 环境分享链接的基地址（云端部署地址）：换域名只改这一处。 */
export const SHARE_ORIGIN_NATIVE = "https://goptop.pages.dev";

/** 分享基地址（自动）：Web 用当前站点；Tauri/非 Web 用 SHARE_ORIGIN_NATIVE。 */
export function shareOrigin(): string {
  return isTauri() ? SHARE_ORIGIN_NATIVE : window.location.origin;
}

/** 从粘贴的任意 URL 中提取站内意图（路径 + 查询参数），与域名无关。
 *  兼容旧 query 风格（`?u=` `?room=` `?watch=`）与路径风格（`/<userId>` `/watch/<id>`）。
 *  返回 null 表示这段文本不是可识别的 GoPtop 链接。 */
export function parsePastedLink(text: string): UrlIntent | null {
  try {
    const raw = text.trim();
    if (!raw) return null;
    // 有协议就当 URL 解析；无协议（如 "goptop.pages.dev/u-1a2b?pwd=xx"）补上再解析
    const withProto = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw) ? raw : `https://${raw}`;
    const url = new URL(withProto);
    const sp = url.searchParams;
    const segs = url.pathname.split("/").filter(Boolean);
    // 旧 query 风格优先（?u= / ?room= / ?watch=）
    const room = sp.get("room");
    if (room) return { mode: "watch", gameId: room };
    const u = sp.get("u");
    if (u) {
      const { kind, size } = kindSizeFromParams(sp);
      // searchParams.get 已解码一次；再 decode 会在 ID 含 % 时抛 URIError 被外层吞成「无法识别」
      return { mode: "user", userId: u, pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc"), spec: sp.get("spec") === "1" };
    }
    const watch = sp.get("watch");
    if (watch) return { mode: "watch", gameId: watch };
    // 路径风格
    const [first, second] = segs;
    if (first === "local" && segs.length === 1) return { mode: "local" };
    if (first === "p2p" && segs.length === 1) return { mode: "p2p" };
    if (first === "users" && segs.length === 1) return { mode: "users" };
    if (first === "settings" && segs.length === 1) return { mode: "settings" };
    if (first === "watch" && second) return { mode: "watch", gameId: decodeURIComponent(second) };
    if (segs.length === 1) {
      // 收紧识别：单段路径必须像用户链接（用户 ID 均为 u- 前缀，或带 pwd/rtc 邀请参数），
      // 否则视为普通文本/陌生网址——避免粘贴任意内容被误判成"用户主页"发起挑战。
      if (!first.startsWith("u-") && !sp.has("pwd") && !sp.has("rtc")) return null;
      const { kind, size } = kindSizeFromParams(sp);
      return { mode: "user", userId: decodeURIComponent(first), pwd: sp.get("pwd"), kind, size, rtc: sp.get("rtc"), spec: sp.get("spec") === "1" };
    }
    return null;
  } catch {
    return null;
  }
}

/** 从粘贴的任意 URL 中提取邀请者回执参数（inviterId + pwd + rtcAns + game/kind/size），与域名无关。 */
export function parsePastedAnswer(text: string): { inviterId: string; pwd: string; rtcAns: string; spectator: boolean; gameId: string | null; kind: GameKind | null; size: Size | null } | null {
  try {
    const raw = text.trim();
    if (!raw) return null;
    const withProto = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw) ? raw : `https://${raw}`;
    const url = new URL(withProto);
    const rtcAns = url.searchParams.get("rtcAns");
    if (!rtcAns) return null;
    const segs = url.pathname.split("/").filter(Boolean);
    if (segs.length !== 1) return null;
    const kindRaw = url.searchParams.get("kind");
    const sizeRaw = Number(url.searchParams.get("size"));
    return {
      inviterId: decodeURIComponent(segs[0]),
      pwd: url.searchParams.get("pwd") ?? "",
      rtcAns,
      // spec=1 标记观战回执（回执类型由链接属性自动判断，用户拍板）；对局回执不带
      spectator: url.searchParams.get("spec") === "1",
      gameId: url.searchParams.get("game"),
      kind: kindRaw === "go" || kindRaw === "gomoku" ? kindRaw : null,
      size: ([9, 13, 15, 19] as number[]).includes(sizeRaw) ? (sizeRaw as Size) : null,
    };
  } catch {
    return null;
  }
}

/** 站内导航（SPA，不刷新）。 */
export function nav(path: string) {
  window.history.pushState(null, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}
