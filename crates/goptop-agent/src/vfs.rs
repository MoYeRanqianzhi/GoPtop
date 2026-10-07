//! 一切皆文件 —— `/game` 动态合成 + `/game/in` 两段式控制文件 + `/memory` 转发 + `/index`。
//!
//! Linux 思路，防工具无限膨胀：对局状态、规则、历史局面、聊天全部合成**动态只读
//! 文件**，grep/read 即可灵活查看当前与历史局势；**动作 = 写文件**（坐标写进落子
//! 文件即暂存，`submit` 才落盘）。动态文件不落盘——read/grep 时从会话快照即时合成；
//! `/history/<n>` 按需合成。布局与全部取值协议以计划「一切皆文件」节 +
//! 「格式样例（权威基线）」为逐字基线，字段名/值域/错误文案不得自创。
//!
//! 两棵子树：`/game`（对局存活期存在）与 `/memory`（持久记忆，转发给
//! [`crate::store::VfsStore`]）。`/index` 列出全部路径与一句话说明（模型第一眼的
//! 可发现性；与提示词、工具描述三处互为冗余）。
//!
//! **两段式**（write=暂存可反复覆盖、submit 才执行）：给 agent「落笔后、提交前」
//! 的反悔窗口；格式错误在 write 即拒，对局规则错误（未轮到/占点/无待决/非计分态）
//! 在 submit 执行时拒并回人话。

use goptop_net::protocol::CoordT;

use crate::player::{EventQueue, PlayerHandle};

/// `/game/in/` 的六个控制文件（两段式的第一段落点）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InFile {
    /// 着法：`"x,y"` 或 `"pass"`（围棋）。
    Move,
    /// 聊天消息文本。
    Chat,
    /// 协商请求：`"undo"` | `"reset"` | `"swap"`。
    Request,
    /// 批复当前待决请求：`"approve"` | `"reject"`。
    Confirm,
    /// 确认计分：`"ok"`。
    Score,
    /// 认输：任意内容。
    Resign,
}

impl InFile {
    /// 控制文件全路径（`"/game/in/move"` 等）。
    #[must_use]
    pub fn path(self) -> &'static str {
        todo!()
    }

    /// 由路径反解（仅接受六个全路径字面量；其余回 None——调用方按未知路径报错）。
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        todo!()
    }

    /// 取值协议的一句话说明（write 的格式错误文案与工具描述共用这一份，防止
    /// 「描述说一套、校验拒另一套」的分叉）。
    #[must_use]
    pub fn accepted(self) -> &'static str {
        todo!()
    }
}

/// `/game` 子树下的动态文件（合成器按变体分派）。
///
/// 变体名即路径段（`BoardGrid` = `/game/board/grid`）；`HistoryN(n)` 是唯一的
/// 带参变体（`/game/history/<n>` 第 n 手后的局面）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameFile {
    /// `/game/status` 轮次/执色/终局/计分/待批复/暂存着法（JSON，权威样例）。
    Status,
    /// `/game/board` 当前局面·稀疏 JSON（默认最省 token；权威样例）。
    Board,
    /// `/game/board/grid` 全量二维数组（grid[y][x]，值域 empty|black|white|staged）。
    BoardGrid,
    /// `/game/board/ascii` 纯 ASCII 盘（`X=black O=white *=staged . =empty`）。
    BoardAscii,
    /// `/game/board/pretty` 制表符框线盘（`●`黑 `○`白 `◍`暂存）。
    BoardPretty,
    /// `/game/board/image.png` PNG 栅格（协议不支持视觉→调用方回占位文本）。
    BoardImage,
    /// `/game/rules` 当前棋种规则文本（markdown）。
    Rules,
    /// `/game/history` JSONL 落子历史（`{"n":1,"by":"black","x":7,"y":7}`，pass 记 `"pass":true`）。
    History,
    /// `/game/history/<n>` 第 n 手后的局面（与 board 同款五变体）。
    HistoryN(u32),
    /// `/game/chat` 聊天记录全文（散文逐行，含自己发的）。
    Chat,
    /// `/game/events` 全量事件历史 JSONL（seq 单调；与事件队列同源两出口）。
    Events,
}

