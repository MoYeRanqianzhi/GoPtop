/**
 * net/rtc 编解码单元测试 —— offer/answer 的 G1 信令编码（审查 D11：P2P 底层此前零测试）。
 *
 * 覆盖面（纯逻辑，不碰真实网络）：
 * - encodeRtcPayload/decodeRtcPayload：往返一致、压缩生效、跨 pwd 解码必败、
 *   坏前缀/截断必败。
 *
 * 运行：`npm test`（node 24 自带 CompressionStream）。
 */
import { describe, it, expect } from "vitest";
import { encodeRtcPayload, decodeRtcPayload } from "./rtc";

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
