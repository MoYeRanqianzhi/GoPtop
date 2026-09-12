/**
 * chat —— 聊天域：统一入口 pushChat、发送 sendChat、显示名解析 resolvePeerName。
 *
 * 从 useGameSession 原样整体搬出（禁止行为变化）：函数体逐字保留，只把原本
 * 引用 hook 内 state/refs 的地方改为工厂顶部从 ctx 一次解构。
 * peersRefCache 仅本域使用（resolvePeerName 的名册缓存），随之迁入并由
 * 本模块的同步 effect 维护；pushChat 经 ctx 回填供 serverSignaling 使用。
 */
import { useEffect, useRef } from "react";
import { serverChannel } from "../net/serverChannel";
import { transport } from "../net/gameChannel";
import { myName } from "../net/identity";
import type { SessionCtx } from "./sessionContext";

export function createChat(ctx: Pick<
  SessionCtx,
  | "chatLog" | "peers" | "spectators" | "serverMode" | "tabUser"
  | "setChatLog"
  | "myHostRef" | "relayTargetsRef" | "roleRef"
>) {
  const { chatLog, peers, spectators, serverMode, tabUser, setChatLog, myHostRef, relayTargetsRef, roleRef } = ctx;

  /** 聊天统一入口：本地插入 + 服务器 signal（对局者）/定向转发（观战者）。 */
  function sendChat(text: string) {
    const t = text.trim();
    if (!t) return;
    pushChat(serverChannel.myServerId ?? tabUser, myName() || tabUser.slice(0, 8), t, true);
    if (roleRef.current === "spectator") {
      // 观战者只与自己 host 通信
      if (myHostRef.current && serverChannel.connected) {
        serverChannel.signal(myHostRef.current, "spec-chat", { userId: serverChannel.myServerId, name: myName() || "观战者", text: t });
      }
    } else if (serverMode && serverChannel.connected) {
      const opp = relayTargetsRef.current.values().next().value as string | undefined;
      if (opp) serverChannel.signal(opp, "chat", { userId: serverChannel.myServerId, name: myName() || tabUser.slice(0, 8), text: t });
    } else {
      transport.send({ type: "Chat", text: t });
    }
  }

  function pushChat(userId: string, name: string, text: string, self: boolean) {
    setChatLog((log) => [...log.slice(-199), { userId, name, text, ts: Date.now(), self }]);
  }

  /** 由内部 userId 解析显示名：优先聊天记录/名册，兜底「对方」。 */
  function resolvePeerName(userId: string, fallback: string): string {
    const fromChat = [...chatLog].reverse().find((c) => c.userId === userId);
    if (fromChat) return fromChat.name;
    const fromPeers = peersRefCache.current.get(userId);
    if (fromPeers) return fromPeers;
    return fallback === userId ? "对方" : fallback;
  }
  const peersRefCache = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const m = new Map<string, string>();
    for (const p of peers) m.set(p.id, p.name);
    for (const s of spectators) m.set(s.id, s.name);
    for (const c of chatLog) { if (!m.has(c.userId)) m.set(c.userId, c.name); }
    peersRefCache.current = m;
  }, [peers, spectators, chatLog]);

  return { pushChat, sendChat, resolvePeerName };
}
