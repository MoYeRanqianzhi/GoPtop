/**
 * sessionContext —— Rust 化第二阶段后的遗留类型（原三域工厂接口已随逻辑
 * 迁入 crates/goptop-net；本文件只剩 UI 层仍在引用的共享类型定义）。
 */

/** 协商/确认弹窗（快照 confirmReq 字段的类型；score-confirm 为围棋计分新增）。 */
export type ConfirmRequest = { kind: "undo" | "reset" | "swap" | "spec-chat" | "wrong-pwd" | "score-confirm"; from: string; fromName: string };
