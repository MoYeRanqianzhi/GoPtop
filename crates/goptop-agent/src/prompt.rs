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

/// Rules digest 段：按棋种给最小心智模型，全文一律指向 `/game/rules`（规则文本
/// 单一权威源在 vfs 的动态合成器——提示词只做摘要，两处各改各的必然漂移）。
fn rules_digest(kind: &str, size: u16) -> String {
    let coords = "The board is indexed x,y from 0 — write coordinates as \"x,y\".";
    let full = "The full rules text lives in `/game/rules` — read it before your first move and whenever a situation feels ambiguous.";
    match kind {
        "gomoku" => format!(
            "# Rules digest\n\n\
             Gomoku on a {size}x{size} board: players alternate placing stones; the first to get \
             five of their stones in a row — horizontal, vertical, or diagonal — wins. {coords} {full}"
        ),
        "go" => format!(
            "# Rules digest\n\n\
             Go on a {size}x{size} board, Chinese rules: a group with no liberties is captured; \
             suicide is banned; two consecutive passes end the game and scoring is by area. \
             Write coordinates as \"x,y\", or \"pass\". During scoring the human marks the dead \
             stones — you only confirm the result via `/game/in/score`. {full}"
        ),
        // 未知棋种不自创规则，全部下放给 /game/rules（值域本就只有两种，护畸形输入）。
        _ => format!(
            "# Rules digest\n\n\
             You are playing {kind} on a {size}x{size} board. {coords} {full}"
        ),
    }
}

/// Event mechanism 段：内置讲自动推送（无轮询工具），MCP 讲 wait_events（既是等待
/// 也是排空）——两模式的事件获取方式不同，措辞不能共用一段。
fn event_mechanism(driver: Driver) -> &'static str {
    match driver {
        Driver::Builtin => {
            "# Event mechanism\n\n\
             You never poll: your opponent's moves, chat messages, requests, scoring and game \
             over are pushed to you automatically between your tool calls as `<event>` blocks. \
             When it is not your turn you may think, take notes in `/memory`, or chat — the \
             next event will reach you without any action on your part."
        }
        Driver::Mcp => {
            "# Event mechanism\n\n\
             Nothing is pushed to you: to learn what your opponent did, call `wait_events`. It \
             returns every queued event at once, or blocks up to its timeout until something \
             arrives — it both waits and drains. An empty result means nothing has happened \
             yet; call it again."
        }
    }
}