/// `/game/history/<n>` 的文本变体（PNG 走 [`syn_board_png`]，不混进文本合成器）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryVariant {
    /// 稀疏 JSON（同 [`GameFile::Board`] 形态，stones 为该手数时的局面）。
    Json,
    /// 全量 grid JSON。
    Grid,
    /// ASCII。
    Ascii,
    /// 制表符 pretty。
    Pretty,
}

/// 路径解析结果：一个路径到底落到哪棵子树的哪个文件。
///
/// 没有 `Unknown` 变体：未知路径在 [`resolve`] 直接 `Err`（文案附 /index 提示）——
/// 「解析成功」就蕴含「可继续处理」，调用方少一条死分支。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// `/index` 全部路径索引与说明。
    Index,
    /// `/game` 子树。
    Game(GameFile),
    /// `/memory` 命中——转发 VfsStore；带「已剥掉 `/memory/` 前缀的相对路径」
    /// （**未规范化**：规范化在 VfsStore 入口做，本层不重复第二份规则）。
    Memory(String),
}

/// 路径解析入口 —— read/write/grep/submit 四个工具共用的第一跳。
///
/// 规则：`/game` → 动态合成器；`/game/in/*` → 暂存结构；`/memory` → VfsStore；
/// `/index` → 索引；未知路径 → `Err` 且文案末尾提示读 `/index`（计划「工具清单」节）。
/// 不带前导 `/` 的输入同样接受（模型经常忘斜杠），等价补全。
///
/// # Errors
/// 未知路径（文案含 /index 提示）；`/game/history/<n>` 的 n 非数字。
pub fn resolve(path: &str) -> Result<Resolved, String> {
    todo!()
}

/// 可写判定：`/game/in/*`（stage 语义）与 `/memory/**` 可写；
/// `/game` 其余全部动态文件与 `/index` 只读。
///
/// **为什么独立成函数而不是塞进 resolve**：read 的 offset/limit、grep 的前缀遍历、
/// write 的拒绝文案、submit 的路径判别各自只要其中一半事实，一个布尔谓词让每个
/// 调用点拿自己要的那一半，而不是背整个 Resolved match。
#[must_use]
pub fn is_writable(r: &Resolved) -> bool {
    todo!()
}

/// 暂存着法在合成里的呈现形态：`/game/status` 的 `staged_move` 字段、
/// `/game/board` 的 `staged_move`、grid 里的 `"staged"` 格、ascii 的 `*`、
/// pretty 的 `◍` 全部由它驱动（计划：暂存落子上盘可见，人能提前看到 Agent 打算下哪）。
///
/// 取值时机：每次合成前从 [`Staging`] 现读——没有缓存层，暂存即盘面所见。
pub type StagedMove = CoordT;

/* ---------------- 动态合成器（输出形态照计划样例逐字） ---------------- */

/// `/game/status`。字段序与样例一致：kind/size/you/opponent/phase/to_move/
/// move_count/winner/scoring/pending_request/staged_move。
#[must_use]
pub fn syn_status(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    todo!()
}

/// `/game/board`（默认·稀疏 JSON，最省 token）：kind/size/you/to_move/move_count/
/// winner/last_move/stones{black,white}/staged_move。
#[must_use]
pub fn syn_board(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    todo!()
}

/// `/game/board/grid`：`grid[y][x]` 二维数组，每格 `"empty"|"black"|"white"|"staged"`
/// ——不分 stones/staged，整盘每格都有描述（模拟棋盘的 JSON 版，有时更直观）。
#[must_use]
pub fn syn_board_grid(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    todo!()
}

