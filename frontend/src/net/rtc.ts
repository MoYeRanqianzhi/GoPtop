/**
 * net/rtc — 跨设备直连（WebRTC DataChannel，STUN-only）。
 *
 * 直连说明（默认直连优先）：
 * - 同源页面间：`BroadcastChannel` 为同源端到端直传（浏览器内共享内存，不经过任何服务器/中转）。
 * - 跨设备：`RTCPeerConnection` 仅配 STUN（NAT 地址发现，不转发数据），**不配任何 TURN**，
 *   数据只走主机候选（host/srflx）直连。
 * - 服务器模式（用户可选启用）另有 `relay` 兜底中转：仅当 P2P 未建立时中转对局消息，
 *   直连建立后仍以直连为准（双发按 sender+seq 去重）。
 *
 * offer/answer 都是全量 gathering 后才交换（见 `waitGathering`），无需 trickle ICE。
 */
import type { GameMsg } from "./protocol";
import { stunServers } from "./stun";

/* offer/answer 的 URL 编码：压缩 + pwd 派生 XOR + URL 安全 base64。
 * - 压缩：`CompressionStream("deflate-raw")`，SDP 是高度重复的文本，压缩比可观，
 *   链接显著变短。兼容面：Chromium 103+ / Firefox 113+ / Safari 16.4+ / Tauri
 *   WebView2（Chromium）均可用；**无降级路径**——极旧内核会在 encode/decode 抛错，
 *   上层以「直连建立失败」提示（不误报为线路问题）。
 * - 加密：与本局钥匙 pwd 派生的密钥流逐字节 XOR——链接里不再出现可读 SDP
 *   （SDP 含本机 IP 候选）。这是混淆级而非密码学级（pwd 本就在同一链接里）；
 *   真正的传输安全由 WebRTC 自带的 DTLS 端到端加密保证，此层只为不裸奔。
 * - base64：URL 安全字母表 + 去填充，`%` 编码后无 `%2B` `%2F` `%3D` 转义。 */

const RTC_ENC_MAGIC = "G1"; // 版本头：未来换编码格式时可平滑迁移

/** deflate-raw 压缩。 */
async function deflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const cs = new CompressionStream("deflate-raw");
  const writer = cs.writable.getWriter();
  // write/close 的失败已由 arrayBuffer() 的 reject 上报；不 catch 会变成
  // unhandled rejection（截断 token 的解码即触发，console 出现游离报错）
  writer.write(bytes).catch(() => { /* 由 readable 侧上报 */ });
  writer.close().catch(() => { /* 由 readable 侧上报 */ });
  const buf = await new Response(cs.readable).arrayBuffer();
  return new Uint8Array(buf);
}

/** deflate-raw 解压。 */
async function inflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const ds = new DecompressionStream("deflate-raw");
  const writer = ds.writable.getWriter();
  writer.write(bytes).catch(() => { /* 由 readable 侧上报 */ });
  writer.close().catch(() => { /* 由 readable 侧上报 */ });
  const buf = await new Response(ds.readable).arrayBuffer();
  return new Uint8Array(buf);
}

/** 由短钥匙派生重复密钥流：fnv1a 双散列扩展成 4 字节步进，避免同钥周期性。 */
function keyStream(pwd: string, len: number): Uint8Array {
  const enc = new TextEncoder();
  const p = enc.encode(pwd);
  const out = new Uint8Array(len);
  let h1 = 0x811c9dc5, h2 = 0x1b873593;
  for (const b of p) {
    h1 = Math.imul(h1 ^ b, 0x01000193) >>> 0;
    h2 = Math.imul(h2 + b, 0x85ebca6b) >>> 0;
  }
  for (let i = 0; i < len; i++) {
    h1 = (Math.imul(h1, 0x01000193) ^ (h1 >>> 15)) >>> 0;
    h2 = (Math.imul(h2, 0x85ebca6b) ^ (h2 >>> 13)) >>> 0;
    out[i] = (h1 ^ h2) & 0xff;
  }
  return out;
}

/** XOR 加密/解密（同一函数）。 */
function xorBytes(bytes: Uint8Array, pwd: string): Uint8Array {
  const ks = keyStream(pwd, bytes.length);
  const out = new Uint8Array(bytes.length);
  for (let i = 0; i < bytes.length; i++) out[i] = bytes[i] ^ ks[i];
  return out;
}

