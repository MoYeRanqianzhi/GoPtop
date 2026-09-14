//! 多链路去重 — (sender, seq) 单调去重表。
//!
//! 同一条对局消息可能经三条链路到达：同源 BroadcastChannel、WebRTC DataChannel、
//! 服务器 relay 兜底。有副作用的类型（落子、协商、聊天、头像）必须只应用一次——
//! 否则 SwapAck 这类「翻转」操作被执行两次就翻回去、同源聊天必然显示两遍。
//!
//! 幂等全量类型（SyncState/SyncRequest/Hello）**不去重**：重连后发送方 seq 归零，
//! 去重表会把重连同步错误丢弃。

use std::collections::HashMap;

/// 按消息类型判定是否参与去重（对齐历史 TS 侧 DEDUP 集合 + 新增计分类）。
#[must_use]
pub fn is_dedupable(kind_type: &str) -> bool {
    matches!(
        kind_type,
        "Move" | "UndoReq" | "UndoAck" | "ResetReq" | "ResetAck" | "SwapReq" | "SwapAck"
            | "Avatar" | "Chat" | "ScoreMark" | "ScoreConfirmReq" | "ScoreConfirmAck"
    )
}

/// 每个远端 sender 已应用的最大 seq。sender 每次页面加载重新生成、seq 单调递增，
/// 去重键不跨会话残留（新对局自然从 1 重新开始）。
#[derive(Default, Debug)]
pub struct DedupTable {
    last_seq: HashMap<String, u64>,
}

impl DedupTable {
    /// 判定并登记：返回 true 表示该消息应当应用（首达），false 表示重复丢弃。
    /// 仅 [`is_dedupable`] 的类型会登记与比较；其余恒返回 true。
    pub fn admit(&mut self, sender: &str, seq: u64, dedupable: bool) -> bool {
        if !dedupable {
            return true;
        }
        let seen = self.last_seq.entry(sender.to_string()).or_insert(0);
        if seq <= *seen {
            return false;
        }
        *seen = seq;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单调去重：首达应用、重复与乱序回退丢弃。
    #[test]
    fn monotonic_admission() {
        let mut t = DedupTable::default();
        assert!(t.admit("peer-a", 1, true));
        assert!(!t.admit("peer-a", 1, true)); // 同 seq 重复
        assert!(t.admit("peer-a", 2, true));
        assert!(!t.admit("peer-a", 1, true)); // 乱序回退（另一链路慢到）
        // sender 间独立。
        assert!(t.admit("peer-b", 1, true));
    }

    /// 幂等类型不去重：重连 seq 归零的 SyncRequest/SyncState 必须放行。
    #[test]
    fn idempotent_types_bypass() {
        let mut t = DedupTable::default();
        assert!(t.admit("peer-a", 5, false));
        assert!(t.admit("peer-a", 1, false)); // seq 回退也放行
    }

    /// 去重类型清单：协商/计分类在内，Hello/SyncState 在外。
    #[test]
    fn dedupable_classification() {
        for k in ["Move", "UndoReq", "UndoAck", "ResetReq", "ResetAck", "SwapReq", "SwapAck", "Avatar", "Chat", "ScoreMark", "ScoreConfirmReq", "ScoreConfirmAck"] {
            assert!(is_dedupable(k), "{k} 应去重");
        }
        for k in ["Hello", "SyncState", "SyncRequest", "Reset", "Ping", "Pong"] {
            assert!(!is_dedupable(k), "{k} 不应去重");
        }
    }
}
