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
//!
//! **为什么合成文件是手拼 JSON 字符串而非 `json!` 宏**：workspace 的 serde_json
//! 未开 `preserve_order`（开了会经 feature 统一改变全仓 Map 行为，越出本 crate
//! 范围），`json!` 的键序是字典序而权威样例的字段序是语义序——样例即契约，
//! 手拼换来逐字节对齐（嵌套数组仍用 serde 序列化，避免手写括号出错）。

use goptop_net::protocol::CoordT;
use goptop_net::session::UiCommand;

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
        match self {
            Self::Move => "/game/in/move",
            Self::Chat => "/game/in/chat",
            Self::Request => "/game/in/request",
            Self::Confirm => "/game/in/confirm",
            Self::Score => "/game/in/score",
            Self::Resign => "/game/in/resign",
        }
    }

    /// 由路径反解（仅接受六个全路径字面量；其余回 None——调用方按未知路径报错）。
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        // 与 registry 的 classify_in 同一条宽容口径（模型忘写前导 `/` 同样接受）；
        // 这里只做全字面量匹配，`/game/in/move/` 尾斜杠等残缺形态一律不认。
        let t = path.trim().trim_start_matches('/');
        match t {
            "game/in/move" => Some(Self::Move),
            "game/in/chat" => Some(Self::Chat),
            "game/in/request" => Some(Self::Request),
            "game/in/confirm" => Some(Self::Confirm),
            "game/in/score" => Some(Self::Score),
            "game/in/resign" => Some(Self::Resign),
            _ => None,
        }
    }

    /// 取值协议的一句话说明（write 的格式错误文案与工具描述共用这一份，防止
    /// 「描述说一套、校验拒另一套」的分叉）。
    #[must_use]
    pub fn accepted(self) -> &'static str {
        match self {
            Self::Move => {
                r#""x,y" coordinates (e.g. "7,7") to place a stone, or "pass" (go only) to pass"#
            }
            Self::Chat => "any message text to send to your opponent",
            Self::Request => r#""undo" | "reset" | "swap" — the negotiation request to send"#,
            Self::Confirm => r#""approve" | "reject" — your answer to the pending request"#,
            Self::Score => r#""ok" — confirm the scoring result (go, after both sides pass)"#,
            Self::Resign => "any content — submitting this file resigns the game",
        }
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
/// `/game/in/*` 不在此解析：暂存槽由 registry 的判别表先行拦截（read/write/submit
/// 的暂存分支各有专属语义），落到这里即按「槽不是普通文件」回专门的提示文案。
///
/// # Errors
/// 未知路径（文案含 /index 提示）；`/game/history/<n>` 的 n 非数字。
pub fn resolve(path: &str) -> Result<Resolved, String> {
    let p = path.trim();
    let p = p.strip_prefix('/').unwrap_or(p);
    match p {
        "index" => return Ok(Resolved::Index),
        // 暂存槽：resolve 的调用点都是「普通文件语义」的路径（registry 已把 in/
        // 分流走暂存分支），落到这里说明调用方拿槽当文件用了——给两段式的用法提示。
        "game/in/move" | "game/in/chat" | "game/in/request" | "game/in/confirm"
        | "game/in/score" | "game/in/resign" => {
            return Err(format!(
                "{path} is a staging slot, not an ordinary file: write to stage, submit(\"/{p}\") to commit."
            ));
        }
        _ => {}
    }
    if p == "memory" || p.starts_with("memory/") {
        // 相对路径原样上交（含空串= ns 根），规范化是 VfsStore 入口的职责。
        return Ok(Resolved::Memory(p.strip_prefix("memory/").unwrap_or("").to_string()));
    }
    if p == "game" || p.starts_with("game/") {
        let rest = p.strip_prefix("game/").unwrap_or("");
        let file = match rest {
            "status" => GameFile::Status,
            "board" => GameFile::Board,
            "board/grid" => GameFile::BoardGrid,
            "board/ascii" => GameFile::BoardAscii,
            "board/pretty" => GameFile::BoardPretty,
            "board/image.png" => GameFile::BoardImage,
            "rules" => GameFile::Rules,
            "history" => GameFile::History,
            "chat" => GameFile::Chat,
            "events" => GameFile::Events,
            _ => {
                if let Some(n) = rest.strip_prefix("history/") {
                    let n: u32 = n.parse().map_err(|_| {
                        format!("invalid history move number {n:?} in {path}. Use /game/history/<n> with n from 1 to the current move count (read /game/status), or plain /game/history for the list.")
                    })?;
                    if n == 0 {
                        return Err(
                            "history starts at move 1 — /game/history/0 does not exist. Read /game/history for the move list."
                                .to_string(),
                        );
                    }
                    GameFile::HistoryN(n)
                } else {
                    return Err(unknown_path(path));
                }
            }
        };
        return Ok(Resolved::Game(file));
    }
    Err(unknown_path(path))
}

/// 未知路径的人话错误（计划：错误附 /index 提示）。
fn unknown_path(path: &str) -> String {
    format!("unknown path {path:?}. Read /index for the full file map — /game/* (live game), /game/in/* (stage actions), /memory/* (persistent memory).")
}