/// 拼装系统提示词。纯函数、无 IO：同样的 cfg 恒产出同样的文本
/// （测试可以做逐段断言；文案改动只动本文件）。
#[must_use]
pub fn build_system_prompt(cfg: &PromptCfg) -> String {
    let PromptCfg {
        agent_name,
        my_color,
        kind,
        size,
        reply_lang,
        driver,
        subagent_enabled,
    } = cfg;

    let mut out = String::with_capacity(4_096);

    // Identity：对等玩家、允许犯错、坦承 AI——计划拍板的三件事一段说清。
    out.push_str(&format!(
        "# Identity\n\n\
         You are {agent_name}, an LLM seated at the board as a full player, on equal footing \
         with your human opponent. You are allowed to make mistakes, and you should own them \
         like any player does. If asked, say plainly that you are an AI. You play {my_color} \
         stones this game; the seat was decided in the game setup and does not change.\n\n"
    ));

    // World model（置顶）：一切皆文件 + stage-then-submit——这是本 Agent 的全部工作面，
    // 紧跟 Identity 放在最前（模型最先建立的心智模型决定它后续会不会乱来）。
    out.push_str(
        "# World model\n\n\
         Everything around the board is a virtual filesystem.\n\
         - `/game` holds the live game as read-only files, synthesized on demand from real \
           state: `/game/status` (turn, colors, winner, scoring, pending request, staged move), \
           `/game/board` (current position; variants `/game/board/grid`, `/ascii`, `/pretty`, \
           `/image.png`), `/game/rules`, `/game/history` (JSONL; `/game/history/<n>` = the \
           position after move n, same variants as the board), `/game/chat`, `/game/events` \
           (full event history, seq-increasing).\n\
         - `/memory` is your long-term memory — plain files that survive across games, e.g. \
           notes on this opponent's style. Writes there take effect immediately; staging \
           and submit do NOT apply to `/memory`.\n\
         - Read `/index` first; it lists every path and what it is for.\n\n\
         Stage-then-submit applies ONLY to the `/game/in/*` slots: write to stage \
         (you may overwrite it to reconsider — nothing happens yet), then `submit(path)` to \
         make it real. Submitting CLEARS the slot — like pressing send empties the input \
         box; the action is out, the file is empty again. Staging gives you a chance to \
         re-read and reconsider before anything happens.\n\
         - Place a stone: write the coords to `/game/in/move`, then submit it.\n\
         - Send a message: write `/game/in/chat`, then submit it.\n\
         - Undo / reset / swap request: `/game/in/request`.\n\
         - Approve or reject a pending request: `/game/in/confirm`.\n\
         - Confirm scoring: `/game/in/score`.\n\
         - Resign: `/game/in/resign`.\n\
         Use `read`'s offset/limit to page through long files (chat log, history); \
         `grep /game/history` to review the position after any move.\n\n",
    );

    out.push_str(&rules_digest(kind, *size));
    out.push_str("\n\n");
    out.push_str(event_mechanism(*driver));
    out.push_str("\n\n");

    // Action discipline：TextOnly≠行动（循环不因此停）、不强制每手棋、被拒不盲试。
    out.push_str(
        "# Action discipline\n\n\
         - Text-only replies are NOT an action. The loop never stops for them — to do anything \
           (move, chat, request, resign) you must go through the tools: stage with `write`, \
           commit with `submit`.\n\
         - You are never forced to move every turn. Waiting, thinking, or writing notes is a \
           valid way to spend a turn.\n\
         - When a submit is rejected, read the error and understand it. Never retry the same \
           coordinates blindly. Format mistakes are rejected at write time; game-rule \
           violations (occupied point, not your turn) at submit time — a rejection never \
           advances the game, so fix and continue.\n",
    );
    // 子代理只在册时提——提示词与工具面互为冗余，但绝不能冗余出一个不存在的工具。
    if *subagent_enabled {
        out.push_str(
            "- For deep deliberation you may `delegate` a task (e.g. \"evaluate candidate \
             points for my next move\"); the subagent reads the same filesystem and returns \
             its conclusion as the tool result, without polluting your context.\n",
        );
    }
    out.push('\n');

    out.push_str(
        "# Etiquette\n\n\
         Greet your opponent at the start of the game. Keep chat purposeful — no spam, no \
         flooding. If the position is hopeless, resign gracefully instead of dragging the \
         game out. Treat your opponent the way you would want to be treated across a board.\n\n\
         # Ending\n\n\
         When the game is about to end — a winner has appeared, or you have decided to \
         resign — write a short farewell to `/game/in/chat` and submit it first. Every game \
         deserves a proper goodbye.\n\n",
    );

    // # Language：任意字符串原样注入（"简体中文"/"喵语"……不校验、不翻译、不加引号）。
    out.push_str(&format!("# Language\n\nAlways respond in {reply_lang}.\n"));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷装配：gomoku 15 路、执黑、内置、简体中文、子代理关。
    fn cfg() -> PromptCfg {
        PromptCfg {
            agent_name: "Agent".into(),
            my_color: "black".into(),
            kind: "gomoku".into(),
            size: 15,
            reply_lang: "简体中文".into(),
            driver: Driver::Builtin,
            subagent_enabled: false,
        }
    }

    /// 关键段全在：计划拍板的八个段名一个不缺（缺段=循环的对应机制对模型失明）。
    #[test]
    fn contains_all_key_sections() {
        let p = build_system_prompt(&cfg());
        for section in [
            "# Identity",
            "# World model",
            "# Rules digest",
            "# Event mechanism",
            "# Action discipline",
            "# Etiquette",
            "# Ending",
            "# Language",
        ] {
            assert!(p.contains(section), "缺段：{section}");
        }
    }

    /// World model 段的要点：/index 先读、stage-then-submit 工作流、六个 in/ 文件
    /// 与长文件翻页——与 /index、工具描述三处冗余的关键词。
    #[test]
    fn world_model_covers_stage_then_submit() {
        let p = build_system_prompt(&cfg());
        assert!(p.contains("Read `/index` first"));
        // 语义三要点（用户拍板 2026-10-08）：stage-then-submit 只管 /game/in/*；
        // submit 提交即清槽（发送键语义）；/memory 即写即存不走 submit。
        assert!(p.contains("Stage-then-submit"));
        assert!(p.contains("ONLY to the `/game/in/*` slots"));
        assert!(p.contains("CLEARS the slot"));
        assert!(p.contains("do NOT apply to `/memory`"));
        assert!(p.contains("`submit(path)`"));
        for path in [
            "/game/in/move",
            "/game/in/chat",
            "/game/in/request",
            "/game/in/confirm",
            "/game/in/score",
            "/game/in/resign",
        ] {
            assert!(p.contains(path), "in/ 文件缺失：{path}");
        }
        assert!(p.contains("offset/limit"));
        assert!(p.contains("`/memory`"));
    }

    /// # Language 行：replyLang 原样注入——中文、任意字符串（喵语）都逐字出现，
    /// 不校验、不翻译（计划拍板：用户可设任意字符串）。
    #[test]
    fn language_line_injects_reply_lang_verbatim() {
        let p = build_system_prompt(&cfg());
        assert!(p.contains("Always respond in 简体中文."));

        let mut weird = cfg();
        weird.reply_lang = "喵语".into();
        assert!(build_system_prompt(&weird).contains("Always respond in 喵语."));
    }

    /// 执色与棋种参数化：颜色、路数进文本；Rules digest 按棋种换段（gomoku 讲五连，
    /// go 讲提子/禁着/双 pass 中国规则且不出现五连）。
    #[test]
    fn rules_digest_parameterized_by_color_and_kind() {
        let gomoku = build_system_prompt(&cfg());
        assert!(gomoku.contains("You play black stones"), "执色必须进提示词");
        assert!(gomoku.contains("15x15"));
        assert!(gomoku.contains("five of their stones in a row"));

        let mut go = cfg();
        go.my_color = "white".into();
        go.kind = "go".into();
        go.size = 19;
        let go_p = build_system_prompt(&go);
        assert!(go_p.contains("You play white stones"));
        assert!(go_p.contains("19x19"));
        assert!(go_p.contains("Chinese rules"));
        assert!(go_p.contains("suicide is banned"));
        assert!(go_p.contains("two consecutive passes"));
        assert!(!go_p.contains("five of their stones"), "围棋段不得串入五子棋规则");
        // 未知棋种：不自创规则，全部下放 /game/rules。
        let mut unknown = cfg();
        unknown.kind = "renju".into();
        assert!(build_system_prompt(&unknown).contains("`/game/rules`"));
    }

    /// Event mechanism 按驱动措辞：内置讲自动推送且绝不提 wait_events（不存在的
    /// 工具）；MCP 讲 wait_events。
    #[test]
    fn event_mechanism_follows_driver() {
        let builtin = build_system_prompt(&cfg());
        assert!(builtin.contains("pushed to you automatically"));
        assert!(!builtin.contains("wait_events"), "内置模式无此工具，不得提及");

        let mut mcp = cfg();
        mcp.driver = Driver::Mcp;
        let mcp_p = build_system_prompt(&mcp);
        assert!(mcp_p.contains("`wait_events`"));
        assert!(!mcp_p.contains("pushed to you automatically"));
    }

    /// 子代理开关：开时提示 delegate（与在册工具一致）；关时只字不提（模型不得
    /// 被引导去调不存在的工具）。
    #[test]
    fn subagent_mention_follows_switch() {
        let mut on = cfg();
        on.subagent_enabled = true;
        assert!(build_system_prompt(&on).contains("`delegate`"));

        let off = build_system_prompt(&cfg());
        assert!(!off.contains("delegate"));
    }

    /// 展示名进 Identity 段（agent_start 可传自定义名）。
    #[test]
    fn agent_name_appears_in_identity() {
        let mut named = cfg();
        named.agent_name = "棋士君".into();
        let p = build_system_prompt(&named);
        assert!(p.contains("You are 棋士君"));
    }
}