/// `/game/board/ascii`：单空格分隔，列头/行头坐标数字，`X=black O=white *=staged . =empty`。
#[must_use]
pub fn syn_board_ascii(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    todo!()
}

/// `/game/board/pretty`：标准制表符 `┌┬┐├┼┤└┴┘` 画围棋式交叉点盘，`●`黑 `○`白 `◍`暂存。
#[must_use]
pub fn syn_board_pretty(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    todo!()
}

/// `/game/board/image.png`：`image` crate 手绘栅格（网格线+黑白圆子+暂存虚线圈，
/// 无字体无坐标标签——精确坐标以 JSON 为准）。
///
/// **承载能力矩阵**（计划）：Anthropic tool_result 原生 image block 可；
/// MCP `CallToolResult type:"image"` 可；OpenAI 两协议工具结果纯字符串不可
/// → 占位符文本（由 registry 按协议能力回，见 [`syn_image_placeholder`]）。
#[must_use]
pub fn syn_board_png(snap: &serde_json::Value, staged: Option<StagedMove>) -> Vec<u8> {
    todo!()
}

/// 视觉能力缺失时的占位文本（权威文案，逐字）：
/// `"image unavailable for this model/protocol — read /game/board/ascii instead"`。
#[must_use]
pub fn syn_image_placeholder() -> &'static str {
    "image unavailable for this model/protocol — read /game/board/ascii instead"
}

/// `/game/rules`：当前棋种规则文本（五子棋/围棋各自内容，markdown）。
/// 规则摘要在系统提示词里只是摘要，全文由这里给——摘要与全文的分工照计划
/// 「系统提示词」节的 Rules digest 条目。
#[must_use]
pub fn syn_rules(snap: &serde_json::Value) -> String {
    todo!()
}

/// `/game/history`：JSONL 逐行 `{"n":..,"by":"..","x":..,"y":..}`（pass 行记
/// `"pass":true`，无坐标字段）。来源是快照 history 数组（含序号重编——history
/// 永远从 1 起，悔棋后自然回退）。
#[must_use]
pub fn syn_history(snap: &serde_json::Value) -> String {
    todo!()
}

/// `/game/history/<n>`：第 n 手后的局面，变体与 board 同款。
///
/// 实现路数：重放 history 前 n 手到空盘（围棋提子/禁着经规则引擎，不能手搓棋盘
/// 数组——提子后的盘面手搓必错），再按变体渲染。n 越界（0 或 > 总手数）回 Err。
///
/// # Errors
/// n 非法（越界/为 0）；棋种与 history 不匹配（理论不可达，防御到错误文案）。
pub fn syn_history_n(snap: &serde_json::Value, n: u32, variant: HistoryVariant) -> Result<String, String> {
    todo!()
}

/// `/game/chat`：聊天记录散文逐行（`名字: 文本`；含自己发的）。
/// 长记录靠 read 的 offset/limit 翻页，不在这里截断。
#[must_use]
pub fn syn_chat(snap: &serde_json::Value) -> String {
    todo!()
}

/// `/game/events`：事件队列全量历史的 JSONL（seq 单调）。
/// 与 [`EventQueue::history_jsonl`] 同源的直通口——本函数存在的意义是把
/// 「events 文件从哪来」钉在 vfs 的合成器清单里，registry 不必知道队列内部。
#[must_use]
pub fn syn_events(queue: &EventQueue) -> String {
    todo!()
}

/// `/index`：全部路径索引与一句话说明（计划里有逐字基线，照抄不发挥）。
/// **动态段**（/game/status 等）在运行期不变，可直接整段返回常量文本。
#[must_use]
pub fn syn_index() -> String {
    todo!()
}

