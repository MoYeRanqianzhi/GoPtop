//! 工具定义 —— 纯数据（计划「工具清单」节的 9 个工具）。
//!
//! 执行在 [`crate::registry`]；这里只有「长什么样、给谁用」的静态事实，供
//! MCP tools/list、三协议的工具面与 /index 的冗余文案共用。**不引 schemars**：
//! input_schema 是手写 JSON Schema 文本一份喂 rmcp 与三协议（计划拍板——lock 里
//! schemars 0.8/1.x 并存，不添乱）。
//!
//! **工具描述里写全各文件的功能职责与用法**（in/ 各文件的取值协议、只读文件的
//! 内容结构都进 description——模型不看 /index 也能用对，计划「文档双轨」节：
//! 提示词、/index、工具描述三处互为冗余）。in/ 取值协议的措辞与
//! [`crate::vfs::InFile::accepted`] 保持同一口径；名字/只读标注/适用面已定死，
//! 实现方不得改名（MCP tools/list 与三协议的工具名都以这里的常量为准）。

use crate::Driver;

/// 工具适用面 —— 两模式共用一张 registry，按驱动过滤。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolScope {
    /// 内置与 MCP 都在册。
    Shared,
    /// 仅内置（delegate：子代理需要同 LLM，外部 Agent 没有意义）。
    BuiltinOnly,
    /// 仅 MCP（wait_events/game_start/game_leave：内置模式事件自动推送、
    /// 席位由用户开局动作建立，这三者对外部 Agent 才有意义）。
    McpOnly,
}

/// 一个工具的静态定义。
///
/// `&'static str` 而非 String/Value：整个清单是编译期常量（`TOOLS`），MCP 出口、
/// 三协议、/index 三处共享零拷贝；schema 的合法由工具负责人手写自证
/// （坏 schema 在协议侧立即显形，运行期不再校验第二遍）。
#[derive(Clone, Copy, Debug)]
pub struct ToolDef {
    /// 工具名（协议面唯一标识；改名=外部 Agent 客户端全断）。
    pub name: &'static str,
    /// 工具描述（各文件职责与用法写全；与提示词、/index 三处互为冗余）。
    pub description: &'static str,
    /// 手写 JSON Schema（对象形 `{"type":"object","properties":{...}}`）。
    pub input_schema: &'static str,
    /// 只读标注（MCP tools/list 的 annotations；read/grep 为 true）。
    pub read_only: bool,
    /// 适用面。
    pub scope: ToolScope,
}

