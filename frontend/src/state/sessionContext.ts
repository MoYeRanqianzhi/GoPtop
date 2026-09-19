/**
 * sessionContext —— Rust 化第二阶段后的遗留类型（原三域工厂接口已随逻辑
 * 迁入 crates/goptop-net；本文件只剩 UI 层仍在引用的共享类型定义）。
 */

/** 协商/确认横幅消费的字段子集；快照 confirmReq 另带 queued（队列深度，session::snapshot:81），
 *  score-confirm 为围棋计分新增。 */
export type ConfirmRequest = { kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd" | "score-confirm"; from: string; fromName: string };
