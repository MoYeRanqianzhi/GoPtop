/**
 * serverSignaling —— 服务器模式（可选）：userId 链接 + pwd 客户端校验 + 观战房间管理。
 *
 * 从 useGameSession 原样整体搬出（禁止行为变化）：函数体逐字保留，只把原本
 * 引用 hook 内 state/refs/函数的地方改为工厂顶部从 ctx 一次解构。
 * ref 所有权全部留在 useGameSession（specPwdRef 等需从返回值暴露），经 ctx 传入。
 */
import { serverChannel } from "../net/serverChannel";
import { transport } from "../net/gameChannel";
import { DirectRtcPeer } from "../net/rtc";
import { genGameId, genPwd, myName } from "../net/identity";
import { inviteToUrl, nav } from "../net/links";
import type { GameKind, Size } from "../net/protocol";
import type { SessionCtx } from "./sessionContext";

export function createServerSignaling(ctx: Pick<
  SessionCtx,
  | "peers" | "serverIncoming" | "tabUser"
  | "setRole" | "setMyColor" | "setPhase" | "setConfirmReq" | "setPwd" | "setInviteUrl"
  | "setPeerConnected" | "setServerIncoming" | "setSpecRequests" | "setSpectateEnabled"
  | "setSpecCanChat" | "setPeerAvatars" | "setGameId" | "setWatchUrl" | "setAnswerBackUrl"
  | "setSpectators"
  | "pendingLinkRef" | "roleRef" | "myHostRef" | "myColorRef" | "phaseRef" | "gameIdRef"
  | "relayTargetsRef" | "inviterRtcRef" | "pwdRef" | "kindRef" | "sizeRef" | "rtcPeersRef"
  | "spectateEnabledRef" | "specPwdRef" | "specChatOkRef" | "specRequestDeniedRef"
  | "specCanChatRef" | "confirmResolveRef" | "confirmRejectRef" | "specRequestsRef" | "spectatorsRef"
  | "showNotice" | "attachPeer" | "closeAllRtcPeers" | "backHome" | "resetBoardFor" | "pushChat"
>) {
  const {
    peers, serverIncoming, tabUser,
    setRole, setMyColor, setPhase, setConfirmReq, setPwd, setInviteUrl,
    setPeerConnected, setServerIncoming, setSpecRequests, setSpectateEnabled,
    setSpecCanChat, setPeerAvatars, setGameId, setWatchUrl, setAnswerBackUrl,
    setSpectators,
    pendingLinkRef, roleRef, myHostRef, myColorRef, phaseRef, gameIdRef,
    relayTargetsRef, inviterRtcRef, pwdRef, kindRef, sizeRef, rtcPeersRef,
    spectateEnabledRef, specPwdRef, specChatOkRef, specRequestDeniedRef,
    specCanChatRef, confirmResolveRef, confirmRejectRef, specRequestsRef, spectatorsRef,
    showNotice, attachPeer, closeAllRtcPeers, backHome, resetBoardFor, pushChat,
  } = ctx;

  /* ---------- 服务器模式（可选）：userId 链接 + pwd 客户端校验 + 观战房间管理 ---------- */
  // 链接模型回归 /<userId>?pwd=...：服务器模式下受邀者/观战者打开链接后经服务器
  // signal 交互（join→offer→answer），pwd 由对局者本地校验——pwd 错误变成弹窗邀请，
  // 而不是像无服务器模式那样直接解不开。服务器只转发，不落地任何数据。

  /** userId 链接意图就绪执行（连接 ready 后发 join/spec-join）。
   *  不在本地查名册判断在线：ready 与首份名册广播的先后不确定，
   *  误判会把拿着有效邀请链接的人弹回主页——直接发信号，无人受理由超时兜底。 */
  function flushPendingServerIntents() {
    const l = pendingLinkRef.current;
    if (l && serverChannel.connected) {
      pendingLinkRef.current = null;
      if (l.spec) {
        setRole("spectator");
        roleRef.current = "spectator";
        myHostRef.current = l.target;
        nav("/p2p");
        serverChannel.signal(l.target, "spec-join", { pwd: l.pwd ?? "", name: myName() });
        showNotice("正在连接对局观战…");
      } else {
        setRole("invitee");
        roleRef.current = "invitee";
        setMyColor("white");
        myColorRef.current = "white";
        setPhase("waiting");
        phaseRef.current = "waiting";
        nav("/p2p");
        serverChannel.signal(l.target, "join", { pwd: l.pwd ?? "", name: myName() });
        showNotice("正在请求加入对局…");
        // 无应答兜底：15s 内既没有 offer 也没有拒绝，多半是对方离线/已换局
        window.setTimeout(() => {
          if (roleRef.current === "invitee" && phaseRef.current === "waiting" && !gameIdRef.current) {
            showNotice("对方不在线或未响应，请确认链接是否最新", 6000);
          }
        }, 15_000);
      }
    }
  }

  function syncSpectators(list: { id: string; name: string; host: string; muted: boolean }[]) {
    spectatorsRef.current = list;
    setSpectators([...list]);
  }

  /** 房间名单变化后向对方对局者与全部观战者广播。 */
  function pushSpecSync(extra?: Record<string, unknown>) {
    const opp = relayTargetsRef.current.values().next().value as string | undefined;
    const payload = { list: spectatorsRef.current, enabled: spectateEnabledRef.current, ...extra };
    if (opp && serverChannel.connected) serverChannel.signal(opp, "spec-sync", payload);
    for (const s of spectatorsRef.current) {
      if (s.host === serverChannel.myServerId && serverChannel.connected) serverChannel.signal(s.id, "spec-sync", payload);
    }
  }

  /** 对局者：受理一次观战（建 spectator 直连并发 offer）。 */
  async function serverAdmitSpectator(sid: string, sname: string) {
    if (phaseRef.current === "home") return;
    relayTargetsRef.current.add(sid);
    const peer = new DirectRtcPeer({ isInviter: true, role: "spectator" });
    peer.peerTag = sid;
    attachPeer(peer);
    try {
      const offer = await peer.createOfferPlain();
      syncSpectators([...spectatorsRef.current, { id: sid, name: sname, host: serverChannel.myServerId ?? "", muted: false }]);
      pushSpecSync();
      serverChannel.signal(sid, "spec-offer", { gameId: gameIdRef.current ?? "", offer });
    } catch {
      try { peer.close(); } catch { /* ignore */ }
    }
  }

  /** 对局者：处理服务器转发来的信令（kind 语义见各分支）。 */
  function serverHandleSignal(from: string, kind: string, pl: Record<string, unknown>) {
    const fromName = String(pl.name ?? peers.find((u) => u.id === from)?.name ?? from);
    switch (kind) {
      // —— 受邀者请求加入对局：pwd 校验在本端，错误转弹窗询问 ——
      case "join": {
        if (phaseRef.current !== "waiting" || roleRef.current !== "inviter" || !inviterRtcRef.current) {
          serverChannel.signal(from, "reject", { name: myName(), reason: phaseRef.current === "playing" ? "正在对局中" : "对方不在等待对局" });
          return;
        }
        const ok = typeof pl.pwd === "string" && pl.pwd === pwdRef.current;
        const proceed = () => {
          const host = inviterRtcRef.current;
          if (!host) return;
          serverChannel.signal(from, "accept", {});
          void (async () => {
            try {
              const offer = await host.createOfferPlain();
              if (inviterRtcRef.current !== host) return;
              host.peerTag = from;
              relayTargetsRef.current.add(from);
              serverChannel.signal(from, "offer", { name: myName(), kind: kindRef.current, size: sizeRef.current, gameId: gameIdRef.current ?? "", offer });
            } catch { /* offer 失败：对端等待超时自行退出 */ }
          })();
        };
        if (ok) proceed();
        else {
          setConfirmReq({ kind: "wrong-pwd", from, fromName });
          confirmResolveRef.current = proceed;
          confirmRejectRef.current = () => serverChannel.signal(from, "reject", { name: myName(), reason: "邀请钥匙不正确" });
        }
        return;
      }
      // —— 受邀者/观战者回 answer：路由到 peerTag 匹配的 peer ——
      case "answer": {
        const main = inviterRtcRef.current;
        const peer = (main && main.peerTag === from ? main : null)
          ?? rtcPeersRef.current.find((q) => q.peerTag === from)
          ?? null;
        if (!peer) return;
        void peer.acceptAnswerPlain(String(pl.answer ?? ""));
        if (peer === main && phaseRef.current === "waiting" && roleRef.current === "inviter") {
          setPhase("playing");
          phaseRef.current = "playing";
          setPwd(null);
          pwdRef.current = null;
          setInviteUrl(null);
          setPeerConnected(false);
          showNotice("对方已加入，对局开始");
          setTimeout(() => {
            transport.send({ type: "Hello", kind: kindRef.current, size: sizeRef.current });
            transport.send({ type: "SyncRequest" });
          }, 80);
        }
        return;
      }
      // —— 邀请者发来的 offer（受邀者视角）——
      case "offer": {
        const offKind: GameKind = pl.kind === "go" ? "go" : "gomoku";
        const offSize: Size = pl.size === 9 || pl.size === 13 || pl.size === 19 ? pl.size : 15;
        void serverAcceptOffer(from, String(pl.name ?? from), offKind, offSize, String(pl.gameId ?? ""), String(pl.offer ?? ""));
        return;
      }
      // —— 拒绝与理由 ——
      case "reject": {
        showNotice(`对方拒绝：${String(pl.reason ?? "未说明")}`, 3600);
        if (phaseRef.current === "waiting") backHome();
        return;
      }
      // —— 大厅挑战 ——
      case "challenge": {
        if (phaseRef.current === "playing") {
          serverChannel.signal(from, "reject", { name: myName(), reason: "正在对局中" });
          return;
        }
        if (phaseRef.current !== "home") {
          serverChannel.signal(from, "reject", { name: myName(), reason: "当前不可接受挑战" });
          return;
        }
        const incKind: GameKind = pl.kind === "go" ? "go" : "gomoku";
        const incSize: Size = pl.size === 9 || pl.size === 13 || pl.size === 19 ? pl.size : 15;
        setServerIncoming({ from, fromName, kind: incKind, size: incSize });
        return;
      }
      // —— 挑战被接受：发起者（执黑）建局并送出 offer ——
      case "challenge-accepted": {
        // 挑战者发 challenge 时处于 home（serverChallengePeer 仅主页可发起且不改状态），
        // 同意信到达时仍是 home。旧守卫要求 waiting+inviter——该状态在本流程不可达，
        // 导致对方同意后建局被静默跳过、双方都停在原地。保留 waiting+inviter 分支：
        // 「先挑战、又点了开启对战」的边缘顺序下，被接受的挑战优先成局。
        if (phaseRef.current !== "home" && !(phaseRef.current === "waiting" && roleRef.current === "inviter")) return;
        serverAdmitChallenger(from);
        return;
      }
      // —— 观战请求（spec=1 链接）：pwd 对自动同意，错/无转聊天区私有申请 ——
      case "spec-join": {
        if (!spectateEnabledRef.current) {
          serverChannel.signal(from, "spec-reply", { name: myName(), ok: false, reason: "房主已关闭观战" });
          return;
        }
        if (phaseRef.current === "home") {
          serverChannel.signal(from, "spec-reply", { name: myName(), ok: false, reason: "对局不存在" });
          return;
        }
        const ok = typeof pl.pwd === "string" && pl.pwd !== "" && pl.pwd === specPwdRef.current;
        if (ok) {
          void serverAdmitSpectator(from, fromName);
        } else {
          setSpecRequests((r) => (r.some((x) => x.from === from) ? r : [...r, { from, fromName }]));
          serverChannel.signal(from, "spec-pending", { name: myName() });
        }
        return;
      }
      // —— 观战者收到房主 offer ——
      case "spec-offer": {
        void serverAcceptSpectatorOffer(from, String(pl.offer ?? ""), String(pl.gameId ?? ""));
        return;
      }
      // —— 观战申请批复 ——
      case "spec-reply": {
        if (pl.ok === true) {
          // 批准后马上会收到 spec-offer
          showNotice("观战申请已通过，连接中…");
        } else {
          specRequestDeniedRef.current = true;
          showNotice(`观战申请被拒绝：${String(pl.reason ?? "未说明")}（本局无法再次申请）`, 4200);
          if (phaseRef.current === "home" && roleRef.current === "idle") backHome();
        }
        return;
      }
      case "spec-pending": {
        showNotice("观战申请已送达，等待对方处理…");
        return;
      }
      // —— 观战房间名单同步（名单/开关/禁言）——
      case "spec-sync": {
        if (Array.isArray(pl.list)) {
          syncSpectators(pl.list as { id: string; name: string; host: string; muted: boolean }[]);
          const meKicked = !spectatorsRef.current.some((s) => s.id === serverChannel.myServerId) && myHostRef.current !== null;
          if (meKicked && roleRef.current === "spectator") {
            myHostRef.current = null;
            closeAllRtcPeers();
            backHome();
            showNotice("你已被移出观战", 3600);
          }
        }
        if (typeof pl.enabled === "boolean") setSpectateEnabled(pl.enabled);
        return;
      }
      // —— 踢出对方直连的观战者（先通知被踢者，再移除）——
      case "spec-kick": {
        const id = String(pl.id ?? "");
        if (serverChannel.connected) serverChannel.signal(id, "spec-kicked", { name: myName() });
        const target = rtcPeersRef.current.find((q) => q.peerTag === id);
        if (target) {
          try { target.close(); } catch { /* ignore */ }
          rtcPeersRef.current = rtcPeersRef.current.filter((q) => q !== target);
          syncSpectators(spectatorsRef.current.filter((s) => s.id !== id));
          pushSpecSync();
        }
        return;
      }
      // —— 被踢者收到通知：断开并回主页 ——
      case "spec-kicked": {
        myHostRef.current = null;
        closeAllRtcPeers();
        backHome();
        showNotice("你已被移出观战", 3600);
        return;
      }
      // —— 观战者聊天（经 host 转发进双方聊天区）——
      case "spec-chat": {
        pushChat(String(pl.userId ?? from), String(pl.name ?? from), String(pl.text ?? ""), false);
        // host 转发给另一位对局者；观战者之间互不可见（v1 简化）
        const opp = [...relayTargetsRef.current].find((id) => id !== from);
        if (opp && serverChannel.connected) serverChannel.signal(opp, "spec-chat", pl);
        return;
      }
      // —— 观战者申请发言：弹窗批准，双方都同意才放开 ——
      // 第一 host 批准后把申请转发给另一位对局者（applicant 带申请者 ID、relay=true），
      // 第二 host 批准后只向申请者回 ack、不再转发——避免 A↔B 循环弹窗
      case "spec-chat-req": {
        if (phaseRef.current !== "playing") return;
        const isRelay = pl.relay === true;
        const applicant = isRelay ? String(pl.applicant ?? from) : from;
        setConfirmReq({ kind: "spec-chat", from, fromName });
        confirmResolveRef.current = () => {
          serverChannel.signal(applicant, "spec-chat-ack", { ok: true, name: myName() });
          if (!isRelay) {
            const opp = [...relayTargetsRef.current].find((id) => id !== from);
            if (opp && serverChannel.connected) serverChannel.signal(opp, "spec-chat-req", { name: fromName, relay: true, applicant: from });
          }
        };
        confirmRejectRef.current = () => {
          serverChannel.signal(applicant, "spec-chat-ack", { ok: false, name: myName() });
        };
        return;
      }
      // —— 发言批准回执：双方都批准才放开输入 ——
      case "spec-chat-ack": {
        if (pl.ok === true) {
          specChatOkRef.current.add(from);
          if (specChatOkRef.current.size >= 2) {
            specRequestDeniedRef.current = false;
            specCanChatRef.current = true;
            setSpecCanChat(true);
            showNotice("双方已同意，你可以参与聊天了", 3200);
          }
        } else {
          specCanChatRef.current = false;
          setSpecCanChat(false);
          specRequestDeniedRef.current = true;
          showNotice("发言申请被拒绝（本局无法再次申请）", 3600);
        }
        return;
      }
      // —— 观战者头像 ——
      case "spec-avatar": {
        const dataUrl = String(pl.dataUrl ?? "");
        if (dataUrl.startsWith("data:image/") && dataUrl.length < 20_000) {
          setPeerAvatars((m) => ({ ...m, [from]: dataUrl }));
        }
        return;
      }
      // —— 对局者聊天（服务器模式不经 GameChannel，直接 signal）——
      case "chat": {
        pushChat(String(pl.userId ?? from), String(pl.name ?? from), String(pl.text ?? ""), false);
        return;
      }
    }
  }

  /** 大厅挑战被接受后：发起者（执黑）建局并送 offer（与 join 流共用 accept/offer 顺序）。 */
  async function serverAdmitChallenger(from: string) {
    closeAllRtcPeers();
    const g = genGameId();
    const p = genPwd();
    // 挑战成局同样要发观战钥匙：缺了它对局卡整个观战区块都不渲染
    specPwdRef.current = genPwd();
    spectateEnabledRef.current = true;
    setSpectateEnabled(true);
    setGameId(g);
    gameIdRef.current = g;
    setPwd(p);
    pwdRef.current = p;
    setRole("inviter");
    roleRef.current = "inviter";
    setPhase("waiting");
    phaseRef.current = "waiting";
    setMyColor("black");
    myColorRef.current = "black";
    setPeerConnected(false);
    resetBoardFor(kindRef.current, sizeRef.current);
    transport.join(g);
    setInviteUrl(inviteToUrl(tabUser, p, kindRef.current, sizeRef.current));
    setWatchUrl(null);
    nav("/p2p");
    const peer = new DirectRtcPeer({ isInviter: true, role: "player" });
    attachPeer(peer);
    try {
      const offer = await peer.createOfferPlain();
      peer.peerTag = from;
      relayTargetsRef.current.add(from);
      inviterRtcRef.current = peer;
      serverChannel.signal(from, "offer", { name: myName(), kind: kindRef.current, size: sizeRef.current, gameId: g, offer });
    } catch {
      try { peer.close(); } catch { /* ignore */ }
      showNotice("直连邀请生成失败：可取消后重开");
    }
  }

  /** 受邀者：收到对局者发来的 offer（join/challenge 流共用）——受理进等待并回 answer。 */
  async function serverAcceptOffer(from: string, fromName: string, k: GameKind, s: Size, gameId: string, offer: string) {
    if (phaseRef.current === "playing") return;
    closeAllRtcPeers();
    setGameId(gameId);
    gameIdRef.current = gameId;
    setRole("invitee");
    roleRef.current = "invitee";
    setMyColor("white");
    myColorRef.current = "white";
    setPeerConnected(false);
    resetBoardFor(k, s);
    transport.join(gameId);
    setPhase("waiting");
    phaseRef.current = "waiting";
    setPwd(null);
    pwdRef.current = null;
    setInviteUrl(null);
    setWatchUrl(null);
    setAnswerBackUrl(null);
    relayTargetsRef.current = new Set([from]);
    showNotice(`接受 ${fromName} 的邀请，正在建立直连…`);
    nav("/p2p");
    const peer = new DirectRtcPeer({ isInviter: false, role: "player" });
    peer.peerTag = from;
    attachPeer(peer);
    try {
      const ans = await peer.acceptOfferPlain(offer);
      serverChannel.signal(from, "answer", { name: myName(), answer: ans });
    } catch {
      showNotice("直连建立失败，可检查设置页线路后重试");
    }
  }

  /** 观战者：收到房主发来的 spectator offer（spec-join 批准后到达）。 */
  async function serverAcceptSpectatorOffer(from: string, offer: string, gameId: string) {
    if (roleRef.current !== "spectator") return;
    if (gameId) {
      setGameId(gameId);
      gameIdRef.current = gameId;
      transport.join(gameId);
    }
    // 进观战态（UI 需 phase=playing 才渲染对局页；SyncState 到达后即可见棋局）
    if (phaseRef.current !== "playing") {
      setPhase("playing");
      phaseRef.current = "playing";
      setPwd(null);
      pwdRef.current = null;
      setInviteUrl(null);
      setWatchUrl(null);
      showNotice("观战模式：只读同步，不可落子");
    }
    const peer = new DirectRtcPeer({ isInviter: false, role: "spectator" });
    peer.peerTag = from;
    attachPeer(peer);
    try {
      const ans = await peer.acceptOfferPlain(offer);
      serverChannel.signal(from, "answer", { name: myName(), answer: ans });
    } catch {
      showNotice("观战直连建立失败，可刷新后重试");
    }
  }

  /** 大厅挑战（服务器模式）：直接向对方发挑战信（无 pwd，需对方手动同意）。 */
  function serverChallengePeer(to: string) {
    if (phaseRef.current !== "home") {
      showNotice("当前状态不可发起挑战", 2400);
      return;
    }
    serverChannel.signal(to, "challenge", { name: myName(), kind: kindRef.current, size: sizeRef.current });
    showNotice("挑战已发出，等待对方同意…", 8000);
  }

  /** 被挑战者同意：回 challenge-accepted，等对方建局送 offer。 */
  function serverAcceptChallenge() {
    const inc = serverIncoming;
    if (!inc) return;
    setServerIncoming(null);
    serverChannel.signal(inc.from, "challenge-accepted", { name: myName() });
    showNotice("已接受挑战，等待对方建立直连…", 6000);
  }

  function serverRejectChallenge() {
    const inc = serverIncoming;
    if (!inc) return;
    setServerIncoming(null);
    serverChannel.signal(inc.from, "reject", { name: myName(), reason: "已拒绝挑战" });
    showNotice("已拒绝该挑战");
  }

  /* ---------- 观战房间管理（对局者权限）与观战者动作 ---------- */

  /** 对局者：批准聊天区里的观战申请（pwd 错/无的请求）。 */
  function approveSpecRequest(from: string) {
    setSpecRequests((r) => r.filter((x) => x.from !== from));
    const name = specRequestsRef.current.find((x) => x.from === from)?.fromName ?? from;
    void serverAdmitSpectator(from, name);
  }

  function rejectSpecRequest(from: string) {
    const req = specRequestsRef.current.find((x) => x.from === from);
    setSpecRequests((r) => r.filter((x) => x.from !== from));
    if (req) serverChannel.signal(from, "spec-reply", { name: myName(), ok: false, reason: "房主拒绝了观战申请" });
  }

  /** 对局者：踢出观战者（自己的直接关连接；对方直连的经 spec-kick 转移处理）。
   *  先向被踢者发 spec-kicked 通知再从名单移除——移除后 spec-sync 就送不到它了。 */
  function kickSpectator(id: string) {
    const target = spectatorsRef.current.find((s) => s.id === id);
    if (!target) return;
    if (target.host === serverChannel.myServerId) {
      if (serverChannel.connected) serverChannel.signal(id, "spec-kicked", { name: myName() });
      const peer = rtcPeersRef.current.find((q) => q.peerTag === id);
      if (peer) {
        try { peer.close(); } catch { /* ignore */ }
        rtcPeersRef.current = rtcPeersRef.current.filter((q) => q !== peer);
      }
    } else if (serverChannel.connected) {
      serverChannel.signal(target.host, "spec-kick", { name: myName(), id });
    }
    syncSpectators(spectatorsRef.current.filter((s) => s.id !== id));
    pushSpecSync();
  }

  /** 对局者：禁言/解除禁言。 */
  function muteSpectator(id: string, muted: boolean) {
    syncSpectators(spectatorsRef.current.map((s) => (s.id === id ? { ...s, muted } : s)));
    pushSpecSync();
  }

  /** 对局者：关闭观战（本局所有人无法观战，全部踢出）。 */
  function disableSpectate() {
    setSpectateEnabled(false);
    spectateEnabledRef.current = false;
    specPwdRef.current = null;
    // 踢出全部观战者
    for (const s of spectatorsRef.current) {
      if (s.host === serverChannel.myServerId) {
        const peer = rtcPeersRef.current.find((q) => q.peerTag === s.id);
        if (peer) {
          try { peer.close(); } catch { /* ignore */ }
          rtcPeersRef.current = rtcPeersRef.current.filter((q) => q !== peer);
        }
      } else if (serverChannel.connected) {
        serverChannel.signal(s.host, "spec-kick", { name: myName(), id: s.id });
      }
    }
    syncSpectators([]);
    setWatchUrl(null);
    pushSpecSync();
    showNotice("已关闭本局观战", 3000);
  }

  /** 观战者：申请发言（一次机会，双 host 均批准才可发言）。 */
  function requestSpecChat() {
    if (roleRef.current !== "spectator") return;
    if (specRequestDeniedRef.current) {
      showNotice("本局发言申请已被拒绝，无法再次申请", 3000);
      return;
    }
    const host = myHostRef.current;
    if (host && serverChannel.connected) {
      serverChannel.signal(host, "spec-chat-req", { name: myName() });
      showNotice("发言申请已发送，等待双方同意…", 6000);
    }
  }

  return {
    flushPendingServerIntents,
    handleSignal: serverHandleSignal,
    serverAdmitChallenger,
    serverAcceptOffer,
    serverAcceptSpectatorOffer,
    serverChallengePeer,
    serverAcceptChallenge,
    serverRejectChallenge,
    approveSpecRequest,
    rejectSpecRequest,
    kickSpectator,
    muteSpectator,
    disableSpectate,
    requestSpecChat,
  };
}
