/**
 * net/identity 与 net/links 单元测试（审查 D11：P2P 底层此前零测试）。
 *
 * 覆盖面（纯逻辑，不碰真实网络）：
 * - genPwd：固定 6 位 base36、CSPRNG 分布抽查。
 * - parsePastedLink / parsePastedAnswer：域名无关解析、旧 query 兼容、
 *   单段路径收紧（防普通文本误判成用户主页）。
 * - parseUrl（审查 #6 C1）：路径路由（node 环境无 jsdom，stub window.location）。
 * - 五个链接构造函数（审查 #6 C2）：构造 → 解析往返等值。
 *
 * 运行：`npm test`。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { genPwd } from "./identity";
import { parseUrl, parsePastedLink, parsePastedAnswer, inviteToUrl, answerToUrl, userToUrl, specLinkUrl, watchToUrl, shareOrigin, SHARE_ORIGIN_NATIVE } from "./links";

describe("genPwd", () => {
  it("固定 6 位 base36", () => {
    for (let i = 0; i < 50; i++) {
      const p = genPwd();
      expect(p).toMatch(/^[0-9a-z]{6}$/);
    }
  });

  it("50 次无重复（熵抽查）", () => {
    const set = new Set<string>();
    for (let i = 0; i < 50; i++) set.add(genPwd());
    expect(set.size).toBeGreaterThan(45);
  });
});

/* ---------------- 链接解析 ---------------- */

describe("parsePastedLink", () => {
  it("完整邀请链接（含 rtc）域名无关解析", () => {
    const r = parsePastedLink("https://goptop.pages.dev/u-abc123?pwd=k9d2x1&kind=go&size=13&rtc=G1AbCd");
    expect(r).toMatchObject({ mode: "user", userId: "u-abc123", pwd: "k9d2x1", kind: "go", size: 13, rtc: "G1AbCd" });
  });

  it("无协议补 https", () => {
    const r = parsePastedLink("goptop.pages.dev/u-abc123?pwd=xyz789&kind=gomoku&size=15");
    expect(r).toMatchObject({ mode: "user", userId: "u-abc123", pwd: "xyz789" });
  });

  it("旧 query 风格 ?u=", () => {
    const r = parsePastedLink("https://x.dev/?u=u-abc&pwd=p1w2e3&kind=go&size=9");
    expect(r).toMatchObject({ mode: "user", userId: "u-abc", pwd: "p1w2e3", kind: "go", size: 9 });
  });

  it("?u= 只解码一次：ID 含 % 不抛 URIError（审查 #3 A4 双解码回归）", () => {
    // get("u") 已解码一次得到 "u-a%b"；若再 decode 会抛 URIError 被吞成 null
    const r = parsePastedLink("https://x.dev/?u=u-a%25b&pwd=p1");
    expect(r).toMatchObject({ mode: "user", userId: "u-a%b", pwd: "p1" });
  });

  it("旧 query 风格 ?room= / ?watch=", () => {
    expect(parsePastedLink("https://x.dev/?room=g-1")).toMatchObject({ mode: "watch", gameId: "g-1" });
    expect(parsePastedLink("https://x.dev/?watch=g-2")).toMatchObject({ mode: "watch", gameId: "g-2" });
  });

  it("路径风格 /watch/<id>", () => {
    expect(parsePastedLink("https://x.dev/watch/g-42")).toMatchObject({ mode: "watch", gameId: "g-42" });
  });

  it("单段路径无 u- 前缀且无 pwd/rtc → 拒识（审查 D4 收紧）", () => {
    expect(parsePastedLink("https://x.dev/hello")).toBeNull();
    expect(parsePastedLink("随便一段中文")).toBeNull();
    expect(parsePastedLink("192.168.1.5:8080")).toBeNull();
  });

  it("单段路径带 pwd 即识别", () => {
    const r = parsePastedLink("https://x.dev/u-xyz?pwd=abc123&kind=gomoku&size=15");
    expect(r).toMatchObject({ mode: "user", userId: "u-xyz" });
  });

  it("未知 kind 回退 gomoku、非法 size 回退默认", () => {
    const r = parsePastedLink("https://x.dev/u-a?pwd=q1w2e3&kind=chess&size=7");
    expect(r).toMatchObject({ kind: "gomoku", size: 15 });
  });
});