/// 可写判定：`/game/in/*`（stage 语义）与 `/memory/**` 可写；
/// `/game` 其余全部动态文件与 `/index` 只读。
///
/// **为什么独立成函数而不是塞进 resolve**：read 的 offset/limit、grep 的前缀遍历、
/// write 的拒绝文案、submit 的路径判别各自只要其中一半事实，一个布尔谓词让每个
/// 调用点拿自己要的那一半，而不是背整个 Resolved match。
#[must_use]
pub fn is_writable(r: &Resolved) -> bool {
    matches!(r, Resolved::Memory(_))
}

/// 暂存着法在合成里的呈现形态：`/game/status` 的 `staged_move` 字段、
/// `/game/board` 的 `staged_move`、grid 里的 `"staged"` 格、ascii 的 `*`、
/// pretty 的 `◍` 全部由它驱动（计划：暂存落子上盘可见，人能提前看到 Agent 打算下哪）。
///
/// 取值时机：每次合成前从 [`Staging`] 现读——没有缓存层，暂存即盘面所见。
pub type StagedMove = CoordT;

/* ---------------- 动态合成器（输出形态照计划样例逐字） ---------------- */

/// 快照 → 棋盘格矩阵（`grid[y][x]`，值 empty|black|white），并把暂存格翻成
/// `"staged"`（越界暂存不翻——越界是对局规则，submit 才判，合成只画真实盘内点）。
fn grid_of(snap: &serde_json::Value, staged: Option<StagedMove>) -> Vec<Vec<String>> {
    let mut grid: Vec<Vec<String>> = snap["board"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r.as_array()
                        .map(|cells| {
                            cells.iter().map(|c| c.as_str().unwrap_or("empty").to_string()).collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(s) = staged {
        if let Some(cell) = grid.get_mut(s.y as usize).and_then(|row| row.get_mut(s.x as usize)) {
            *cell = "staged".into();
        }
    }
    grid
}

/// 执色按手序反推（协议 HistoryEntry 不带行棋方）：黑先、悔棋后序号从 1 重编，
/// 序数奇偶即行棋方——与 [`crate::player::diff_events`] 同一条规则。
fn color_of_ply(n: usize) -> &'static str {
    if n % 2 == 1 { "black" } else { "white" }
}

/// 对手展示名：聊天记录里第一条非本人条目的名字（快照没有独立的对手名字段，
/// 聊天记录是对手名字唯一的出现点）；还没聊过天 → null。
fn opponent_name(snap: &serde_json::Value) -> serde_json::Value {
    snap["chatLog"]
        .as_array()
        .and_then(|log| {
            log.iter()
                .find(|m| m["self"] == serde_json::Value::Bool(false))
                .and_then(|m| m["name"].as_str())
        })
        .map(|n| serde_json::json!(n))
        .unwrap_or(serde_json::Value::Null)
}

/// 待决请求的合成形态：`{kind, from}`（from 取展示名），无待决 → null。
fn pending_request(snap: &serde_json::Value) -> serde_json::Value {
    match snap["confirmReq"].as_object() {
        None => serde_json::Value::Null,
        Some(req) => {
            let from = req["fromName"].as_str().filter(|s| !s.is_empty())
                .or_else(|| req["from"].as_str()).unwrap_or_default();
            serde_json::json!({
                "kind": req["kind"],
                "from": from,
            })
        }
    }
}

/// `/game/status`。字段序与样例一致：kind/size/you/opponent/phase/to_move/
/// move_count/winner/scoring/pending_request/staged_move。
#[must_use]
pub fn syn_status(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    let staged_move = match staged {
        Some(c) => serde_json::json!({ "x": c.x, "y": c.y }).to_string(),
        None => "null".to_string(),
    };
    format!(
        r#"{{"kind":{},"size":{},"you":{},"opponent":{},"phase":{},"to_move":{},"move_count":{},"winner":{},"scoring":{},"pending_request":{},"staged_move":{}}}"#,
        snap["kind"], snap["size"], snap["myColor"], opponent_name(snap), snap["phase"],
        snap["toMove"], snap["moveCount"], snap["winner"], snap["scoring"],
        pending_request(snap), staged_move,
    )
}

/// `/game/board`（默认·稀疏 JSON，最省 token）：kind/size/you/to_move/move_count/
/// winner/last_move/stones{black,white}/staged_move。
#[must_use]
pub fn syn_board(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    let grid = grid_of(snap, staged);
    let (blacks, whites) = stones_of(&grid);
    // last_move 的 by 按手序反推（快照 lastMove 只有坐标）。
    let last_move = match snap["lastMove"].as_object() {
        Some(c) => {
            let by = color_of_ply(snap["moveCount"].as_u64().unwrap_or(0) as usize);
            serde_json::json!({ "by": by, "x": c["x"], "y": c["y"] }).to_string()
        }
        None => "null".to_string(),
    };
    let staged_move = match staged {
        Some(c) => serde_json::json!({ "x": c.x, "y": c.y }).to_string(),
        None => "null".to_string(),
    };
    format!(
        r#"{{"kind":{},"size":{},"you":{},"to_move":{},"move_count":{},"winner":{},"last_move":{last_move},"stones":{{"black":{},"white":{}}},"staged_move":{staged_move}}}"#,
        snap["kind"], snap["size"], snap["myColor"], snap["toMove"], snap["moveCount"],
        snap["winner"], serde_json::json!(blacks), serde_json::json!(whites),
    )
}

/// 从格矩阵收双方面坐标表（行优先遍历——与引擎落子顺序无关，只求稳定可复现）。
fn stones_of(grid: &[Vec<String>]) -> (Vec<[u16; 2]>, Vec<[u16; 2]>) {
    let mut blacks = Vec::new();
    let mut whites = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            match cell.as_str() {
                "black" => blacks.push([x as u16, y as u16]),
                "white" => whites.push([x as u16, y as u16]),
                _ => {}
            }
        }
    }
    (blacks, whites)
}

