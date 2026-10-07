//! 系统提示词 —— Claude Code 风格简化版（**英文撰写**），末尾 `# Language` 段注入
//! 回复语言（计划「系统提示词」节的分段拍板）。
//!
//! 分段：Identity（equal-footing player, allowed to err, admits being an AI）／
//! **World model（置顶）**：a virtual filesystem — /game holds live game files,
//! /memory is your long-term memory; read /index first. Workflow is always:
//! write to stage, submit(path) to commit ／ Rules digest（gomoku 五连；go 提子/
//! 禁着/双 pass 中国规则——全文在 /game/rules）／ Event mechanism（builtin:
//! 对手动作自动推送，无轮询工具）／ Action discipline（text-only is NOT an action,
//! the loop never stops for it; 被拒后读错误，绝不盲试同一坐标）／ Etiquette
//!（开局问好、认输有风度、不刷屏）／ Ending（winner 出现 → 写告别语并 submit）／
//! **# Language（Always respond in {replyLang}）**。
//!
//! **三处冗余**：提示词、/index、工具描述互为副本——模型漏看其一也能用对；
//! 改任何一处的文件职责描述，三处要同步改（测试钉住 /index 与工具描述的关键词）。

use crate::Driver;

/// 提示词装配参数（AgentHub 在开局时从 UI 配置聚齐）。
#[derive(Clone, Debug)]
pub struct PromptCfg {
    /// B 席展示名（默认「Agent」，仅聊天展示，非身份）。
    pub agent_name: String,
    /// 本局执色（`"black"`/`"white"`；座位方向由 pair 决定，这里只用于措辞）。
    pub my_color: String,
    /// 棋种（`"gomoku"`/`"go"`；Rules digest 按它选段）。
    pub kind: String,
    /// 路数。
    pub size: u16,
    /// 回复语言：**任意字符串**原样注入 `# Language` 段（"简体中文"/"English"/
    /// "喵语"/"摩斯密码"……不校验；null=跟随 UI 语言，在 AgentHub 装配时已解开）。
    pub reply_lang: String,
    /// 驱动方式（Event mechanism 段据此措辞：内置讲自动推送，MCP 讲 wait_events）。
    pub driver: Driver,
    /// delegate 工具是否在册（子代理默认关——关时提示词不提子代理，
    /// 免得模型去调一个不存在的工具）。
    pub subagent_enabled: bool,
}

/// 拼装系统提示词。纯函数、无 IO：同样的 cfg 恒产出同样的文本
/// （测试可以做逐段断言；文案改动只动本文件）。
#[must_use]
pub fn build_system_prompt(cfg: &PromptCfg) -> String {
    todo!()
}
