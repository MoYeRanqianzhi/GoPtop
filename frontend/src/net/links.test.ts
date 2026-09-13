/**
 * net/identity 与 net/links 单元测试（审查 D11：P2P 底层此前零测试）。
 *
 * 覆盖面（纯逻辑，不碰真实网络）：
 * - genPwd：固定 6 位 base36、CSPRNG 分布抽查。
 * - parsePastedLink / parsePastedAnswer：域名无关解析、旧 query 兼容、
 *   单段路径收紧（防普通文本误判成用户主页）。
 *
 * 运行：`npm test`。
 */
import { describe, it, expect } from "vitest";
import { genPwd } from "./identity";
import { parsePastedLink, parsePastedAnswer } from "./links";

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