/// `/game/board/grid`：`grid[y][x]` 二维数组，每格 `"empty"|"black"|"white"|"staged"`
/// ——不分 stones/staged，整盘每格都有描述（模拟棋盘的 JSON 版，有时更直观）。
#[must_use]
pub fn syn_board_grid(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    let grid = grid_of(snap, staged);
    format!(
        r#"{{"kind":{},"size":{},"grid":{}}}"#,
        snap["kind"], snap["size"], serde_json::json!(grid),
    )
}

/// `/game/board/ascii`：单空格分隔，列头/行头坐标数字，`X=black O=white *=staged . =empty`。
#[must_use]
pub fn syn_board_ascii(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    let grid = grid_of(snap, staged);
    ascii_of(snap, &grid)
}

/// ASCII 渲染本体（board 与 history/<n> 共用）。布局照计划样例：
/// 首行 `{size}x{size} {kind}` + 图例；列头 4 空格起、单空格分隔；行头右对齐
/// 2 字符 + 2 空格；一位数列单空格分隔、两位数列各带一个前导空格（与列头对齐）。
fn ascii_of(snap: &serde_json::Value, grid: &[Vec<String>]) -> String {
    let n = grid.len();
    let size = snap["size"].as_u64().unwrap_or(n as u64);
    let kind = snap["kind"].as_str().unwrap_or("");
    let sym = |cell: &str| match cell {
        "black" => "X",
        "white" => "O",
        "staged" => "*",
        _ => ".",
    };
    let mut out = format!("{size}x{size} {kind}   X=black O=white *=staged . =empty\n");
    // 列头：4 空格 + 数字单空格连接（两位数天然占两格，与数据行的前导空格对齐）。
    let header: Vec<String> = (0..n).map(|c| c.to_string()).collect();
    out.push_str(&format!("    {}\n", header.join(" ")));
    for (y, row) in grid.iter().enumerate() {
        // 行头右对齐 2 字符 + 2 空格（样例 " 0  " / " 7  "）。
        out.push_str(&format!("{y:>2}  "));
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(x, cell)| if x >= 10 { format!(" {}", sym(cell)) } else { sym(cell).to_string() })
            .collect();
        out.push_str(&cells.join(" "));
        out.push('\n');
    }
    out
}

/// `/game/board/pretty`：标准制表符 `┌┬┐├┼┤└┴┘` 画围棋式交叉点盘，`●`黑 `○`白 `◍`暂存。
#[must_use]
pub fn syn_board_pretty(snap: &serde_json::Value, staged: Option<StagedMove>) -> String {
    let grid = grid_of(snap, staged);
    pretty_of(&grid)
}

/// pretty 渲染本体：n×n 交叉点，符号间以 `─` 相连；角/边/中的框线按行位置选。
fn pretty_of(grid: &[Vec<String>]) -> String {
    let n = grid.len();
    let sym = |cell: &str| match cell {
        "black" => "●",
        "white" => "○",
        "staged" => "◍",
        _ => "",
    };
    let corner = |y: usize, x: usize| -> &str {
        let (top, left, right, bottom) = (y == 0, x == 0, x + 1 == n, y + 1 == n);
        match (top, bottom, left, right) {
            (true, _, true, _) => "┌",
            (true, _, _, true) => "┐",
            (_, true, true, _) => "└",
            (_, true, _, true) => "┘",
            (true, _, _, _) => "┬",
            (_, true, _, _) => "┴",
            (_, _, true, _) => "├",
            (_, _, _, true) => "┤",
            _ => "┼",
        }
    };
    let mut out = String::new();
    for (y, row) in grid.iter().enumerate() {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(x, cell)| {
                let s = sym(cell);
                if s.is_empty() { corner(y, x).to_string() } else { s.to_string() }
            })
            .collect();
        out.push_str(&line.join("─"));
        out.push('\n');
    }
    out
}

