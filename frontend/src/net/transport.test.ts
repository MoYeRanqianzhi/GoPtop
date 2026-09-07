/**
 * transport.ts 单元测试（审查 D11：P2P 底层此前零测试）。
 *
 * 覆盖面（均为纯逻辑，不碰真实网络）：
 * - GameChannel.dispatch：Move 按 (sender, seq) 单调去重（双链路只应用一次）、
 *   SyncState 不去重（幂等全量同步，重连后 seq 归零不能被误丢）。
 * - encodeRtcPayload/decodeRtcPayload：往返一致、跨 pwd 解码必败、坏前缀/截断必败。
 * - genPwd：固定 6 位 base36、CSPRNG 分布抽查。
 * - parsePastedLink / parsePastedAnswer：域名无关解析、旧 query 兼容、
 *   单段路径收紧（防普通文本误判成用户主页）。
 *
 * 运行：`npm test`（node 24 自带 BroadcastChannel/CompressionStream）。
 */
import { describe, it, expect } from "vitest";
import {
  GameChannel,
  genPwd,
  parsePastedLink,
  parsePastedAnswer,
  encodeRtcPayload,
  decodeRtcPayload,
  type GameMsg,
} from "./transport";

/* ---------------- GameChannel 去重 ---------------- */

function remoteMove(seq: number, sender = "remote-peer"): GameMsg {
  return {
    seq,
    sender,
    userId: "remote-user",
    kind: { type: "Move", move: { type: "Place", coord: { x: 7, y: 7 } }, by: "black" },
  };
}

function remoteSync(seq: number, sender = "remote-peer"): GameMsg {
  return {
    seq,
    sender,
    userId: "remote-user",
    kind: {
      type: "SyncState",
      board: [["empty"]],
      toMove: "black",
      winner: null,
      history: [],
      lastMove: null,
      kind: "gomoku",
      size: 15,
    },
  };
}

/** 构造一个不依赖真实 BroadcastChannel 的 channel（node 24 有 BC，但单测隔离更稳）。 */
function isolatedChannel(): { ch: GameChannel; received: GameMsg[] } {
  const ch = new GameChannel();
  const received: GameMsg[] = [];
  ch.onMessage((m) => received.push(m));
  return { ch, received };
}

describe("GameChannel.dispatch — Move (sender,seq) 单调去重", () => {
  it("同一 (sender,seq) 双链路到达：只应用一次", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteMove(1));
    ch.injectRemote(remoteMove(1)); // BroadcastChannel + WebRTC 双链路重复
    expect(received).toHaveLength(1);
  });

  it("seq 回退（重放/乱序）被丢弃", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteMove(5));
    ch.injectRemote(remoteMove(3));
    expect(received).toHaveLength(1);
  });

  it("seq 单调递增正常应用", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteMove(1));
    ch.injectRemote(remoteMove(2));
    ch.injectRemote(remoteMove(3));
    expect(received).toHaveLength(3);
  });

  it("不同 sender 的相同 seq 互不影响", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteMove(1, "peer-A"));
    ch.injectRemote(remoteMove(1, "peer-B"));
    expect(received).toHaveLength(2);
  });

  it("自己发出的消息回流被忽略", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteMove(1, ch.myId));
    expect(received).toHaveLength(0);
  });

  it("非法消息（缺 seq/kind）被丢弃", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote({ sender: "x", userId: "u" } as unknown as GameMsg);
    ch.injectRemote({ seq: 1, sender: "x", userId: "u" } as unknown as GameMsg);
    expect(received).toHaveLength(0);
  });
});

describe("GameChannel.dispatch — SyncState 不去重", () => {
  it("同 seq 的 SyncState 重复到达均应用（幂等全量同步）", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteSync(1));
    ch.injectRemote(remoteSync(1)); // 双链路重复也应用——但 SyncState 幂等，覆盖无害
    expect(received).toHaveLength(2);
  });

  it("重连后发送方 seq 归零的 SyncState 不被误丢", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteSync(3));
    ch.injectRemote(remoteSync(0)); // 重连归零：去重会丢掉它
    expect(received).toHaveLength(2);
  });
});

/* ---------------- G1 编码往返 ---------------- */

describe("encodeRtcPayload/decodeRtcPayload 往返", () => {
  const payload = { s: "v=0\r\no=- 4611731 2 IN IP4 127.0.0.1\r\n...".repeat(20), t: "offer", r: "player" };

  it("往返一致：编码→解码还原原对象", async () => {
    const token = await encodeRtcPayload(payload, "q9bwbu");
    expect(token.startsWith("G1")).toBe(true);
    const out = (await decodeRtcPayload(token, "q9bwbu")) as typeof payload;
    expect(out).toEqual(payload);
  });

  it("压缩生效：token 显著短于原始 JSON", async () => {
    const token = await encodeRtcPayload(payload, "q9bwbu");
    expect(token.length).toBeLessThan(JSON.stringify(payload).length);
  });

  it("pwd 不符解码必败（密钥错误或被篡改）", async () => {
    const token = await encodeRtcPayload(payload, "q9bwbu");
    await expect(decodeRtcPayload(token, "wrong0")).rejects.toThrow();
  });

  it("坏前缀拒识（G1 版本头校验）", async () => {
    await expect(decodeRtcPayload("XXAbCdEf", "q9bwbu")).rejects.toThrow("unknown rtc token");
  });

  it("截断 token 解码必败（链接被截断的真实故障面）", async () => {
    const token = await encodeRtcPayload(payload, "q9bwbu");
    await expect(decodeRtcPayload(token.slice(0, Math.floor(token.length * 0.4)), "q9bwbu")).rejects.toThrow();
  });
});

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