/** URL 安全 base64（无填充）。 */
function b64urlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** URL 安全 base64 解码。 */
function b64urlDecode(s: string): Uint8Array {
  const t = s.replace(/-/g, "+").replace(/_/g, "/");
  const pad = t.length % 4 === 0 ? "" : "=".repeat(4 - (t.length % 4));
  const bin = atob(t + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** 编码 offer/answer：JSON 短键 → deflate → pwd XOR → URL 安全 base64。
 *  `pwd` 为本局钥匙（编解码两端必须一致）。 */
export async function encodeRtcPayload(payload: unknown, pwd: string): Promise<string> {
  const json = JSON.stringify(payload);
  const raw = new TextEncoder().encode(json);
  const deflated = await deflateRaw(raw);
  const encrypted = xorBytes(deflated, pwd);
  return RTC_ENC_MAGIC + b64urlEncode(encrypted);
}

/** 解码 offer/answer（`encodeRtcPayload` 的逆）。 */
export async function decodeRtcPayload(token: string, pwd: string): Promise<unknown> {
  const token2 = token.trim();
  if (!token2.startsWith(RTC_ENC_MAGIC)) throw new Error("unknown rtc token");
  const encrypted = b64urlDecode(token2.slice(RTC_ENC_MAGIC.length));
  const deflated = xorBytes(encrypted, pwd);
  const raw = await inflateRaw(deflated);
  return JSON.parse(new TextDecoder().decode(raw));
}

export type RtcState = "idle" | "making-invite" | "waiting-invitee" | "joining" | "open" | "closed" | "error";

/**
 * URL 邀请式点对点直连。约束：
 * - `iceServers` 仅用户启用的 STUN 线路（默认国服 A/B 区；外服默认关闭；可自定义），
 *   只做 NAT 地址发现、不转发数据；**不配置任何 TURN**。
 * - 服务器模式另有 relay 兜底中转（用户选择连接服务器时启用，仅 P2P 未建立时
 *   中转对局消息，直连优先）；本层建连本身不经过任何中转。
 * - 信令随邀请走：邀请者 offer 压缩混淆后编进邀请 URL `&rtc=`；受邀者 answer 经同源
 *   Presence 自动回传，跨设备时编进回执链接 `?rtcAns=` 由邀请者弹窗粘贴完成。
 *   编码见 `encodeRtcPayload`（deflate 压缩 + pwd XOR + URL 安全 base64）。
 * - 建连后 DataChannel 标签 `goptop` 直接传 `GameMsg` JSON；观战者同样以独立连接接入邀请者广播。
 */
export class DirectRtcPeer {
  state: RtcState = "idle";
  isInviter: boolean;
  role: "player" | "spectator";
  /** 对端标识（服务器模式 = 对端 s- 短 ID）：多连接（对手+多名观战者）时
   *  邀请者凭它把 answer 路由到正确的 peer；无服务器模式不使用。 */
  peerTag: string | null = null;
  onRemote: ((msg: GameMsg) => void) | null = null;
  onState: ((s: RtcState) => void) | null = null;
  private pc: RTCPeerConnection | null = null;
  private dc: RTCDataChannel | null = null;
  /** answer 是否已成功应用（setRemoteDescription 成功后才置位）。
   *  回执可能经弹窗与 Presence 双路径到达，本位防重复应用；App 层也读它判断
   *  「失败是否真的失败」（另一路径成功时 catch 不算失败）。 */
  answered = false;
  private static all = new Set<DirectRtcPeer>();

  constructor(opts: { isInviter: boolean; role: "player" | "spectator" }) {
    this.isInviter = opts.isInviter;
    this.role = opts.role;
  }

  private setState(s: RtcState) {
    this.state = s;
    try { this.onState?.(s); } catch { /* ignore */ }
  }

  private makePc(): RTCPeerConnection {
    const pc = new RTCPeerConnection({
      // 仅用户启用的 STUN 线路：解析自身公网地址以便直连；数据不经任何服务器中转。
      iceServers: stunServers(),
    });
    pc.ondatachannel = (ev) => {
      this.attachChannel(ev.channel);
    };
    pc.onconnectionstatechange = () => {
      const st = pc.connectionState;
      if (st === "connected") this.setState("open");
      // disconnected 常可自愈（网络抖动、ICE 切换候选），不判死；真断会转 failed
      else if (st === "failed") this.setState("error");
      else if (st === "closed") this.setState("closed");
    };
    this.pc = pc;
    return pc;
  }

  private attachChannel(ch: RTCDataChannel) {
    this.dc = ch;
    ch.onopen = () => this.setState("open");
    ch.onclose = () => this.setState("closed");
    ch.onerror = () => this.setState("error");
    ch.onmessage = (ev) => {
      try {
        const msg = JSON.parse(String(ev.data)) as GameMsg;
        this.onRemote?.(msg);
      } catch { /* ignore */ }
    };
  }

  private waitGathering(pc: RTCPeerConnection, timeoutMs = 8000): Promise<void> {
    if (pc.iceGatheringState === "complete") return Promise.resolve();
    return new Promise((resolve) => {
      let done = false;
      const finish = () => {
        if (done) return;
        done = true;
        resolve();
      };
      const timer = window.setTimeout(finish, timeoutMs);
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === "complete") {
          window.clearTimeout(timer);
          finish();
        }
      };
      pc.onicecandidate = (ev) => {
        if (!ev.candidate) {
          window.clearTimeout(timer);
          finish();
        }
      };
    });
  }

  /** 邀请者：预生成邀请用 offer（role 固定 player），编进邀请 URL 的 `&rtc=` 参数。
   *  `pwd` 为本局钥匙，同时充当信令编码密钥（两端一致即可解码）。 */
  async createOffer(pwd: string): Promise<string> {
    const desc = await this.prepareOffer();
    return encodeRtcPayload({ s: desc.sdp, t: desc.type, r: this.role }, pwd);
  }

  /** 邀请者（服务器模式）：offer 以明文 JSON 走 WSS（传输层已有 TLS，pwd 只在服务器侧校验）。 */
  async createOfferPlain(): Promise<string> {
    const desc = await this.prepareOffer();
    return JSON.stringify({ s: desc.sdp, t: desc.type, r: this.role });
  }

  private async prepareOffer(): Promise<RTCSessionDescription> {
    this.close();
    this.setState("making-invite");
    const pc = this.makePc();
    const ch = pc.createDataChannel("goptop");
    this.attachChannel(ch);
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await this.waitGathering(pc);
    const desc = pc.localDescription!;
    DirectRtcPeer.all.add(this);
    this.setState("waiting-invitee");
    return desc;
  }

  /** 受邀者：用邀请 URL 的 offer 生成 answer 返回（随后自动回传邀请者，无需用户操作）。 */
  async acceptOffer(token: string, pwd: string): Promise<string> {
    const payload = await decodeRtcPayload(token, pwd) as { s: string; t: RTCSdpType; r?: string };
    const desc = await this.acceptOfferInner(payload);
    return encodeRtcPayload({ s: desc.sdp, t: desc.type, r: this.role }, pwd);
  }

  /** 受邀者（服务器模式）：offer/answer 均为明文 JSON（WSS 已加密，pwd 只在服务器侧校验）。 */
  async acceptOfferPlain(payloadJson: string): Promise<string> {
    const desc = await this.acceptOfferInner(JSON.parse(payloadJson));
    return JSON.stringify({ s: desc.sdp, t: desc.type, r: this.role });
  }

  private async acceptOfferInner(payload: { s: string; t: RTCSdpType; r?: string }): Promise<RTCSessionDescription> {
    this.close();
    this.setState("joining");
    const pc = this.makePc();
    if (payload.r === "spectator") this.role = "spectator";
    await pc.setRemoteDescription({ type: payload.t, sdp: payload.s });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    await this.waitGathering(pc);
    DirectRtcPeer.all.add(this);
    return pc.localDescription!;
  }

  /** 邀请者：用受邀者回传的 answer 完成直连（同源经 Presence 自动回传；跨设备由弹窗粘贴回执触发）。
   *  幂等位在「应用成功」后才置位：坏回执解码失败不锁死，邀请者可再贴正确回执。 */
  async acceptAnswer(token: string, pwd: string): Promise<void> {
    const payload = await decodeRtcPayload(token, pwd) as { s: string; t: RTCSdpType };
    await this.acceptAnswerPayload(payload);
  }

  /** 邀请者（服务器模式）：answer 为明文 JSON。 */
  async acceptAnswerPlain(payloadJson: string): Promise<void> {
    const payload = JSON.parse(payloadJson) as { s: string; t: RTCSdpType };
    await this.acceptAnswerPayload(payload);
  }

  private async acceptAnswerPayload(payload: { s: string; t: RTCSdpType }): Promise<void> {
    if (!this.pc) throw new Error("no pending offer");
    if (this.answered) return;
    try {
      await this.pc.setRemoteDescription({ type: payload.t, sdp: payload.s });
      this.answered = true;
    } catch (err) {
      // 双路径竞态：另一路径（Presence/弹窗）可能已成功应用同一 answer
      if (this.answered || this.pc.remoteDescription) return;
      throw err;
    }
  }

  send(msg: GameMsg) {
    try {
      if (this.dc && this.dc.readyState === "open") {
        this.dc.send(JSON.stringify(msg));
      }
    } catch { /* ignore */ }
  }

  close() {
    DirectRtcPeer.all.delete(this);
    try { this.dc?.close(); } catch { /* ignore */ }
    try { this.pc?.close(); } catch { /* ignore */ }
    this.dc = null;
    this.pc = null;
    this.answered = false;
    if (this.state !== "idle") this.setState("closed");
  }
}

/** 邀请者侧广播辅助：向所有 open 的直连发送（选手+观战）。
 *  App 启动时经 `wireRtcBroadcast` 注入实现；GameChannel.send 单入口调用。
 *  关键约束：注入的消息与 BroadcastChannel 发出的是同一个对象（同 seq/sender），
 *  两条链路送达同一消息时 GameChannel 按 sender+seq 去重，绝不重复应用。 */
let rtcBroadcast: ((msg: GameMsg) => void) | null = null;

export function wireRtcBroadcast(fn: ((msg: GameMsg) => void) | null) {
  rtcBroadcast = fn;
}

export const DirectRtc = {
  broadcast(msg: GameMsg) {
    rtcBroadcast?.(msg);
  },
};