/// 动态文件的统一读口：按解析结果分派到合成器。
///
/// `/game/in/*` 也走这里：read 回显**暂存内容**（两段式的「可 read 回看」承诺）；
/// 未暂存回一句话说明（该文件的功能职责——in/ 的取值协议本就写进工具描述，
/// read 空槽时再给一遍是三处冗余的最后一处）。`/game/board/image.png` 在文本读口
/// 拒绝，回 [`syn_image_placeholder`]——图像只经协议适配器走 [`syn_board_png`]。
///
/// # Errors
/// HistoryN 越界（[`syn_history_n`] 透传）；未知路径（[`resolve`] 透传）。
pub fn read_dynamic(
    r: &Resolved,
    snap: &serde_json::Value,
    staged: Option<StagedMove>,
    queue: &EventQueue,
    staging: &Staging,
) -> Result<String, String> {
    todo!()
}

/* ---------------- in/ 暂存区与 submit 分发 ---------------- */

/// in/ 暂存区 —— 两段式的第一段。
///
/// write 只做**格式校验**并回预览（计划样例：
/// `{"ok":true,"staged":{"path":"/game/in/move","value":{"x":8,"y":8}},"note":"call submit(\"/game/in/move\") to place"}`），
/// 不执行；可反复覆盖；可 read 回看。submit 消费并清槽。
///
/// 值存 write 的**原始文本**（校验通过后原样留档）：read 回看的是「我写了什么」，
/// 不是解析后的形态——两者不一致时以原文为准，避免「回看的是另一回事」的错觉。
#[derive(Default)]
pub struct Staging {
    slots: std::sync::Mutex<std::collections::HashMap<&'static str, String>>,
}

impl Staging {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 暂存：按 [`InFile::accepted`] 的协议校验格式。成功回暂存回执 JSON（计划样例）；
    /// 格式错回 `Err`（权威错误文案逐字，如
    /// `invalid move "abc". Expected "x,y" (e.g. "7,7") or "pass".`）。
    ///
    /// **只查格式不查对局规则**：坐标是否越界/占用、是否轮到我、有没有待决请求
    /// ……全部留给 submit——格式错误写时拒（便宜、即时），规则错误提交时拒（此时
    /// 才有权威快照可查）。
    ///
    /// # Errors
    /// 取值协议不匹配（文案逐字照计划样例）。
    pub fn stage(&self, file: InFile, content: &str) -> Result<serde_json::Value, String> {
        todo!()
    }

    /// 回看暂存内容（read /game/in/* 的取数口）；空槽回 None。
    #[must_use]
    pub fn peek(&self, file: InFile) -> Option<String> {
        todo!()
    }

    /// 提交取走（取走即清槽——同一份内容不允许二次 submit；第二次回
    /// 「无内容」级错误，提示先 write 再 submit）。
    pub fn take(&self, file: InFile) -> Option<String> {
        todo!()
    }
}

/// submit 分发：按路径自动判别提交什么并驱动玩家席执行，回执行结果 JSON
/// （计划样例：`{"ok":true,"action":"move","move_count":2,"finished":false}`；
/// 各动作的回执字段按计划「工具清单」submit 行）。
///
/// 映射：in/move→Place/Pass、in/chat→SendChat、in/request→RequestUndo/Reset/Swap、
/// in/confirm→ConfirmApprove/Decline、in/score→ConfirmScore、in/resign→Resign。
///
/// **执行后必须 settle**：对手同步与事件生成需要几拍传播（headless.rs 的 settle/
/// pump_until 手法），settle_ms 由调用方给（工具层用固定短拍，等待对手行动是
/// wait_events/事件推送的事，不是 submit 的）。回执从 settle 后重读的快照取数。
///
/// # Errors
/// 空槽（没 write 就 submit）；对局规则错误（未轮到/占点/无待决/非计分态——
/// 错误文案照计划样例逐字）；玩家席命令拒发（理论不可达，防御性文案）。
pub async fn dispatch_submit(
    file: InFile,
    player: &dyn PlayerHandle,
    staging: &Staging,
    settle_ms: u64,
) -> Result<serde_json::Value, String> {
    todo!()
}
