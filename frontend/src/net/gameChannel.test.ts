/**
 * net/gameChannel 单元测试 —— GameChannel.dispatch 去重语义（审查 D11：P2P 底层此前零测试）。
 *
 * 覆盖面（纯逻辑，不碰真实网络）：
 * - Move 按 (sender, seq) 单调去重（BroadcastChannel/WebRTC/relay 多链路只应用一次）、
 *   SyncState 不去重（幂等全量同步，重连后 seq 归零不能被误丢）。
 *
 * 运行：`npm test`（node 24 自带 BroadcastChannel）。
 */
import { describe, it, expect } from "vitest";
import { GameChannel } from "./gameChannel";
import type { GameMsg } from "./protocol";

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

describe("GameChannel.dispatch — Chat 去重（审查 A2：同源双链路必重复）", () => {
  function remoteChat(seq: number, text: string, sender = "remote-peer"): GameMsg {
    return { seq, sender, userId: "remote-user", kind: { type: "Chat", text } };
  }

  it("同一条聊天经双链路到达只显示一次", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteChat(1, "你好"));
    ch.injectRemote(remoteChat(1, "你好"));
    expect(received).toHaveLength(1);
  });

  it("不同文本的不同 seq 正常送达", () => {
    const { ch, received } = isolatedChannel();
    ch.injectRemote(remoteChat(1, "你好"));
    ch.injectRemote(remoteChat(2, "在吗"));
    expect(received).toHaveLength(2);
  });
});