describe("parsePastedAnswer", () => {
  const ANSWER = "https://x.dev/u-inviter01?pwd=q9bwbu&rtcAns=G1AnsToken&game=g-99&kind=go&size=13";

  it("完整回执：inviterId/pwd/rtcAns/game/kind/size", () => {
    expect(parsePastedAnswer(ANSWER)).toEqual({
      inviterId: "u-inviter01",
      pwd: "q9bwbu",
      rtcAns: "G1AnsToken",
      spectator: false,
      gameId: "g-99",
      kind: "go",
      size: 13,
    });
  });

  it("spec=1 识别为观战回执", () => {
    const r = parsePastedAnswer("https://x.dev/u-h?pwd=a1b2c3&rtcAns=G1X&game=g-1&kind=gomoku&size=15&spec=1");
    expect(r?.spectator).toBe(true);
  });

  it("对局回执 spectator=false", () => {
    const r = parsePastedAnswer(ANSWER);
    expect(r?.spectator).toBe(false);
  });

  it("无 rtcAns 拒识", () => {
    expect(parsePastedAnswer("https://x.dev/u-h?pwd=x&game=g")).toBeNull();
  });

  it("多段路径拒识", () => {
    expect(parsePastedAnswer("https://x.dev/watch/g-1?rtcAns=G1X")).toBeNull();
  });

  it("裸文本拒识", () => {
    expect(parsePastedAnswer("这不是链接")).toBeNull();
    expect(parsePastedAnswer("")).toBeNull();
  });

  it("非法 kind/size 置 null（由上层校验）", () => {
    const r = parsePastedAnswer("https://x.dev/u-h?pwd=x&rtcAns=G1X&kind=xxx&size=0");
    expect(r?.kind).toBeNull();
    expect(r?.size).toBeNull();
  });
});

/* ---------------- parseUrl：路径路由（审查 #6 C1） ---------------- */

describe("parseUrl", () => {
  afterEach(() => vi.unstubAllGlobals());

  /** node 环境无 jsdom：把待测地址注入 window.location.href 再解析。 */
  function at(href: string) {
    vi.stubGlobal("window", { location: { href, origin: "https://x.dev" } });
    return parseUrl();
  }

  it("根路径 → menu", () => {
    expect(at("https://x.dev/")).toEqual({ mode: "menu" });
    expect(at("https://x.dev")).toEqual({ mode: "menu" });
  });

  it("四个静态页面各自 mode", () => {
    expect(at("https://x.dev/local")).toEqual({ mode: "local" });
    expect(at("https://x.dev/p2p")).toEqual({ mode: "p2p" });
    expect(at("https://x.dev/users")).toEqual({ mode: "users" });
    expect(at("https://x.dev/settings")).toEqual({ mode: "settings" });
  });

  it("/watch/<id> → watch", () => {
    expect(at("https://x.dev/watch/g-1")).toEqual({ mode: "watch", gameId: "g-1" });
  });

  it("邀请路径 → user 全字段", () => {
    expect(at("https://x.dev/u-abc?pwd=k&kind=go&size=13&rtc=G1X")).toEqual({
      mode: "user",
      userId: "u-abc",
      pwd: "k",
      kind: "go",
      size: 13,
      rtc: "G1X",
      spec: false,
    });
  });

  it("spec=1 → 观战意图（pwd 为观战钥匙）", () => {
    expect(at("https://x.dev/u-abc?pwd=spec123&spec=1")).toEqual({
      mode: "user",
      userId: "u-abc",
      pwd: "spec123",
      kind: "gomoku",
      size: 15,
      rtc: null,
      spec: true,
    });
  });

  it("旧 query 兼容 ?u= / ?room= / ?watch=", () => {
    expect(at("https://x.dev/?u=u-abc&pwd=k&kind=go&size=9")).toMatchObject({
      mode: "user",
      userId: "u-abc",
      pwd: "k",
      kind: "go",
      size: 9,
    });
    expect(at("https://x.dev/?room=g-1")).toEqual({ mode: "watch", gameId: "g-1" });
    expect(at("https://x.dev/?watch=g-2")).toEqual({ mode: "watch", gameId: "g-2" });
  });

  it("多段路径 → menu（含保留字后缀段，不误路由）", () => {
    expect(at("https://x.dev/a/b")).toEqual({ mode: "menu" });
    expect(at("https://x.dev/local/extra")).toEqual({ mode: "menu" });
  });

  it("路径含畸形 % 不抛：decodeURIComponent 的 URIError 被吞 → menu", () => {
    expect(at("https://x.dev/u-a%zz?pwd=k")).toEqual({ mode: "menu" });
  });

  it("路径 userId 百分号解码", () => {
    expect(at("https://x.dev/u-a%20b%2Fc")).toMatchObject({ mode: "user", userId: "u-a b/c" });
  });
});