/// `/game/board/image.png`：PNG 栅格（网格线+黑白圆子+暂存空线圈，无字体无坐标
/// 标签——精确坐标以 JSON 为准）。编码失败回空 Vec（内存 writer 实际不可失败，
/// 调用方按空值回占位文本，绝不 panic 炸掉工具执行）。
///
/// **承载能力矩阵**（计划）：Anthropic tool_result 原生 image block 可；
/// MCP `CallToolResult type:"image"` 可；OpenAI 两协议工具结果纯字符串不可
/// → 占位符文本（由 registry 按协议能力回，见 [`syn_image_placeholder`]）。
#[must_use]
pub fn syn_board_png(snap: &serde_json::Value, staged: Option<StagedMove>) -> Vec<u8> {
    let grid = grid_of(snap, staged);
    let n = grid.len() as u32;
    if n == 0 {
        return Vec::new();
    }
    // 布局：margin 边距 + cell 交叉点间距；圆子半径取 cell 的四成（相邻不粘连）。
    const CELL: u32 = 32;
    const MARGIN: u32 = 16;
    let dim = MARGIN * 2 + CELL * (n - 1);
    let mut img = vec![255u8; (dim * dim * 3) as usize];
    let mut put = |x: u32, y: u32, rgb: [u8; 3]| {
        if x < dim && y < dim {
            let i = ((y * dim + x) * 3) as usize;
            img[i..i + 3].copy_from_slice(&rgb);
        }
    };
    // 网格线（行/列各 n 条，贯穿边距内全域）。
    for i in 0..n {
        let p = MARGIN + CELL * i;
        for t in 0..=(CELL * (n - 1)) {
            put(p, MARGIN + t, [70, 70, 70]);
            put(MARGIN + t, p, [70, 70, 70]);
        }
    }
    // 圆盘填充（黑实心、白实心描灰边）；暂存 = 空心圈（计划：虚线圈，光栅上以
    // 环带近似——PNG 无字体无反锯齿，环带已是可辨识的最简形态）。
    let draw_disc = |put: &mut dyn FnMut(u32, u32, [u8; 3]), cx: i64, cy: i64, r: i64, rgb: [u8; 3]| {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    put((cx + dx).max(0) as u32, (cy + dy).max(0) as u32, rgb);
                }
            }
        }
    };
    let draw_ring = |put: &mut dyn FnMut(u32, u32, [u8; 3]), cx: i64, cy: i64, r: i64, rgb: [u8; 3]| {
        for dy in -r..=r {
            for dx in -r..=r {
                let d = dx * dx + dy * dy;
                if d <= r * r && d >= (r - 3) * (r - 3) {
                    put((cx + dx).max(0) as u32, (cy + dy).max(0) as u32, rgb);
                }
            }
        }
    };
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            let (cx, cy) = ((MARGIN + CELL * x as u32) as i64, (MARGIN + CELL * y as u32) as i64);
            let r = (CELL as i64) * 2 / 5;
            match cell.as_str() {
                "black" => draw_disc(&mut put, cx, cy, r, [20, 20, 20]),
                "white" => {
                    draw_disc(&mut put, cx, cy, r, [250, 250, 250]);
                    draw_ring(&mut put, cx, cy, r, [120, 120, 120]);
                }
                "staged" => draw_ring(&mut put, cx, cy, r, [200, 60, 30]),
                _ => {}
            }
        }
    }
    let mut png_data = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_data, dim, dim);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        // 内存 writer 的两步不可能失败（无 IO、尺寸已在界内）——失败即骨架 bug，
        // 回空 Vec 让调用方走占位文本路径，绝不 panic 进工具执行。
        match enc.write_header() {
            Ok(mut w) => {
                if w.write_image_data(&img).is_err() {
                    return Vec::new();
                }
            }
            Err(_) => return Vec::new(),
        }
    }
    png_data
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
    let size = snap["size"].as_u64().unwrap_or(15);
    match snap["kind"].as_str() {
        Some("gomoku") => format!(
            "# Gomoku rules ({size}x{size})\n\n\
             - Black moves first; players alternate placing one stone on any empty intersection.\n\
             - The first player to get **five of their own stones in a row** — horizontal, \
             vertical, or diagonal — wins immediately.\n\
             - Stones are never moved or captured; the board only fills up.\n\
             - A full board with no five-in-a-row is a draw.\n\
             - Passing is not a move in gomoku: to act, place a stone. To give up, resign via \
             `/game/in/resign`.\n\
             - Undo/reset are negotiation requests (`/game/in/request`) — they only take effect \
             when the opponent approves.\n"
        ),
        Some("go") => format!(
            "# Go rules ({size}x{size}, Chinese rules)\n\n\
             - Black moves first; players alternate placing one stone on any empty intersection, \
             or pass.\n\
             - A group with no adjacent empty point (liberty) is captured and removed. Suicide \
             is banned. The simple ko rule forbids immediately recapturing at a ko point.\n\
             - **Two consecutive passes** end the game: scoring starts, the human marks dead \
             stones, and both players confirm. You confirm via write `/game/in/score` \
             (\"ok\") + submit.\n\
             - Scoring is by area: a player's score = their stones + surrounded empty territory. \
             Komini is not applied in local play.\n\
             - Undo/reset are negotiation requests (`/game/in/request`) — they only take effect \
             when the opponent approves.\n"
        ),
        _ => concat!(
            "# Rules\n\nUnknown game kind — the engine enforces the rules; invalid actions are ",
            "rejected at submit time with the reason.\n"
        )
        .to_string(),
    }
}

