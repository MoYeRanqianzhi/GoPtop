//! 对局线格式 — 唯一真源（对齐历史 frontend/src/net/protocol.ts，字段名逐一对齐）。
//!
//! 所有链路（同源 BroadcastChannel、WebRTC DataChannel、服务器 relay 兜底中转）
//! 传的都是这里定义的 JSON 结构。serde 规则：
//! - 判别标签为外部字段 `type`（TS 判别联合的 `type:`），枚举变体 PascalCase 原样；
//! - 字段名 camelCase（`dataUrl`/`lastMove`/`toMove`…），与 TS 对象字面量一致；
//! - `by`：发送端声明的执子颜色——接收端（尤其观战者）据此判定，不得从本地
//!   toMove/myColor 推断（历史 bug：任何一方认输观战者都判白胜）。
//!
//! 与 goptop-core 的差异：core 的 GameKind/Stone 是结构化枚举（`{"Go":{"size":9}}`），
//! 线格式是字符串标签 + 独立 size 字段（`"go"` + `9`）。两套各自序列化，wasm 边界
//! 已有换算；本模块在线格式侧统一用 [`GameKindT`]/[`Color`]，不与 core 类型混用。

use serde::{Deserialize, Serialize};

/// 线格式棋子颜色（TS StoneColor）。
pub type Color = String;

/// 线格式坐标（TS Coord）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordT {
    pub x: u16,
    pub y: u16,
}

/// 线格式棋类（TS GameKind 字符串）。
pub type GameKindT = String;

/// 线格式尺寸（TS Size：9/13/15/19）。
pub type SizeT = u16;

/// 线格式落子（TS Move 判别联合）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum MoveT {
    Place { coord: CoordT },
    Pass,
    Resign,
}

/// 消息体（TS MsgKind 判别联合，标签 `type`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum MsgKind {
    /// 握手：宣告棋类与尺寸。
    Hello { kind: GameKindT, size: SizeT },
    /// 落子/停一手/认输。`by` 见模块注释。
    Move {
        #[serde(rename = "move")]
        move_: MoveT,
        by: Color,
    },
    /// 全量快照（可选字段 sv = 回退纪元，见 useGameSession 审计 B1 注释）。
    /// history 可表达停一手（字符串 "pass"，untagged）。
    SyncState {
        #[serde(default)]
        sv: Option<u32>,
        board: Vec<Vec<Color>>,
        #[serde(rename = "toMove")]
        to_move: Color,
        winner: Option<Color>,
        history: Vec<HistoryEntry>,
        #[serde(rename = "lastMove")]
        last_move: Option<CoordT>,
        kind: GameKindT,
        size: SizeT,
    },
    /// 请求对端回全量快照。
    SyncRequest,
    /// 聊天文本。
    Chat { text: String },
    /// 心跳（保留占位，现无发送入口）。
    Ping,
    Pong,
    /// 本地对战直发重开（P2P 走 ResetReq 协商，此类型仅旧路径/本地）。
    Reset { kind: GameKindT, size: SizeT },
    // —— 协商类：请求 → 对方弹窗 → Ack；同意后双方各自执行确定性操作 ——
    UndoReq,
    UndoAck { ok: bool },
    ResetReq,
    ResetAck { ok: bool },
    SwapReq,
    SwapAck { ok: bool },
    // —— 围棋终局计分（2026-09-15 新增）：死子标记与计分确认同步 ——
    /// 死子标记同步：发送方当前标记的死子集合（双方各自标记，求交集为真死子）。
    ScoreMark { dead: Vec<CoordT> },
    /// 计分确认请求/批复（复用协商模式：一方点「确认计分」→ 对方弹窗 → Ack）。
    ScoreConfirmReq,
    ScoreConfirmAck { ok: bool },
    /// 头像：圆形裁剪后的 dataURL（≤128px，几 KB），走 P2P 不经服务器。
    Avatar { #[serde(rename = "dataUrl")] data_url: String },
}

/// history 条目：坐标对象或字符串 "pass"（untagged）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HistoryEntry {
    Place(CoordT),
    Pass(String),
}

/// 顶层对局消息（TS GameMsg）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameMsg {
    /// 发送方单调递增序号（去重键之一）。
    pub seq: u64,
    /// 发送方页面级随机 ID（TS sender）。
    pub sender: String,
    /// 发送方持久 userId（区分选手/观战者）。
    #[serde(rename = "userId")]
    pub user_id: String,
    pub kind: MsgKind,
}

impl GameMsg {
    /// 便捷构造。
    #[must_use]
    pub fn new(seq: u64, sender: &str, user_id: &str, kind: MsgKind) -> Self {
        Self { seq, sender: sender.to_string(), user_id: user_id.to_string(), kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 线格式与历史 TS 侧完全一致（历史 bug 防线：字段名漂移即失联）。
    #[test]
    fn wire_format_matches_ts() {
        let msg = GameMsg::new(3, "peer-x", "u-abc", MsgKind::Move {
            move_: MoveT::Place { coord: CoordT { x: 4, y: 5 } },
            by: "black".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"seq":3,"sender":"peer-x","userId":"u-abc","kind":{"type":"Move","move":{"type":"Place","coord":{"x":4,"y":5}},"by":"black"}}"#
        );
        // 往返。
        let back: GameMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    /// SyncState：camelCase 字段 + history 混合坐标/"pass" + sv 可选。
    #[test]
    fn sync_state_wire_format() {
        let json = r#"{"type":"SyncState","sv":2,"board":[["empty"]],"toMove":"white","winner":null,"history":[{"x":0,"y":0},"pass"],"lastMove":null,"kind":"go","size":9}"#;
        let k: MsgKind = serde_json::from_str(json).unwrap();
        match k {
            MsgKind::SyncState { sv, history, .. } => {
                assert_eq!(sv, Some(2));
                assert_eq!(history, vec![HistoryEntry::Place(CoordT { x: 0, y: 0 }), HistoryEntry::Pass("pass".into())]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 协商类与计分类消息的序列化名。
    #[test]
    fn negotiation_and_score_names() {
        assert_eq!(
            serde_json::to_string(&MsgKind::UndoAck { ok: true }).unwrap(),
            r#"{"type":"UndoAck","ok":true}"#
        );
        assert_eq!(serde_json::to_string(&MsgKind::UndoReq).unwrap(), r#"{"type":"UndoReq"}"#);
        assert_eq!(
            serde_json::to_string(&MsgKind::ScoreMark { dead: vec![CoordT { x: 1, y: 2 }] }).unwrap(),
            r#"{"type":"ScoreMark","dead":[{"x":1,"y":2}]}"#
        );
        assert_eq!(serde_json::to_string(&MsgKind::ScoreConfirmReq).unwrap(), r#"{"type":"ScoreConfirmReq"}"#);
    }
}