/* ---------------- 链接构造 → 解析往返（审查 #6 C2） ---------------- */

describe("链接构造 → 解析往返", () => {
  beforeEach(() => {
    // shareOrigin() 非 Tauri 下取 window.location.origin。
    vi.stubGlobal("window", { location: { href: "https://x.dev/", origin: "https://x.dev" } });
  });
  afterEach(() => vi.unstubAllGlobals());

  it("inviteToUrl 含 rtc：全字段往返等值", () => {
    const url = inviteToUrl("u-abc", "k9d2x1", "go", 13, "G1X");
    expect(url.startsWith("https://x.dev/u-abc?")).toBe(true);
    expect(parsePastedLink(url)).toEqual({
      mode: "user",
      userId: "u-abc",
      pwd: "k9d2x1",
      kind: "go",
      size: 13,
      rtc: "G1X",
      spec: false,
    });
  });

  it("inviteToUrl 不带 rtc：解析出 rtc=null", () => {
    const url = inviteToUrl("u-abc", "k9d2x1", "gomoku", 15, null);
    expect(url).not.toContain("rtc=");
    expect(parsePastedLink(url)).toMatchObject({ mode: "user", userId: "u-abc", rtc: null, spec: false });
  });

  it("specLinkUrl 带 spec=1", () => {
    const url = specLinkUrl("u-host", "specpwd1");
    expect(url).toContain("spec=1");
    expect(parsePastedLink(url)).toMatchObject({
      mode: "user",
      userId: "u-host",
      pwd: "specpwd1",
      spec: true,
    });
  });

  it("watchToUrl → watch 往返", () => {
    expect(parsePastedLink(watchToUrl("g-42"))).toEqual({ mode: "watch", gameId: "g-42" });
  });

  it("userToUrl：u- 前缀无 pwd 也识别为用户主页", () => {
    expect(parsePastedLink(userToUrl("u-plain"))).toMatchObject({
      mode: "user",
      userId: "u-plain",
      pwd: null,
      spec: false,
    });
  });

  it("answerToUrl → parsePastedAnswer 往返（含 game/kind/size）", () => {
    const url = answerToUrl("u-inviter", "q9bwbu", "G1Ans", "g-99", "go", 13);
    expect(parsePastedAnswer(url)).toEqual({
      inviterId: "u-inviter",
      pwd: "q9bwbu",
      rtcAns: "G1Ans",
      spectator: false,
      gameId: "g-99",
      kind: "go",
      size: 13,
    });
  });

  it("userId 特殊字符：encodeURIComponent 编入、解码往返等值", () => {
    for (const id of ["u-a b/c?", "u-中文", "u-100%安全", "u-a&b=c"]) {
      const url = inviteToUrl(id, "p1w2e3", "gomoku", 15);
      expect(url).not.toContain(id); // 确已编码，未裸拼进 URL
      expect(parsePastedLink(url)).toMatchObject({ mode: "user", userId: id });
    }
  });
});

/* ---------------- 分享基地址的运行时判定（2026-09-18 鸿蒙实机测试回归） ----------------
 *
 * 鸿蒙 ArkWeb 壳不是 Tauri（没有 __TAURI_INTERNALS__），页面宿主是虚拟域名
 * appassets.goptop；若按「非 Tauri 就用当前 origin」处理，会生成
 * `https://appassets.goptop/u-xxx?pwd=` 这类**对方设备根本打不开**的邀请链接。
 */
describe("shareOrigin 运行时判定", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("普通 Web：用当前站点 origin", () => {
    vi.stubGlobal("window", { location: { href: "https://my.site/", origin: "https://my.site", hostname: "my.site" } });
    expect(shareOrigin()).toBe("https://my.site");
  });

  it("Tauri 壳（有 __TAURI_INTERNALS__）：用云端部署地址", () => {
    vi.stubGlobal("window", {
      __TAURI_INTERNALS__: {},
      location: { href: "http://tauri.localhost/", origin: "http://tauri.localhost", hostname: "tauri.localhost" },
    });
    expect(shareOrigin()).toBe(SHARE_ORIGIN_NATIVE);
  });

  it("鸿蒙 ArkWeb 壳（虚拟域名、无 Tauri 注入）：用云端部署地址", () => {
    vi.stubGlobal("window", {
      location: { href: "https://appassets.goptop/", origin: "https://appassets.goptop", hostname: "appassets.goptop" },
    });
    expect(shareOrigin()).toBe(SHARE_ORIGIN_NATIVE);
  });
});