/// `/game/history`：JSONL 逐行 `{"n":..,"by":"..","x":..,"y":..}`（pass 行记
/// `"pass":true`，无坐标字段）。来源是快照 history 数组（含序号重编——history
/// 永远从 1 起，悔棋后自然回退）。
#[must_use]
pub fn syn_history(snap: &serde_json::Value) -> String {
    let mut out = String::new();
    for (i, entry) in snap["history"].as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().enumerate() {
        let n = i + 1;
        let by = color_of_ply(n);
        if entry.is_string() {
            out.push_str(&format!(r#"{{"n":{n},"by":"{by}","pass":true}}"#));
        } else {
            out.push_str(&format!(
                r#"{{"n":{n},"by":"{by}","x":{},"y":{}}}"#,
                entry["x"], entry["y"],
            ));
        }
        out.push('\n');
    }
    out
}

/// `/game/history/<n>`：第 n 手后的局面，变体与 board 同款。
///
/// 实现路数：重放 history 前 n 手到空盘（围棋提子/禁着经规则引擎，不能手搓棋盘
/// 数组——提子后的盘面手搓必错），再按变体渲染。n 越界（0 或 > 总手数）回 Err。
///
/// # Errors
/// n 非法（越界/为 0）；棋种与 history 不匹配（理论不可达，防御到错误文案）。
pub fn syn_history_n(snap: &serde_json::Value, n: u32, variant: HistoryVariant) -> Result<String, String> {
    let history = snap["history"].as_array().ok_or_else(|| {
        format!("no game history in the current snapshot — /game/history/{n} needs a live game.")
    })?;
    if n == 0 || n as usize > history.len() {
        return Err(format!(
            "/game/history/{n} is out of range: the game has {} moves (1..={}).",
            history.len(),
            history.len().max(1)
        ));
    }
    // 重放走真规则引擎：围棋的提子/劫禁着由核心层判定，手搓数组在提子局面必错
    //（合成器骨架注释钉死的实现路数）。kind+size 经 make_engine_kind 与会话同一
    // 条守卫（非法组合回退默认，与真人主页选棋种同款）。
    let kind_str = snap["kind"].as_str().unwrap_or("gomoku");
    let size = snap["size"].as_u64().unwrap_or(15) as u16;
    let mut engine = goptop_core::game::GameState::new(goptop_net::session::make_engine_kind(kind_str, size));
    let mut engine_ok = true;
    for entry in &history[..n as usize] {
        let mv = if entry.is_string() {
            goptop_core::game::Move::Pass
        } else {
            goptop_core::game::Move::Place(goptop_core::board::Coord::new(
                entry["x"].as_u64().unwrap_or(0) as u8,
                entry["y"].as_u64().unwrap_or(0) as u8,
            ))
        };
        // 中途被拒（理论不可达：历史本身是引擎收下的序列）——防御性翻标志，
        // 渲染仍给出已重放到的那一步的局面，不让一个坏手炸掉整份合成。
        if engine.try_play(mv).is_err() {
            engine_ok = false;
        }
    }
    // 引擎盘是物理 15/19，逻辑尺寸取会话 size——只读前 n 路有效区域。
    let mut grid: Vec<Vec<String>> = (0..size as usize)
        .map(|y| {
            (0..size as usize)
                .map(|x| match engine.board.get(goptop_core::board::Coord::new(x as u8, y as u8)) {
                    Some(goptop_core::board::Stone::Black) => "black".to_string(),
                    Some(goptop_core::board::Stone::White) => "white".to_string(),
                    _ => "empty".to_string(),
                })
                .collect()
        })
        .collect();
    if !engine_ok {
        grid.clear();
    }
    // 局面子视图借用会话级快照的 kind/size（JSON 头字段与 board 同款）。
    let head = serde_json::json!({ "kind": snap["kind"], "size": snap["size"] });
    match variant {
        HistoryVariant::Json => {
            let (blacks, whites) = stones_of(&grid);
            let last = history[n as usize - 1].clone();
            let last_move = if last.is_string() {
                serde_json::json!({ "by": color_of_ply(n as usize), "pass": true }).to_string()
            } else {
                serde_json::json!({ "by": color_of_ply(n as usize), "x": last["x"], "y": last["y"] })
                    .to_string()
            };
            Ok(format!(
                r#"{{"kind":{},"size":{},"after_move":{n},"to_move":{},"last_move":{last_move},"stones":{{"black":{},"white":{}}}}}"#,
                head["kind"], head["size"],
                if n % 2 == 0 { serde_json::json!("black") } else { serde_json::json!("white") },
                serde_json::json!(blacks), serde_json::json!(whites),
            ))
        }
        HistoryVariant::Grid => Ok(format!(
            r#"{{"kind":{},"size":{},"after_move":{n},"grid":{}}}"#,
            head["kind"], head["size"], serde_json::json!(grid),
        )),
        HistoryVariant::Ascii => Ok(ascii_of(&head, &grid)),
        HistoryVariant::Pretty => Ok(pretty_of(&grid)),
    }
}

/// `/game/chat`：聊天记录散文逐行（`名字: 文本`；含自己发的）。
/// 长记录靠 read 的 offset/limit 翻页，不在这里截断。
#[must_use]
pub fn syn_chat(snap: &serde_json::Value) -> String {
    let mut out = String::new();
    for m in snap["chatLog"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let name = m["name"].as_str().unwrap_or("?");
        let text = m["text"].as_str().unwrap_or_default();
        out.push_str(name);
        out.push_str(": ");
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// `/game/events`：事件队列全量历史的 JSONL（seq 单调）。
/// 与 [`EventQueue::history_jsonl`] 同源的直通口——本函数存在的意义是把
/// 「events 文件从哪来」钉在 vfs 的合成器清单里，registry 不必知道队列内部。
#[must_use]
pub fn syn_events(queue: &EventQueue) -> String {
    queue.history_jsonl()
}

/// `/index`：全部路径索引与一句话说明（计划里有逐字基线，照抄不发挥）。
/// **动态段**（/game/status 等）在运行期不变，可直接整段返回常量文本。
#[must_use]
pub fn syn_index() -> String {
    // 计划「/game/chat（散文逐行）、/game/rules（markdown）、/index（文档）」节的
    // 逐字基线；模型的第一眼文件地图，改工具面时此处与工具描述同步改。
    r#"# Virtual filesystem index
Read-only (synthesized from live game):
  /game/status     turn, colors, winner, scoring, pending request, staged move
  /game/board      current board (default JSON, cheapest) — variants: /grid /ascii /pretty /image.png
  /game/rules      rules of the current game kind
  /game/history    move list (JSONL)  |  /game/history/<n> = position after move n (same five variants)
  /game/chat       chat log (plain lines; long → read with offset/limit)
  /game/events     full event history (JSONL, seq-increasing)
Stage-then-submit (write to stage, submit(path) to commit):
  /game/in/move     "x,y" or "pass"          → submit places your stone
  /game/in/chat     message text             → submit sends it
  /game/in/request  "undo"|"reset"|"swap"    → submit sends the request
  /game/in/confirm  "approve"|"reject"       → submit answers pending request
  /game/in/score    "ok"                     → submit confirms scoring
  /game/in/resign   (anything)               → submit resigns
Persistent memory (survives across games):
  /memory/...       free-form files, e.g. /memory/notes/opponent-style.md
"#
    .to_string()
}

/// 动态文件的统一读口：按解析结果分派到合成器。
///
/// 只吃 `/game` 的合成文件——in/ 暂存槽在 registry 的 read/grep 里先行分流
/// （read 回显暂存内容/空槽职责说明），到不了这里：`resolve` 对槽路径本就不产
/// [`Resolved`]（骨架的「in/ 也走这里」契约与 Resolved 无槽变体自相矛盾，集成期
/// 把矛盾收敛到 registry 一侧——暂存读取只有「回显原文」一种形态，两处实现反而
/// 会漂移）。`/game/board/image.png` 在文本读口拒绝，回 [`syn_image_placeholder`]
/// ——图像只经协议适配器走 [`syn_board_png`]。
///
/// # Errors
/// HistoryN 越界（[`syn_history_n`] 透传）；未知路径（[`resolve`] 透传）。
pub fn read_dynamic(
    r: &Resolved,
    snap: &serde_json::Value,
    staged: Option<StagedMove>,
    queue: &EventQueue,
) -> Result<String, String> {
    let Resolved::Game(file) = r else {
        return Err("not a /game dynamic file — resolve() only routes /game here.".into());
    };
    Ok(match file {
        GameFile::Status => syn_status(snap, staged),
        GameFile::Board => syn_board(snap, staged),
        GameFile::BoardGrid => syn_board_grid(snap, staged),
        GameFile::BoardAscii => syn_board_ascii(snap, staged),
        GameFile::BoardPretty => syn_board_pretty(snap, staged),
        // 图像不进文本读口：占位文本即拒绝理由，模型照它改读 ascii。
        GameFile::BoardImage => return Err(syn_image_placeholder().to_string()),
        GameFile::Rules => syn_rules(snap),
        GameFile::History => syn_history(snap),
        GameFile::HistoryN(n) => syn_history_n(snap, *n, HistoryVariant::Json)?,
        GameFile::Chat => syn_chat(snap),
        GameFile::Events => syn_events(queue),
    })
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
        // 各槽先按协议校验（格式错在此拒、绝不入槽），再存原始文本。
        let value = match file {
            InFile::Move => {
                let t = content.trim();
                if t == "pass" {
                    serde_json::json!({ "pass": true })
                } else if let Some((x, y)) = parse_coord(t) {
                    serde_json::json!({ "x": x, "y": y })
                } else {
                    return Err(format!(
                        "invalid move {content:?}. Expected \"x,y\" (e.g. \"7,7\") or \"pass\"."
                    ));
                }
            }
            InFile::Chat => {
                let t = content.trim();
                if t.is_empty() {
                    return Err(
                        "chat message is empty — write the text you want to send, then submit."
                            .into(),
                    );
                }
                serde_json::json!(content)
            }
            InFile::Request => {
                let t = content.trim();
                if matches!(t, "undo" | "reset" | "swap") {
                    serde_json::json!(t)
                } else {
                    return Err(format!(
                        "invalid request {content:?}. Expected \"undo\", \"reset\" or \"swap\"."
                    ));
                }
            }
            InFile::Confirm => {
                let t = content.trim();
                if matches!(t, "approve" | "reject") {
                    serde_json::json!(t)
                } else {
                    return Err(format!(
                        "invalid confirm {content:?}. Expected \"approve\" or \"reject\"."
                    ));
                }
            }
            InFile::Score => {
                let t = content.trim();
                if t == "ok" {
                    serde_json::json!("ok")
                } else {
                    return Err(format!("invalid score {content:?}. Expected \"ok\"."));
                }
            }
            // 认输：任意内容（含空串）——写即暂存，submit 才生效。
            InFile::Resign => serde_json::json!(content),
        };
        let note = match file {
            InFile::Move => "call submit(\"/game/in/move\") to place",
            InFile::Chat => "call submit(\"/game/in/chat\") to send",
            InFile::Request => "call submit(\"/game/in/request\") to send the request",
            InFile::Confirm => "call submit(\"/game/in/confirm\") to answer",
            InFile::Score => "call submit(\"/game/in/score\") to confirm scoring",
            InFile::Resign => "call submit(\"/game/in/resign\") to resign",
        };
        self.lock()
            .insert(file.path(), content.to_string());
        Ok(serde_json::json!({
            "ok": true,
            "staged": { "path": file.path(), "value": value },
            "note": note,
        }))
    }

    /// 回看暂存内容（read /game/in/* 的取数口）；空槽回 None。
    #[must_use]
    pub fn peek(&self, file: InFile) -> Option<String> {
        self.lock().get(file.path()).cloned()
    }

    /// 提交取走（取走即清槽——同一份内容不允许二次 submit；第二次回
    /// 「无内容」级错误，提示先 write 再 submit）。
    pub fn take(&self, file: InFile) -> Option<String> {
        self.lock().remove(file.path())
    }

    /// 锁入口：临界区只有 HashMap 增删查，无 await。中毒即持锁方 panic（bug），
    /// 暂存区是单值小结构，into_inner 硬闯比把毒扩散成整局失败稳。
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<&'static str, String>> {
        self.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 解析 `"x,y"` 坐标文本（两段 u16，容许空白）；残缺/负数/多段一律 None。
fn parse_coord(t: &str) -> Option<(u16, u16)> {
    let (x, y) = t.split_once(',')?;
    let x = x.trim().parse::<u16>().ok()?;
    let y = y.trim().parse::<u16>().ok()?;
    Some((x, y))
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
/// **对局规则错误在本函数前置拒**：状态机对非法命令是静默 no-op（can_place 返回
/// 假即空效应，调用面无返回值可查），所以 submit 在发命令**前**用快照复算同一条
/// 判定并回人话错误——错误文案照计划样例逐字；发命令后再核对实际生效（moveCount
/// /history 增长），核对失败回防御性错误而不是谎报成功。
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
    let Some(content) = staging.take(file) else {
        return Err(format!(
            "nothing staged in {}. Write it first, then submit the path.",
            file.path()
        ));
    };
    let mut snap = player.snapshot();
    match file {
        InFile::Move => {
            let t = content.trim();
            if t == "pass" {
                // pass 是围棋的动作：五子棋引擎不收停一手，直接按规则错误回话。
                if snap["kind"].as_str() != Some("go") {
                    return Err(format!(
                        "\"pass\" is not a move in {} — write coordinates instead. Read /game/rules.",
                        snap["kind"].as_str().unwrap_or("this game")
                    ));
                }
                check_my_turn(&snap)?;
                let before = hist_len(&snap);
                player.cmd(UiCommand::Pass);
                snap = settle_and_reread(player, settle_ms).await;
                if hist_len(&snap) <= before {
                    return Err("the pass was not accepted — read /game/status and retry.".into());
                }
                return Ok(serde_json::json!({
                    "ok": true, "action": "pass", "move_count": hist_len(&snap),
                    "scoring": snap["scoring"], "finished": !snap["winner"].is_null(),
                }));
            }
            // 坐标着法：槽内容在 write 时已过格式校验，这里残缺只可能是槽被绕过
            // 写入的防御路径——按格式错误回话（模型读到的是自己写进去的原文）。
            let Some((x, y)) = parse_coord(t) else {
                return Err(format!(
                    "invalid move {content:?}. Expected \"x,y\" (e.g. \"7,7\") or \"pass\"."
                ));
            };
            check_place_allowed(&snap, x, y)?;
            let before = hist_len(&snap);
            player.cmd(UiCommand::Place { x, y });
            snap = settle_and_reread(player, settle_ms).await;
            let after = hist_len(&snap);
            if after <= before {
                return Err(format!(
                    "the move ({x},{y}) was not accepted — the board may have changed; re-read /game/board."
                ));
            }
            Ok(serde_json::json!({
                "ok": true, "action": "move", "move_count": after,
                "finished": !snap["winner"].is_null(),
            }))
        }
        InFile::Chat => {
            player.cmd(UiCommand::SendChat(content.clone()));
            snap = settle_and_reread(player, settle_ms).await;
            // 回执以自己快照的 chatLog 末条对账（含本人条目即已入流并广播）。
            let last = snap["chatLog"].as_array().and_then(|l| l.last());
            if last.and_then(|m| m["text"].as_str()) != Some(content.as_str()) {
                return Err("the chat message was not delivered — retry once.".into());
            }
            Ok(serde_json::json!({ "ok": true, "action": "chat", "sent": content }))
        }
        InFile::Request => {
            let kind = content.trim().to_string();
            if snap["phase"].as_str() != Some("playing") {
                return Err(format!(
                    "not in a live game (phase {:?}) — a negotiation request needs an opponent at the table.",
                    snap["phase"].as_str().unwrap_or("?")
                ));
            }
            match kind.as_str() {
                "undo" => player.cmd(UiCommand::RequestUndo),
                "reset" => player.cmd(UiCommand::RequestReset),
                "swap" => player.cmd(UiCommand::RequestSwap),
                _ => return Err(format!(
                    "invalid request {content:?}. Expected \"undo\", \"reset\" or \"swap\"."
                )),
            }
            settle_and_reread(player, settle_ms).await;
            Ok(serde_json::json!({
                "ok": true, "action": "request", "kind": kind,
                "note": "sent — the outcome arrives as a pushed event (request_resolved).",
            }))
        }
        InFile::Confirm => {
            // 批复必须有待决请求：状态机对空弹窗的 Confirm 是静默 no-op，前置拒
            //（计划样例文案逐字）。
            if snap["confirmReq"].is_null() {
                return Err("no pending request. Check pending_request in /game/status.".into());
            }
            let kind = snap["confirmReq"]["kind"].as_str().unwrap_or_default().to_string();
            let approved = content.trim() == "approve";
            match content.trim() {
                "approve" => player.cmd(UiCommand::ConfirmApprove),
                _ => player.cmd(UiCommand::ConfirmDecline),
            }
            settle_and_reread(player, settle_ms).await;
            Ok(serde_json::json!({
                "ok": true, "action": "confirm", "kind": kind, "approved": approved,
                "note": "answered — the effect (history change / color swap) shows in /game/status.",
            }))
        }
        InFile::Score => {
            // 计分确认只在计分态且我方未确认时合法（非计分态 submit 失败——计划测试项）。
            if snap["scoring"].as_bool() != Some(true) {
                return Err(
                    "not in scoring — scoring starts after both sides pass in go. Check scoring in /game/status.".into(),
                );
            }
            if snap["myScoreOk"].as_bool() == Some(true) {
                return Err("you already confirmed scoring — waiting for the opponent.".into());
            }
            player.cmd(UiCommand::ConfirmScore);
            snap = settle_and_reread(player, settle_ms).await;
            let mut receipt = serde_json::json!({
                "ok": true, "action": "score", "my_confirm": true,
                "finished": !snap["winner"].is_null(),
            });
            if !snap["scoreResult"].is_null() {
                receipt["score_result"] = snap["scoreResult"].clone();
            }
            Ok(receipt)
        }
        InFile::Resign => {
            if snap["phase"].as_str() != Some("playing") || !snap["winner"].is_null() {
                return Err(format!(
                    "no live game to resign (phase {:?}, winner {}).",
                    snap["phase"].as_str().unwrap_or("?"),
                    snap["winner"]
                ));
            }
            player.cmd(UiCommand::Resign);
            snap = settle_and_reread(player, settle_ms).await;
            Ok(serde_json::json!({
                "ok": true, "action": "resign", "finished": true, "winner": snap["winner"],
            }))
        }
    }
}

/// submit 后的定拍：自泵若干拍再重读快照（对手同步与事件生成需要几拍传播）。
async fn settle_and_reread(player: &dyn PlayerHandle, settle_ms: u64) -> serde_json::Value {
    let rounds = (settle_ms / 50).max(1);
    for _ in 0..rounds {
        player.pump();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    player.pump();
    player.snapshot()
}

/// 快照 history 长度（坏快照按 0——与合成器的兜底同一口径）。
fn hist_len(snap: &serde_json::Value) -> usize {
    snap["history"].as_array().map_or(0, |a| a.len())
}

/// 落子前的对局规则复算（can_place 的人话版——状态机静默拒绝，错误在这里给）。
fn check_place_allowed(snap: &serde_json::Value, x: u16, y: u16) -> Result<(), String> {
    if snap["phase"].as_str() != Some("playing") {
        return Err(format!(
            "not in a live game (phase {:?}) — read /game/status.",
            snap["phase"].as_str().unwrap_or("?")
        ));
    }
    check_my_turn(snap)?;
    let n = snap["size"].as_u64().unwrap_or(0) as u16;
    if x >= n || y >= n {
        return Err(format!(
            "({x},{y}) is off the {n}x{n} board. Choose an empty intersection — read /game/board."
        ));
    }
    let cell = snap["board"][y as usize][x as usize].as_str().unwrap_or("empty");
    if cell != "empty" {
        return Err(format!(
            "({x},{y}) is occupied by {cell}. Choose an empty intersection — read /game/board."
        ));
    }
    if snap["peerConnected"].as_bool() != Some(true)
        && snap["relayAvailable"].as_bool() != Some(true)
    {
        return Err(
            "not connected to your opponent — the pairing is down; wait for reconnect or stop."
                .into(),
        );
    }
    Ok(())
}

/// 轮次复算（计划样例文案逐字：`not your turn (black to move). Opponent action
/// arrives as a pushed event.`）。
fn check_my_turn(snap: &serde_json::Value) -> Result<(), String> {
    let to_move = snap["toMove"].as_str().unwrap_or("?");
    if snap["myColor"].as_str() != Some(to_move) {
        return Err(format!(
            "not your turn ({to_move} to move). Opponent action arrives as a pushed event."
        ));
    }
    Ok(())
}
