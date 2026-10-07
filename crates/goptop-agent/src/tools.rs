//! 工具定义 —— 纯数据（计划「工具清单」节的 9 个工具）。
//!
//! 执行在 [`crate::registry`]；这里只有「长什么样、给谁用」的静态事实，供
//! MCP tools/list、三协议的工具面与 /index 的冗余文案共用。**不引 schemars**：
//! input_schema 是手写 JSON Schema 文本一份喂 rmcp 与三协议（计划拍板——lock 里
//! schemars 0.8/1.x 并存，不添乱）。
//!
//! **工具描述里写全各文件的功能职责与用法**（in/ 各文件的取值协议、只读文件的
//! 内容结构都进 description——模型不看 /index 也能用对）。骨架期 description 为
//! 占位文本，由阶段①的 tools 负责人写全；名字/只读标注/适用面已定死，实现方
//! 不得改名（MCP tools/list 与三协议的工具名都以这里的常量为准）。

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
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: true,
    scope: ToolScope::Shared,
};

/// `write`：path + content（整体覆盖）。对 `/game/in/*` 写=**暂存**（只做格式校验
/// 并回预览，不执行）；对只读路径写→拒绝；对 /memory 写=持久。
pub const TOOL_WRITE: ToolDef = ToolDef {
    name: "write",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `submit`：path（按路径自动判别提交什么：in/move→落子、in/chat→发消息、
/// in/request→发起请求、in/confirm→批复、in/score→确认计分、in/resign→认输）。
pub const TOOL_SUBMIT: ToolDef = ToolDef {
    name: "submit",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `edit`：path + old_string + new_string + replace_all?=false（仅 /memory；
/// 0 次或 >1 次不唯一→报错回模型，Claude Code 语义）。回 occurrences/size。
pub const TOOL_EDIT: ToolDef = ToolDef {
    name: "edit",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::Shared,
};

/// `grep`：pattern（正则）+ path 前缀（如 `/game/history`、`/memory`）。
/// 对动态文件即时合成后匹配——grep 历史局面=灵活回看任意手数后的局势。
pub const TOOL_GREP: ToolDef = ToolDef {
    name: "grep",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: true,
    scope: ToolScope::Shared,
};

/// `delegate`：task（交给子代理的任务描述）。子代理=同 LLM 独立上下文的小循环
/// （只读 read/grep，深度 1 不许再嵌套，独立小预算），结论作为工具结果回父代理。
/// **默认不启用**（设置卡可选启用 `enableSubagent`）。
pub const TOOL_DELEGATE: ToolDef = ToolDef {
    name: "delegate",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: true,
    scope: ToolScope::BuiltinOnly,
};

/// `wait_events`：timeout_secs=0（0=只排空；>0≤120=阻塞等）。
/// 队列里**全部**待处理事件一次返回；空超时=`[]`（正常返回非错误）。
/// 它既是等待也是排空——MCP 模式获取对手动作的唯一通道。
pub const TOOL_WAIT_EVENTS: ToolDef = ToolDef {
    name: "wait_events",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: true,
    scope: ToolScope::McpOnly,
};

/// `game_start`：无必填（棋种/路数/执色以用户 UI 配置为权威）→ 回 started/my_color。
/// 外部 Agent 认领席位并完成配对（同一 pair() 函数）。
pub const TOOL_GAME_START: ToolDef = ToolDef {
    name: "game_start",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::McpOnly,
};

/// `game_leave`：—（局中自动认输后离场；收尾动作，配对随局散）。
pub const TOOL_GAME_LEAVE: ToolDef = ToolDef {
    name: "game_leave",
    description: "placeholder（阶段①由 tools 负责人写全）",
    input_schema: r#"{"type":"object","properties":{}}"#,
    read_only: false,
    scope: ToolScope::McpOnly,
};

/// 全集（9 个，顺序即 tools/list 的呈现序：共用文件族在前、模式特有在后）。
pub const TOOLS: [ToolDef; 9] = [
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
    todo!()
}

/// 按名取定义（execute 分发与 MCP tools/call 的名字校验共用）。
#[must_use]
pub fn by_name(name: &str) -> Option<&'static ToolDef> {
    todo!()
}
