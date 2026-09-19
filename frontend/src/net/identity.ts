/**
 * net/identity — 本页身份：用户 ID 与昵称。
 *
 * 每个标签页即一个用户：`userId` 存 `sessionStorage`（每页不同），昵称可改。
 * pwd 只在「已开战但无对手（waiting）」时有效；两人进对局后 pwd 即失效，
 * 不存在第三人凭旧 pwd 加入。
 *
 * 存储分工：`tabUser` 是**每标签页不同**的会话态，固定存 sessionStorage（四端一致）；
 * 昵称是用户设置，走 net/store 门面落平台存储（Web 端即 localStorage）。
 */
import { storeGet, storeSet } from "./store";

/** 本页的用户 id（sessionStorage，每页不同）。 */
export function myUserId(): string {
  try {
    let id = sessionStorage.getItem("goptop:tabUser");
    if (!id) {
      id = `u-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
      sessionStorage.setItem("goptop:tabUser", id);
    }
    return id;
  } catch {
    return `u-${Math.random().toString(36).slice(2, 10)}`;
  }
}

export function myName(): string {
  return storeGet("goptop:name") || "";
}

export function setMyName(n: string) {
  storeSet("goptop:name", n);
}

/** 每局轮换的一次性 pwd（邀请钥匙）。CSPRNG 生成，固定 6 位 base36。
 *  pwd 同时是信令编码密钥（G1 token 的 XOR 密钥，见 crates/goptop-net/src/codec.rs:131 encode），必须不可预测且长度稳定。 */
export function genPwd(): string {
  const buf = new Uint32Array(1);
  crypto.getRandomValues(buf);
  // 36^6 与 2^32 不整除：低段 2118184960 个值出现 2 次、其余 1 次，
  // 单值概率绝对差 ~2.3e-10——对 6 位钥匙的可预测性无实际影响
  return (buf[0] % 2176782336).toString(36).padStart(6, "0");
}

/** 每局轮换的 gameId（观战 channel 后缀）。 */
export function genGameId(): string {
  return `g-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}