/// `read`：path + offset（起始行）+ limit（最多行数）——防长文件（聊天记录/history）
/// 撑爆上下文；返回含 total_lines 供翻页。动态文件即时合成。
pub const TOOL_READ: ToolDef = ToolDef {
    name: "read",
    description: r#"Read a virtual file. The file map (read /index first):
- /game/* — read-only, synthesized live from the game: /game/status (JSON: kind/size/your color/opponent/phase/to_move/move_count/winner/scoring/pending_request/staged_move), /game/board (sparse JSON, cheapest), /game/board/grid | /ascii | /pretty | /image.png (board renderings), /game/rules, /game/history (JSONL move list), /game/history/<n> (position after move n, same variants), /game/chat (chat log, plain lines), /game/events (full event history, JSONL).
- /game/in/* — shows what you currently staged there (nothing staged: explains the slot's protocol).
- /memory/<path> — your persistent memory files.
Paging: offset = 0-based first line to return (default 0), limit = max lines (default 2000) — page through long files (/game/chat, /game/history, /game/events) instead of reading them whole. Returns {ok, path, total_lines, content}; combine total_lines with offset to fetch the rest. Image variants (/game/board/image.png, /game/history/<n>/image.png) arrive as a PNG image when your model supports vision; otherwise you get a placeholder telling you to use /game/board/ascii instead."#,
    input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"absolute virtual path, e.g. /game/board, /game/history/3, /memory/notes/style.md"},"offset":{"type":"integer","minimum":0,"description":"0-based first line to return (default 0)"},"limit":{"type":"integer","minimum":0,"description":"max lines to return (default 2000)"}},"required":["path"]}"#,
    read_only: true,
    scope: ToolScope::Shared,
};

/// `write`：path + content（整体覆盖）。对 `/game/in/*` 写=**暂存**（只做格式校验
/// 并回预览，不执行）；对只读路径写→拒绝；对 /memory 写=持久。
pub const TOOL_WRITE: ToolDef = ToolDef {
    name: "write",
    description: r#"Write a virtual file (whole-file overwrite; use edit for in-place changes in /memory).
- /game/in/* = STAGE an action. Two-phase: write only stages (format-checked, previewed, board shows the staged stone), submit(path) commits. You may rewrite a slot any number of times before submitting. Value protocols:
    /game/in/move     "x,y" (e.g. "7,7") or "pass" (go) — your move
    /game/in/chat     message text — what to say in chat
    /game/in/request  "undo" | "reset" | "swap" — negotiation request to the opponent
    /game/in/confirm  "approve" | "reject" — answer the pending request (see /game/status pending_request)
    /game/in/score    "ok" — confirm scoring after both sides pass in go
    /game/in/resign   any content — resign
  Receipt: {ok:true, staged:{path,value}, note:"call submit(\"...\") to ..."}.
- /memory/<path> = persistent memory, survives across games (e.g. opponent style notes). Path rules: no "..", no empty segments, <=256 chars. Limits: 256KB per file, 8MB total — on quota errors delete old files first. Receipt: {ok:true, path, bytes, usage}.
- everything else (/game/status, /game/board*, /game/rules, /game/history*, /game/chat, /game/events, /index) is read-only and rejected.
Format errors (e.g. invalid move text) are rejected at write; game-rule errors (not your turn, occupied point) surface at submit."#,
    input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"absolute virtual path: /game/in/move|chat|request|confirm|score|resign to stage an action (then submit(path) to commit — submit clears the slot), or /memory/<path> for your persistent long-term memory that survives across games and sessions (takes effect immediately, no submit needed)"},"content":{"type":"string","description":"whole new file content"}},"required":["path","content"]}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `submit`：path（按路径自动判别提交什么：in/move→落子、in/chat→发消息、
/// in/request→发起请求、in/confirm→批复、in/score→确认计分、in/resign→认输）。
pub const TOOL_SUBMIT: ToolDef = ToolDef {
    name: "submit",
    description: r#"Commit what you staged — THE action verb; write alone never plays.
Path discrimination (automatic): /game/in/move -> place the stone (or pass in go), /game/in/chat -> send the message, /game/in/request -> send the undo/reset/swap request, /game/in/confirm -> answer the pending request, /game/in/score -> confirm scoring, /game/in/resign -> resign (this terminates your loop — only when appropriate).
Game-rule errors are rejected HERE, not at write: not your turn, point occupied, no pending request, not in scoring — the error tells you what to read (/game/status, /game/board) to recover; read it, adjust, re-stage. Submitting a slot you never wrote is an error — write first. Returns e.g. {ok:true, action:"move", move_count:2, finished:false}."#,
    input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"one of /game/in/move, /game/in/chat, /game/in/request, /game/in/confirm, /game/in/score, /game/in/resign — what you staged decides what gets committed"}},"required":["path"]}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `edit`：path + old_string + new_string + replace_all?=false（仅 /memory；
/// 0 次或 >1 次不唯一→报错回模型，Claude Code 语义）。回 occurrences/size。
pub const TOOL_EDIT: ToolDef = ToolDef {
    name: "edit",
    description: r#"Exact string replacement in YOUR persistent memory files (/memory/** only — /game files are read-only, use write+submit to act).
old_string must match the file content EXACTLY (including whitespace/indentation) and must be UNIQUE: 0 occurrences -> error (read the file first, copy the exact text); more than 1 -> error (include more surrounding lines to make it unique, or set replace_all=true to replace every occurrence). new_string replaces it verbatim (empty string = deletion). Returns {ok:true, path, occurrences, size}."#,
    input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"/memory/<path> of the file to edit"},"old_string":{"type":"string","description":"exact text to replace; must be unique unless replace_all"},"new_string":{"type":"string","description":"replacement text (may be empty to delete)"},"replace_all":{"type":"boolean","description":"replace every occurrence (default false)"}},"required":["path","old_string","new_string"]}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `grep`：pattern（正则）+ path 前缀（如 `/game/history`、`/memory`）。
/// 对动态文件即时合成后匹配——grep 历史局面=灵活回看任意手数后的局势。
pub const TOOL_GREP: ToolDef = ToolDef {
    name: "grep",
    description: r#"Regex search over the virtual filesystem, line-based and case-sensitive (^ = line start, $ = line end).
pattern: regex — literals, . * + ? {m,n} (repetition), [...] classes, | alternation, () groups, ^ $ anchors, \d \w \s escapes.
path: prefix to search, e.g. /game/history, /game/board, /game, /memory, /memory/notes, /index. Dynamic files are synthesized on the fly, so you grep LIVE content: /game/history expands to /game/history/<n> for every played move — grep a stone pattern to review the position after any move; /game/events greps the full event history; /memory searches your persistent files.
Returns {ok, pattern, matches:[{path, line, text}], total, truncated} — per-line text capped at 240 chars, at most 200 matches."#,
    input_schema: r#"{"type":"object","properties":{"pattern":{"type":"string","description":"regex to match per line"},"path":{"type":"string","description":"path prefix to search under, e.g. /game/history, /game, /memory"}},"required":["pattern","path"]}"#,
    read_only: true,
    scope: ToolScope::Shared,
};

/// `delegate`：task（交给子代理的任务描述）。子代理=同 LLM 独立上下文的小循环
/// （只读 read/grep，深度 1 不许再嵌套，独立小预算），结论作为工具结果回父代理。
/// **默认不启用**（设置卡可选启用 `enableSubagent`）。
pub const TOOL_DELEGATE: ToolDef = ToolDef {
    name: "delegate",
    description: r#"Delegate a thinking task to a subagent — the same LLM in a fresh context. task describes what to work out, e.g. "read the board and rank the 3 best candidate points with short reasons".
The subagent gets only read/grep (no actions, no nesting — depth 1) and its own small LLM budget; its final conclusion text comes back as this tool's result, so deep thinking does not pollute your context. Disabled by default — the user enables it in the agent settings; when disabled the tool is not even listed. Use sparingly: one delegate per hard decision, not every move."#,
    input_schema: r#"{"type":"object","properties":{"task":{"type":"string","description":"self-contained task for the subagent; it starts with no context, so include what to read and what to answer"}},"required":["task"]}"#,
    read_only: true,
    scope: ToolScope::BuiltinOnly,
};

/// `wait_events`：timeout_secs=0（0=只排空；>0≤120=阻塞等）。
/// 队列里**全部**待处理事件一次返回；空超时=`[]`（正常返回非错误）。
/// 它既是等待也是排空——MCP 模式获取对手动作的唯一通道。
pub const TOOL_WAIT_EVENTS: ToolDef = ToolDef {
    name: "wait_events",
    description: r#"Wait for and drain game events — MCP mode only (in builtin mode the opponent's actions are pushed to you automatically between turns; there is nothing to poll).
timeout_secs: 0 = drain whatever is pending and return immediately; 1..=120 = block until at least one event arrives (checking the queue after every snapshot beat), then return ALL pending events at once. Empty after the timeout = [] — a normal result, not an error; call again to keep waiting.
Events (JSON objects, tagged "t"): move{x,y,by}, pass{by}, chat{from,text}, request_received{kind,from}, request_resolved{kind,approved}, scoring_started, score_result{black,white,winner,deadRemoved}, game_over{winner}. This is your ONLY channel for the opponent's moves and messages — after placing your stone, wait_events for the reply."#,
    input_schema: r#"{"type":"object","properties":{"timeout_secs":{"type":"integer","minimum":0,"maximum":120,"description":"0 = drain only; 1..120 = block up to N seconds for new events (default 0)"}}}"#,
    read_only: true,
    scope: ToolScope::McpOnly,
};

/// `game_start`：无必填（棋种/路数/执色以用户 UI 配置为权威）→ 回 started/my_color。
/// 外部 Agent 认领席位并完成配对（同一 pair() 函数）。
pub const TOOL_GAME_START: ToolDef = ToolDef {
    name: "game_start",
    description: r#"Claim the agent seat at the table (MCP entry). No required arguments — game kind, board size and your color are chosen by the user in the UI and are authoritative; you cannot change them.
Returns {ok:true, started:true, my_color:"black"|"white"} once a game is live, or a business error if the human side has not set a game up yet — then wait and retry. After started:true, read /index then /game/status before acting."#,
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::McpOnly,
};

/// `game_leave`：—（局中自动认输后离场；收尾动作，配对随局散）。
pub const TOOL_GAME_LEAVE: ToolDef = ToolDef {
    name: "game_leave",
    description: r#"Leave the table (MCP exit — call when you are done). If a game is still live it auto-resigns first (the human wins), then the pairing is torn down. Returns {ok:true, resigned:true|false} — false when there was nothing to resign (game already over)."#,
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::McpOnly,
};

/// 全集（9 个，顺序即 tools/list 的呈现序：共用文件族在前、模式特有在后）。
///
/// **static 而非 const**：`tools_for`/`by_name` 要回 `&'static ToolDef`——const
/// 每次引用都内联临时值，借不出 'static；static 天然 'static 且同样是编译期数据
/// （ToolDef 全字段是 &'static str/bool，自动 Sync）。
pub static TOOLS: [ToolDef; 9] = [
    TOOL_READ,
    TOOL_WRITE,
    TOOL_SUBMIT,
    TOOL_EDIT,
    TOOL_GREP,
    TOOL_DELEGATE,
    TOOL_WAIT_EVENTS,
    TOOL_GAME_START,
    TOOL_GAME_LEAVE,
];

/// 按驱动过滤后的工具清单（MCP 出口的 tools/list 与内置循环的 LLM 工具面都吃它）。
///
/// delegate 的开关独立于驱动：`subagent_enabled=false` 时内置模式也不在册
/// （默认关；测试断言「未启用时工具不在清单」就钉在这条过滤上）。
#[must_use]
pub fn tools_for(driver: Driver, subagent_enabled: bool) -> Vec<&'static ToolDef> {
    TOOLS
        .iter()
        .filter(|t| match t.scope {
            ToolScope::Shared => true,
            ToolScope::BuiltinOnly => driver == Driver::Builtin && subagent_enabled,
            ToolScope::McpOnly => driver == Driver::Mcp,
        })
        .collect()
}

/// 按名取定义（execute 分发与 MCP tools/call 的名字校验共用）。
#[must_use]
pub fn by_name(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// schema 合法性：每个工具的 input_schema 都必须能解析成 JSON 对象，且
    /// required 里点名的属性都在 properties 里（协议侧直接内嵌，坏 schema 立即显形）。
    #[test]
    fn schema_合法且自洽() {
        for t in TOOLS.iter() {
            let v: serde_json::Value = serde_json::from_str(t.input_schema)
                .unwrap_or_else(|e| panic!("{} 的 schema 不是合法 JSON: {e}", t.name));
            assert_eq!(v["type"], "object", "{} schema 顶层必须是 object", t.name);
            let props = v["properties"].as_object().expect("properties 必须是对象");
            if let Some(req) = v["required"].as_array() {
                for r in req {
                    let key = r.as_str().expect("required 元素必须是字符串");
                    assert!(props.contains_key(key), "{}: required[{key}] 不在 properties", t.name);
                }
            }
            for (k, p) in props {
                let ty = p["type"].as_str().expect("每个属性都要有 type");
                assert!(
                    matches!(ty, "string" | "integer" | "number" | "boolean"),
                    "{}.{k} 的 type={ty} 超出手写约定",
                    t.name
                );
            }
        }
    }

    /// 清单完整性：名字唯一、只读标注与计划一致、模式过滤后计数正确。
    #[test]
    fn 清单名字唯一且只读标注正确() {
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "工具名重复");
        for t in TOOLS.iter() {
            let expect_ro = matches!(t.name, "read" | "grep" | "wait_events" | "delegate");
            assert_eq!(t.read_only, expect_ro, "{} 的 read_only 标注", t.name);
        }
    }

    /// delegate 开关对工具清单的影响（计划测试项）：内置默认 5 个、开启后 6 个；
    /// MCP 恒 8 个（无 delegate、多三个模式专属）；开关对 MCP 无影响。
    #[test]
    fn delegate_开关与驱动过滤工具清单() {
        let names = |v: &[&'static ToolDef]| -> Vec<&str> { v.iter().map(|t| t.name).collect() };

        let builtin_off = tools_for(Driver::Builtin, false);
        assert_eq!(builtin_off.len(), 5, "内置默认（子代理关）：{:?}", names(&builtin_off));
        assert!(!names(&builtin_off).contains(&"delegate"));

        let builtin_on = tools_for(Driver::Builtin, true);
        assert_eq!(builtin_on.len(), 6);
        assert!(names(&builtin_on).contains(&"delegate"));

        for on in [false, true] {
            let mcp = tools_for(Driver::Mcp, on);
            assert_eq!(mcp.len(), 8, "MCP 与子代理开关无关");
            let mcp = names(&mcp);
            for expect in ["wait_events", "game_start", "game_leave"] {
                assert!(mcp.contains(&expect), "MCP 缺 {expect}");
            }
            assert!(!mcp.contains(&"delegate"), "delegate 对外部 Agent 无意义");
        }

        // by_name 与清单互为反查：过滤后的每个名字都能取回同一个定义。
        for t in TOOLS.iter() {
            assert_eq!(by_name(t.name).map(|d| d as *const ToolDef), Some(t as *const ToolDef));
        }
        assert!(by_name("nope").is_none());
    }
}
