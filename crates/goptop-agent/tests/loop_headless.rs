//! 无头全量测试 —— 真状态机（双会话经进程内 BC 配对）+ Mock LLM 剧本，驱动内置
//! 决策循环与工具面打完整对局（计划「测试计划」第 1 条的全清单）。
//!
//! 手法复刻 `crates/goptop-transport-native/tests/headless.rs`：
//! - **SERIAL 全局锁**：进程内 BC hub 无局号，并行用例会互相收到对方的消息——
//!   配对/对局类用例必须串行（这是测试隔离的需要，不是产品缺陷）；
//! - **pump_until 条件等待**：传播耗时刻随环境波动，死等固定轮数必 flaky——
//!   一律「轮询到谓词成立或超时」。
//!
//! 人侧脚本化：A' 的动作（落子/停一手/批复/聊天）由每用例的 driver 任务按剧本
//! 发出；Agent 侧只经工具面与循环行动——**绝不手工替 Agent 发 UiCommand**，
//! 那会绕过被测的两段式工具面。
//!
//! 剧本的「呼吸冗余」：循环在等待回合以 IDLE_POLL_MS 节拍空转，submit 撞
//! 「未轮到/已占点」回业务错误是正常呼吸——落子步的坐标**逐点翻倍**（同一坐标
//! 重复出现），无论每手消耗 1 还是 2 步剧本，落点序列都不变。

// SERIAL 锁必须横跨整个用例的 await（串行化的本意）——与 pair.rs / headless.rs
// 的既有纪律同款，不是「锁忘放」。
#![allow(clippy::await_holding_lock)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use goptop_agent::agent_loop::{LoopConfig, LoopDeps, LoopStop, SubagentLoop, run};
use goptop_agent::llm::{
    Block, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError, MockScript, MockStep,
    Protocol, StopReason, ToolCall, Usage,
};
use goptop_agent::pair::{PairConfig, SeatColor, pair};
use goptop_agent::player::{EventQueue, EmitWatch, GameEvent, NativePlayer, PlayerHandle, run_event_pump};
use goptop_agent::prompt::{PromptCfg, build_system_prompt};
use goptop_agent::registry::{SubagentRunner, ToolCtx, ToolError, execute};
use goptop_agent::store::NativeStore;
use goptop_agent::vfs::{InFile, Staging};
use goptop_agent::Driver;
use goptop_net::session::UiCommand;
use goptop_transport_native::{HeadlessHost, Host, NativeSession};
use serde_json::{Value, json};

/// 进程内 BC hub 全局无局号（bc.rs 头注自证缺口）——对局类用例串行。
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/* ---------------- 剧本步构造（Mock LLM 的预置响应） ---------------- */

/// 纯文字步：零工具调用（TextOnly ≠ 行动，循环必须续命）。
fn pure_text(text: &str) -> ChatResponse {
    ChatResponse {
        content: text.to_string(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        usage: Usage { input_tokens: 10, output_tokens: 5, cache_read_tokens: 0, cache_write_tokens: 0 },
    }
}

/// 工具调用步：一轮里的多个调用按序执行（write→submit 同轮是常规打法）。
fn tool_step(calls: Vec<(&str, Value)>) -> ChatResponse {
    ChatResponse {
        content: String::new(),
        tool_calls: calls
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| ToolCall { id: format!("c{i}"), name: name.into(), arguments: args })
            .collect(),
        stop: StopReason::ToolUse,
        usage: Usage { input_tokens: 10, output_tokens: 5, cache_read_tokens: 0, cache_write_tokens: 0 },
    }
}

/// 暂存并提交一步（两段式的一整口气）。
fn ws(path: &str, content: &str) -> ChatResponse {
    tool_step(vec![
        ("write", json!({ "path": path, "content": content })),
        ("submit", json!({ "path": path })),
    ])
}

/// read 工具步（等待回合的廉价填充——给对手动作留出传播窗口）。
fn peek(path: &str) -> ChatResponse {
    tool_step(vec![("read", json!({ "path": path }))])
}

/// 落子步坐标翻倍的展开（见模块注「呼吸冗余」）。
fn double_moves(coords: &[(u16, u16)]) -> Vec<ChatResponse> {
    coords
        .iter()
        .flat_map(|&(x, y)| {
            let c = format!("{x},{y}");
            [ws("/game/in/move", &c), ws("/game/in/move", &c)]
        })
        .collect()
}

/* ---------------- 测试夹具 ---------------- */

/// 每用例独享的记忆库文件（并行用例同进程，文件名撞车会互读对方的表）。
fn temp_db() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("goptop-agent-loop-{}-{n}.db", std::process::id()))
}

/// 无 STUN 的宿主：本地候选即刻收集完，省掉 gathering 的 8s 超时兜底。
fn host() -> Arc<HeadlessHost> {
    let h = Arc::new(HeadlessHost::default());
    h.storage_set("goptop:stun", Some("[]"));
    h
}

/// 一局已配对、已接好工具面与事件泵的测试台。
struct Rig {
    front: NativePlayer,
    ctx: ToolCtx,
    queue: Arc<EventQueue>,
}

/// 搭台：配对（方向随执色）→ 前端泵任务 → 事件物化泵 → ToolCtx 装配。
async fn rig(kind: &str, size: u16, my_color: SeatColor) -> Rig {
    let memory = Arc::new(NativeStore::open(&temp_db()).expect("temp memory db"));
    let cfg = PairConfig {
        kind: kind.into(),
        size,
        my_color,
        front_name: "甲".into(),
        agent_name: "Agent".into(),
        share_origin: "http://localhost".into(),
        front_host: host(),
        agent_host: host(),
    };
    let paired = pair(cfg).await.expect("配对应成功");
    let front = paired.front;
    // A' 的常驻泵（黑方向 pair() 不给 front start_pump——前端 poll 模式；测试里
    // 由本任务代泵。重复泵幂等，白方向与常驻泵并存无害）。
    let f = front.session().clone();
    tokio::spawn(async move {
        loop {
            f.pump();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    // 事件物化泵：基线在 spawn 前于本任务同步取定（event_baseline）——泵任务晚起
    // 也吞不掉基线之后的事件（「基线前的存量不重发」由参数语义保证，不靠固定
    // sleep 屏障死等）。HookHost 绑队列：协商 Ack 只出提示条不改快照（B 作为请求
    // 方被拒的唯一载体），由 notice 拦截物化进同一队列（seq 与 diff 泵单锁统一分配）。
    let queue = Arc::new(EventQueue::new());
    paired.agent_hook.bind_events(&queue);
    let baseline = goptop_agent::player::event_baseline(&paired.agent_watch);
    tokio::spawn(run_event_pump(
        EmitWatch::new(paired.agent_watch.clone_rx()),
        queue.clone(),
        baseline,
    ));

    let ctx = ToolCtx {
        player: Arc::new(paired.agent),
        watch: EmitWatch::new(paired.agent_watch.clone_rx()),
        events: queue.clone(),
        staging: Arc::new(Staging::new()),
        memory,
        memory_ns: Driver::Builtin.memory_ns(),
        driver: Driver::Builtin,
        subagent_enabled: false,
        subagent: None,
    };
    Rig { front, ctx, queue }
}

/// 免配对的工具面台子（home 相位会话）：delegate / compact 等不依赖对局现场的
/// 用例用——配对是昂贵夹具（ICE 等待以秒计），这些语义只需要一个能 snapshot 的席。
/// 会话用装饰宿主（与 pair() 的 B 席同形）：HookHost（watch sender 唯一持有者）
/// 随会话活在 ctx.player 里，watch 的接收端整局可 changed()。
fn home_ctx() -> ToolCtx {
    let (hook, watch) = goptop_agent::player::HookHost::wrap(host());
    ToolCtx {
        player: Arc::new(NativePlayer::new(Arc::new(NativeSession::new(
            goptop_transport_native::SessionConfig {
                name: "测试".into(),
                server_mode: false,
                share_origin: "http://localhost".into(),
                kind: "gomoku".into(),
                size: 15,
            },
            hook,
            "http://localhost/p2p",
        )))),
        watch,
        events: Arc::new(EventQueue::new()),
        staging: Arc::new(Staging::new()),
        memory: Arc::new(NativeStore::open(&temp_db()).unwrap()),
        memory_ns: "builtin",
        driver: Driver::Builtin,
        subagent_enabled: false,
        subagent: None,
    }
}

/// ToolCtx 的共享克隆（字段全是 Arc/句柄）——子代理与父代理看同一局、同一暂存区。
fn clone_ctx(c: &ToolCtx) -> ToolCtx {
    ToolCtx {
        player: Arc::clone(&c.player),
        watch: EmitWatch::new(c.watch.clone_rx()),
        events: Arc::clone(&c.events),
        staging: Arc::clone(&c.staging),
        memory: Arc::clone(&c.memory),
        memory_ns: c.memory_ns,
        driver: c.driver,
        subagent_enabled: c.subagent_enabled,
        subagent: c.subagent.clone(),
    }
}

/// LoopDeps 装配（spawn_loop 与 compact 用例共用；compact 用例要拿 LoopOutcome
/// 的 stats，自己 await run()）。
fn build_deps(ctx: ToolCtx, script: Arc<MockScript>, my_color: &str, cfg: LoopConfig) -> LoopDeps {
    LoopDeps {
        llm: Arc::new(LlmClient::Mock(Arc::clone(&script))),
        llm_cfg: LlmConfig {
            protocol: Protocol::Anthropic,
            base_url: "http://mock.local/v1".into(),
            model: "mock-1".into(),
            max_output_tokens: 1024,
            reply_lang: None,
        },
        cfg,
        tools: ctx,
        stop: Arc::new(AtomicBool::new(false)),
        system: build_system_prompt(&PromptCfg {
            agent_name: "Agent".into(),
            my_color: my_color.into(),
            kind: "gomoku".into(),
            size: 15,
            reply_lang: "简体中文".into(),
            driver: Driver::Builtin,
            subagent_enabled: false,
        }),
    }
}

/// 起循环：Mock 剧本 + 内置装配，后台跑到五退出条件之一。
fn spawn_loop(ctx: ToolCtx, script: Arc<MockScript>, my_color: &str) -> (Arc<AtomicBool>, tokio::task::JoinHandle<LoopStop>) {
    let deps = build_deps(ctx, script, my_color, LoopConfig::default());
    let stop = Arc::clone(&deps.stop);
    let handle = tokio::spawn(async move { run(deps).await.stop });
    (stop, handle)
}

/// 请求里**最后一条 user 消息**的文本（循环每轮的注入都落在这条上；Debug 格式会
/// 转义引号不能作 contains 断言，且更早的提醒/心跳已入史——整史拼接会把「上一轮
/// 的提醒」误读成本轮又提醒了）。
fn last_user_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find(|m| m.role == goptop_agent::llm::Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 泵到谓词成立（依赖两席各自的常驻泵推进）。超时 panic 带说明。
async fn until(pred: impl Fn() -> bool, secs: u64, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("超时：{what}");
}

/// 人侧 driver：按坐标表落子（等轮到自己，绝不抢手；终局后自然收摊）。
fn drive_front_places(front: NativePlayer, coords: Vec<(u16, u16)>) {
    tokio::spawn(async move {
        for (x, y) in coords {
            let ok = front.wait_until(
                &mut |s| {
                    s["phase"] == json!("playing") && s["winner"].is_null()
                        && s["toMove"] == s["myColor"]
                },
                Duration::from_secs(30),
            );
            if !ok {
                return;
            }
            front.cmd(UiCommand::Place { x, y });
        }
    });
}

/// Agent 侧工具调用的简写（直跑 registry——「Agent write+submit」的落点）。
async fn act(ctx: &ToolCtx, name: &str, args: Value) -> Result<Value, ToolError> {
    execute(name, &args, ctx).await
}

/* ---------------- 基础对局至五连（我执黑） ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 循环_基础对局至五连_我执黑() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    drive_front_places(front.clone(), vec![(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)]);

    // 剧本：白方垫手（row2，不干扰黑方 row0 五连）+ 聊天垫步 + 告别步。
    // winner 出现后的任意一步工具调用都收束 GameOver——聊天步落在这个窗口无害。
    let mut steps = double_moves(&[(7, 2), (8, 2), (9, 2), (10, 2), (11, 2), (12, 2)]);
    steps.push(ws("/game/in/chat", "好局，恭喜！"));
    steps.push(ws("/game/in/chat", "好局，恭喜！"));
    let script = Arc::new(MockScript::new(steps));

    let (_stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "white");
    let outcome = tokio::time::timeout(Duration::from_secs(120), handle)
        .await
        .expect("循环 120s 内应收束")
        .expect("循环任务不 panic");

    assert!(matches!(outcome, LoopStop::GameOver), "五连终局应收束 GameOver，实际 {outcome:?}");
    assert_eq!(agent.snapshot()["winner"], front.snapshot()["winner"], "两端胜者一致");
    assert_eq!(front.snapshot()["winner"], json!("black"), "人执黑五连胜");
}

/* ---------------- 我执白（反向配对）到终局 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 循环_我执白反向配对_agent五连胜() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::White).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    // 反向配对的自证：B 邀请=黑、人执白。
    assert_eq!(agent.snapshot()["myColor"], json!("black"));
    assert_eq!(front.snapshot()["myColor"], json!("white"));
    drive_front_places(front.clone(), vec![(0, 7), (1, 7), (2, 7), (3, 7), (4, 7)]);

    // Agent 黑连成 row1 五连（坐标翻倍，见模块注）。
    let mut steps = double_moves(&[(0, 1), (1, 1), (2, 1), (3, 1), (4, 1)]);
    steps.push(ws("/game/in/chat", "承让了。"));
    steps.push(ws("/game/in/chat", "承让了。"));
    let script = Arc::new(MockScript::new(steps));

    let (_stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "black");
    let outcome = tokio::time::timeout(Duration::from_secs(120), handle)
        .await
        .expect("循环 120s 内应收束")
        .expect("循环任务不 panic");

    assert!(matches!(outcome, LoopStop::GameOver), "Agent 五连应收束 GameOver，实际 {outcome:?}");
    assert_eq!(agent.snapshot()["winner"], front.snapshot()["winner"], "两端胜者一致");
    assert_eq!(front.snapshot()["winner"], json!("black"), "Agent（黑）五连胜");
}

/* ---------------- submit 两段式语义 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_两段式语义() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let ctx = &rig.ctx;

    // ① write 只暂存不执行：盘面纹丝不动，回执照计划样例。
    let receipt = act(ctx, "write", json!({ "path": "/game/in/move", "content": "7,7" }))
        .await
        .expect("暂存应成功");
    assert_eq!(
        receipt,
        json!({ "ok": true, "staged": { "path": "/game/in/move", "value": { "x": 7, "y": 7 } },
                "note": "call submit(\"/game/in/move\") to place" }),
        "暂存回执逐字照计划样例"
    );
    let snap = agent.snapshot();
    assert_eq!(snap["moveCount"], json!(0), "暂存不落子");
    assert_eq!(snap["board"][7][7], json!("empty"), "盘面未变");

    // ② 提交前可覆盖重写：read 回看新值。
    act(ctx, "write", json!({ "path": "/game/in/move", "content": "8,8" })).await.expect("覆盖暂存");
    let seen = act(ctx, "read", json!({ "path": "/game/in/move" })).await.expect("回看");
    assert_eq!(seen["content"], json!("8,8"), "read 回显暂存原文");
    assert_eq!(seen["total_lines"], json!(1));

    // ③ 未 write 直接 submit → 报错回模型。
    let err = act(ctx, "submit", json!({ "path": "/game/in/chat" })).await.unwrap_err();
    assert!(err.message().contains("nothing staged in /game/in/chat"), "空槽 submit 报错：{err:?}");

    // ④ 格式错误 write 即拒（权威文案逐字）。
    let err = act(ctx, "write", json!({ "path": "/game/in/move", "content": "abc" }))
        .await
        .unwrap_err();
    assert_eq!(
        err.message(),
        "invalid move \"abc\". Expected \"x,y\" (e.g. \"7,7\") or \"pass\".",
        "格式错误文案逐字"
    );

    // ⑤ 对局规则错误 submit 时拒（未轮到我——黑先，Agent 执白）。
    let err = act(ctx, "submit", json!({ "path": "/game/in/move" })).await.unwrap_err();
    assert_eq!(
        err.message(),
        "not your turn (black to move). Opponent action arrives as a pushed event.",
        "未轮到文案逐字"
    );
    assert_eq!(agent.snapshot()["moveCount"], json!(0), "被拒的 submit 不改盘面");

    // ⑥ 人落子后轮到 Agent：暂存的 (8,8) 提交落地。
    front.cmd(UiCommand::Place { x: 7, y: 7 });
    until(|| agent.snapshot()["moveCount"] == json!(1), 15, "人落子应同步到 B").await;
    act(ctx, "write", json!({ "path": "/game/in/move", "content": "8,8" })).await.expect("再暂存");
    let receipt = act(ctx, "submit", json!({ "path": "/game/in/move" })).await.expect("提交落地");
    assert_eq!(receipt, json!({ "ok": true, "action": "move", "move_count": 2, "finished": false }));
    assert_eq!(agent.snapshot()["board"][8][8], json!("white"), "提交后落子");

    // ⑦ 占点：规则错误文案逐字（(7,7) 已被黑占）。占点判定在轮次之后——
    // 先让人再落一手把轮次交回来，占点错误才会显形。
    front.cmd(UiCommand::Place { x: 5, y: 5 });
    until(|| agent.snapshot()["moveCount"] == json!(3), 15, "人第二手应同步到 B").await;
    act(ctx, "write", json!({ "path": "/game/in/move", "content": "7,7" })).await.expect("暂存占点");
    let err = act(ctx, "submit", json!({ "path": "/game/in/move" })).await.unwrap_err();
    assert_eq!(
        err.message(),
        "(7,7) is occupied by black. Choose an empty intersection — read /game/board.",
        "占点文案逐字"
    );

    // ⑧ 同一份内容不允许二次 submit：取走即清槽。
    let err = act(ctx, "submit", json!({ "path": "/game/in/move" })).await.unwrap_err();
    assert!(err.message().contains("nothing staged"), "清槽后 submit 报无内容：{err:?}");
}

/* ---------------- 悔棋批复（人请求 → Agent 批准） ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_人请求悔棋_agent批准两端一致() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let ctx = &rig.ctx;

    // 人先落一手，再请求悔棋。
    front.cmd(UiCommand::Place { x: 3, y: 3 });
    until(|| agent.snapshot()["moveCount"] == json!(1), 15, "落子同步到 B").await;
    front.cmd(UiCommand::RequestUndo);
    until(|| agent.snapshot()["confirmReq"]["kind"] == json!("undo"), 15, "B 应收到悔棋请求").await;

    // Agent 两段式批复：write in/confirm "approve" + submit。
    act(ctx, "write", json!({ "path": "/game/in/confirm", "content": "approve" }))
        .await
        .expect("暂存批复");
    let receipt = act(ctx, "submit", json!({ "path": "/game/in/confirm" })).await.expect("提交批复");
    assert_eq!(receipt["ok"], json!(true));
    assert_eq!(receipt["action"], json!("confirm"));
    assert_eq!(receipt["kind"], json!("undo"));
    assert_eq!(receipt["approved"], json!(true));

    // 两端一致：手数都归零、盘面都空。
    until(
        || agent.snapshot()["moveCount"] == json!(0) && front.snapshot()["moveCount"] == json!(0),
        15,
        "同意悔棋后两端手数应归零",
    )
    .await;
    assert_eq!(agent.snapshot()["board"], front.snapshot()["board"], "两端棋盘一致");
}

/* ---------------- Agent 请求悔棋被拒（事件 approved:false） ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_agent悔棋被拒_事件approved_false() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let ctx = &rig.ctx;
    front.cmd(UiCommand::Place { x: 3, y: 3 });
    until(|| agent.snapshot()["moveCount"] == json!(1), 15, "落子同步到 B").await;

    // Agent 发起悔棋请求（两段式）。
    act(ctx, "write", json!({ "path": "/game/in/request", "content": "undo" }))
        .await
        .expect("暂存请求");
    let receipt = act(ctx, "submit", json!({ "path": "/game/in/request" })).await.expect("发送请求");
    assert_eq!(receipt["kind"], json!("undo"), "请求回执：{receipt}");

    // 人侧收到弹窗并拒绝。
    until(|| front.snapshot()["confirmReq"]["kind"] == json!("undo"), 15, "A' 应收到请求").await;
    front.cmd(UiCommand::ConfirmDecline);

    // Agent 侧事件送达 approved:false——拒绝必须可观测（等待方要看到拒绝）。
    until(
        || {
            rig.queue.history().iter().any(|e| matches!(e,
                GameEvent::RequestResolved { kind, approved, .. } if kind == "undo" && !*approved))
        },
        15,
        "B 应物化出 request_resolved(approved:false)",
    )
    .await;
    // 局面未被拒着的请求撼动，对局继续（moveCount 仍 1）。
    assert_eq!(agent.snapshot()["moveCount"], json!(1));
}

/* ---------------- 聊天双向全文 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_双向全文() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let ctx = &rig.ctx;

    // Agent → 人：write + submit 后对方收到全文。
    let text = "你好，请多指教，这局请手下留情。";
    act(ctx, "write", json!({ "path": "/game/in/chat", "content": text })).await.expect("暂存聊天");
    let receipt = act(ctx, "submit", json!({ "path": "/game/in/chat" })).await.expect("发送聊天");
    assert_eq!(receipt["ok"], json!(true));
    assert_eq!(receipt["action"], json!("chat"));
    until(
        || front.snapshot()["chatLog"].as_array().is_some_and(|l| l.iter().any(|m| m["text"] == json!(text))),
        15,
        "人应收到 Agent 聊天全文",
    )
    .await;

    // 人 → Agent：全文进 chatLog，/game/chat 散文可读，事件流有 chat 条目。
    let reply = "请多指教，开始吧。";
    front.cmd(UiCommand::SendChat(reply.into()));
    until(
        || agent.snapshot()["chatLog"].as_array().is_some_and(|l| l.iter().any(|m| m["text"] == json!(reply))),
        15,
        "Agent 应收到人的聊天全文",
    )
    .await;
    let chat = goptop_agent::vfs::syn_chat(&agent.snapshot());
    assert!(chat.contains(text) && chat.contains(reply), "chat 文件应含双向全文：{chat}");
    assert!(
        rig.queue.history().iter().any(|e| matches!(e,
            GameEvent::Chat { text: t, .. } if t == reply)),
        "事件流应有 chat 条目（附全文）"
    );
}

/* ---------------- 认输：循环 terminate、两端 winner 一致 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resign_循环终止两端一致() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();

    // 循环剧本只有一步：write in/resign + submit → terminate（退出条件 2）。
    let script = Arc::new(MockScript::new(vec![ws("/game/in/resign", "这局我撑不住了，认输。")]));
    let (_stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "white");
    // 120s 与五连用例同预算：配对类用例的传播耗时随整机负载波动（并行用例多时
    // 60s 曾被击穿），预算给足、判定仍靠退出条件本身。
    let outcome = tokio::time::timeout(Duration::from_secs(120), handle)
        .await
        .expect("认输应收束")
        .expect("循环任务不 panic");
    assert!(matches!(outcome, LoopStop::Resigned), "resign 的 terminate：{outcome:?}");

    // 两端 winner 一致：Agent（白）认输 → 黑胜。
    until(
        || !agent.snapshot()["winner"].is_null() && !front.snapshot()["winner"].is_null(),
        15,
        "认输后两端都应有 winner",
    )
    .await;
    assert_eq!(agent.snapshot()["winner"], front.snapshot()["winner"], "两端胜者判定必须一致");
    assert_eq!(front.snapshot()["winner"], json!("black"));
}

/* ---------------- 文件面：合成、变体、翻页、grep、拒绝 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 文件面_合成与翻页与拒绝() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let ctx = &rig.ctx;

    // 造现场：人落 (7,7)，Agent 暂存 (8,8)（幽灵子上盘可见）。
    front.cmd(UiCommand::Place { x: 7, y: 7 });
    until(|| agent.snapshot()["moveCount"] == json!(1), 15, "落子同步到 B").await;
    act(ctx, "write", json!({ "path": "/game/in/move", "content": "8,8" })).await.expect("暂存");

    let read = |p: &str| act(ctx, "read", json!({ "path": p }));

    // /index 与 /rules 可读。
    let idx = read("/index").await.expect("index 可读");
    assert!(idx["content"].as_str().unwrap().contains("/game/status"));
    assert!(idx["content"].as_str().unwrap().contains("Stage-then-submit"));
    let rules = read("/game/rules").await.expect("rules 可读");
    assert!(rules["content"].as_str().unwrap().contains("Gomoku rules"));

    // /game/board 与快照一致：stones/staged 与镜像对上。
    let board: Value = serde_json::from_str(
        read("/game/board").await.expect("board 可读")["content"].as_str().unwrap(),
    )
    .expect("board 是 JSON");
    let snap = agent.snapshot();
    assert_eq!(board["stones"]["black"], json!([[7, 7]]), "board 与快照 stones 一致");
    assert_eq!(board["staged_move"], json!({ "x": 8, "y": 8 }), "暂存子上盘");
    assert_eq!(board["last_move"], json!({ "by": "black", "x": 7, "y": 7 }));
    assert_eq!(board["to_move"], snap["toMove"]);

    // /game/board/grid 每格值正确（含 staged）。
    let grid: Value = serde_json::from_str(
        read("/game/board/grid").await.expect("grid 可读")["content"].as_str().unwrap(),
    )
    .expect("grid 是 JSON");
    let g = grid["grid"].as_array().unwrap();
    assert_eq!(g.len(), 15, "15 行");
    assert_eq!(g[0].as_array().unwrap().len(), 15, "15 列");
    assert_eq!(g[7][7], json!("black"));
    assert_eq!(g[8][8], json!("staged"), "暂存格呈现 staged");
    let non_empty: usize =
        g.iter().flat_map(|r| r.as_array().unwrap()).filter(|c| c.as_str() != Some("empty")).count();
    assert_eq!(non_empty, 2, "全盘只有黑子一枚 + 暂存一枚");

    // ascii / pretty 变体。
    let ascii = read("/game/board/ascii").await.expect("ascii 可读")["content"].as_str().unwrap().to_string();
    assert!(ascii.starts_with("15x15 gomoku"));
    assert!(ascii.contains("\n 7  . . . . . . . X . ."), "row7 黑子在位：{ascii}");
    assert!(ascii.contains("\n 8  . . . . . . . . * ."), "row8 暂存 * 在位");
    let pretty = read("/game/board/pretty").await.expect("pretty 可读")["content"].as_str().unwrap().to_string();
    assert!(pretty.contains('┼') && pretty.contains('●') && pretty.contains('◍'), "pretty 框线+子：{pretty}");

    // image.png：Mock 按协议矩阵走最富路径——read 回执带 base64 PNG（占位文案
    // 只属于不支持视觉的协议；逐字基线见计划「格式样例」节，此处按字面锚定）。
    let img = read("/game/board/image.png").await.expect("image 可读");
    assert_eq!(img["ok"], json!(true));
    assert_eq!(img["image"]["mime"], json!("image/png"));
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        img["image"]["data_base64"].as_str().expect("回执带 base64"),
    )
    .expect("base64 合法");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "解开即 PNG 签名");

    // /game/history：JSONL 行形。
    let hist = read("/game/history").await.expect("history 可读")["content"].as_str().unwrap().to_string();
    assert_eq!(hist.lines().count(), 1);
    assert_eq!(hist.trim(), r#"{"n":1,"by":"black","x":7,"y":7}"#);

    // /game/history/<n> 五变体（/index 逐字承诺 same five variants——条条路径
    // 经 read 工具可达，不是只有 JSON 直调库函数）。
    let h1: Value = serde_json::from_str(
        read("/game/history/1").await.expect("history/1 可读")["content"].as_str().unwrap(),
    )
    .expect("history/1 是 JSON");
    assert_eq!(h1["stones"]["black"], json!([[7, 7]]));
    assert_eq!(h1["after_move"], json!(1));
    let h1g: Value = serde_json::from_str(
        read("/game/history/1/grid").await.expect("history/1/grid 可读")["content"].as_str().unwrap(),
    )
    .expect("grid 变体是 JSON");
    assert_eq!(h1g["grid"][7][7], json!("black"));
    let h1a = read("/game/history/1/ascii").await.expect("ascii 变体可读")["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(h1a.contains('X'), "{h1a}");
    let h1p = read("/game/history/1/pretty").await.expect("pretty 变体可读")["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(h1p.contains('●'), "{h1p}");
    let h1i = read("/game/history/1/image.png").await.expect("history 图像变体可读");
    assert_eq!(h1i["image"]["mime"], json!("image/png"), "history/<n> 也有图像变体");
    let err = read("/game/history/5").await.unwrap_err();
    assert!(err.message().contains("out of range"), "越界 history/<n>：{err:?}");
    let err = read("/game/history/1/jpeg").await.unwrap_err();
    assert!(err.message().contains("unknown history variant"), "未知变体：{err:?}");

    // grep：history 命中指定手数；board 文件里搜得到暂存。
    let hits = act(ctx, "grep", json!({ "pattern": r#""x":7,"y":7"#, "path": "/game/history" }))
        .await
        .expect("grep history");
    // /game/history（清单文件）与 /game/history/1（该手局面）都含此坐标——两处都命中。
    assert_eq!(hits["total"], json!(2), "命中第 1 手：{hits}");
    let paths: Vec<&str> = hits["matches"].as_array().unwrap().iter().map(|m| m["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"/game/history/1"), "history/<n> 展开参与 grep：{paths:?}");
    let hits = act(ctx, "grep", json!({ "pattern": "staged", "path": "/game/board" }))
        .await
        .expect("grep board");
    assert!(hits["total"].as_u64().unwrap() >= 1, "board 文件里应搜得到暂存：{hits}");

    // /game/events：seq 连续完整、可 grep 回查。
    until(
        || rig.queue.history().iter().any(|e| matches!(e, GameEvent::Move { .. })),
        10,
        "move 事件应已物化",
    )
    .await;
    let events = read("/game/events").await.expect("events 可读")["content"].as_str().unwrap().to_string();
    assert_eq!(events.lines().count(), rig.queue.history().len(), "events 文件行数与队列历史一致");
    for (i, e) in rig.queue.history().iter().enumerate() {
        assert_eq!(e.seq(), (i + 1) as u64, "seq 连续无空洞");
    }
    let hits = act(ctx, "grep", json!({ "pattern": r#""t":"move""#, "path": "/game/events" }))
        .await
        .expect("grep events");
    assert!(hits["total"].as_u64().unwrap() >= 1, "应能 grep 回查 move 事件：{hits}");

    // read 的 offset/limit 翻页 + total_lines（长文件用 /memory 造 10 行）。
    let ten: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    act(ctx, "write", json!({ "path": "/memory/lines.txt", "content": ten.trim_end() }))
        .await
        .expect("写 10 行记忆");
    let page = act(ctx, "read", json!({ "path": "/memory/lines.txt", "offset": 2, "limit": 3 }))
        .await
        .expect("翻页");
    assert_eq!(page["total_lines"], json!(10));
    assert_eq!(page["content"], json!("line3\nline4\nline5"), "offset=2 limit=3 取第 3..5 行");

    // 写只读路径拒绝（计划样例文案逐字）。
    let err = act(ctx, "write", json!({ "path": "/game/board", "content": "x" })).await.unwrap_err();
    assert_eq!(
        err.message(),
        "/game/board is read-only (synthesized from live game state).",
        "只读文案逐字"
    );
    let err = act(ctx, "write", json!({ "path": "/index", "content": "x" })).await.unwrap_err();
    assert_eq!(err.message(), "/index is read-only (generated index).");

    // in/confirm 无待决 submit 失败（计划样例文案逐字）。
    act(ctx, "write", json!({ "path": "/game/in/confirm", "content": "approve" })).await.expect("暂存批复");
    let err = act(ctx, "submit", json!({ "path": "/game/in/confirm" })).await.unwrap_err();
    assert_eq!(
        err.message(),
        "no pending request. Check pending_request in /game/status.",
        "无待决文案逐字"
    );

    // in/score 非计分态 submit 失败。
    act(ctx, "write", json!({ "path": "/game/in/score", "content": "ok" })).await.expect("暂存计分");
    let err = act(ctx, "submit", json!({ "path": "/game/in/score" })).await.unwrap_err();
    assert!(err.message().contains("not in scoring"), "非计分态拒：{err:?}");

    // 未知路径：错误附 /index 提示。
    let err = read("/game/nothing").await.unwrap_err();
    assert!(err.message().contains("/index"), "未知路径要指路 /index：{err:?}");
}

/* ---------------- 围棋双 pass → scoring → 双确认到 score_result ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn go_双pass计分到score_result() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("go", 9, SeatColor::White).await; // Agent 执黑
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    let queue = rig.queue.clone();
    assert_eq!(agent.snapshot()["myColor"], json!("black"));

    // 人侧 driver：轮到白就 Pass（至多两手）；进计分态就确认计分。
    let f2 = front.clone();
    tokio::spawn(async move {
        let front = f2;
        for _ in 0..2 {
            if front.snapshot()["scoring"] == json!(true) {
                break;
            }
            let ok = front.wait_until(
                &mut |s| {
                    s["phase"] == json!("playing") && s["winner"].is_null()
                        && s["toMove"] == s["myColor"]
                },
                Duration::from_secs(30),
            );
            if !ok {
                return;
            }
            front.cmd(UiCommand::Pass);
        }
        let ok = front.wait_until(
            &mut |s| s["scoring"] == json!(true) && s["winner"].is_null(),
            Duration::from_secs(30),
        );
        if ok {
            front.cmd(UiCommand::ConfirmScore);
        }
    });

    // 剧本：Agent 黑 Pass 一步；其后是等待回合（计分自动确认不消耗剧本），
    // winner 出现后的任意一步工具调用收束 GameOver（read 填充与告别步都安全）。
    let script = Arc::new(MockScript::new(vec![
        ws("/game/in/move", "pass"),
        peek("/game/status"),
        peek("/game/status"),
        peek("/game/status"),
        peek("/game/status"),
        peek("/game/status"),
        peek("/game/status"),
        peek("/game/status"),
        ws("/game/in/chat", "多谢指教。"),
    ]));
    let (_stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "black");
    let outcome = tokio::time::timeout(Duration::from_secs(120), handle)
        .await
        .expect("计分局应收束")
        .expect("循环任务不 panic");
    assert!(matches!(outcome, LoopStop::GameOver), "双 pass 计分终局应收束 GameOver：{outcome:?}");

    // score_result 落地且两端一致。
    until(
        || !agent.snapshot()["scoreResult"].is_null() && !front.snapshot()["scoreResult"].is_null(),
        15,
        "双方确认后两端应有 scoreResult",
    )
    .await;
    assert_eq!(
        agent.snapshot()["scoreResult"],
        front.snapshot()["scoreResult"],
        "两端计分结果一致"
    );
    assert_eq!(agent.snapshot()["winner"], front.snapshot()["winner"], "两端胜者一致");
    // Agent 侧的事件流应完整走到 scoring_started。
    assert!(
        queue.history().iter().any(|e| matches!(e, GameEvent::ScoringStarted { .. })),
        "事件流应有 scoring_started"
    );
}

/* ---------------- delegate：默认关 / 真实子循环（结论回父、深度 1 不嵌套） ---------------- */

#[tokio::test]
async fn delegate_开关与越权拦截() {
    // 免配对：delegate 的语义不依赖对局（home 相位的会话即可搭 ctx）。
    let ctx = home_ctx();

    // 默认关：工具不在清单（tools_for 已有单测），执行口的越权拦截也要在。
    let err = act(&ctx, "delegate", json!({ "task": "想一下" })).await.unwrap_err();
    assert!(err.message().contains("not available in this mode"), "关闭时点名调用被拦：{err:?}");
    assert!(err.message().contains("Available tools"), "错误里给可用清单");

    // 开启但未装配 runner：装配缺口回模型说明，不炸局。
    let mut on = ctx;
    on.subagent_enabled = true;
    let err = act(&on, "delegate", json!({ "task": "想一下" })).await.unwrap_err();
    assert!(err.message().contains("no subagent runner"), "未装配 runner 的说明：{err:?}");

    // 装配 Mock runner：结论原样回父代理（registry 的 delegate 接线）。
    struct FakeSub;
    use async_trait::async_trait;
    #[async_trait]
    impl goptop_agent::registry::SubagentRunner for FakeSub {
        async fn run(&self, task: String) -> Result<String, String> {
            Ok(format!("结论（针对：{task}）：B 点最优。"))
        }
    }
    on.subagent = Some(Arc::new(FakeSub));
    let out = act(&on, "delegate", json!({ "task": "评估候选点" })).await.expect("delegate 成功");
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["conclusion"], json!("结论（针对：评估候选点）：B 点最优。"));

    // 子循环失败：RespondToModel 级文本回父（不是 Fatal——父代理该知道原因并继续）。
    struct FailingSub;
    #[async_trait]
    impl goptop_agent::registry::SubagentRunner for FailingSub {
        async fn run(&self, _task: String) -> Result<String, String> {
            Err("预算打满".into())
        }
    }
    on.subagent = Some(Arc::new(FailingSub));
    let err = act(&on, "delegate", json!({ "task": "评估候选点" })).await.unwrap_err();
    assert!(matches!(err, ToolError::RespondToModel(_)), "子循环失败是业务错误：{err:?}");
}

/// 真实子循环（SubagentLoop，计划测试项「启用后 Mock 子循环回结论、深度 1 不嵌套」）：
/// 同 LLM（Mock）、独立上下文（从任务空史起跑）、只读工具面；模型越权点名 write
/// 被执行口拒绝且不落任何副作用——深度 1 不靠模型守约。
#[tokio::test]
async fn delegate_真实子循环_结论回父且深度1不嵌套() {
    let ctx = home_ctx();
    let sub_script = Arc::new(MockScript::new_steps(vec![
        // 第 1 轮：模型守约，read /game/status。
        MockStep::Reply(tool_step(vec![("read", json!({ "path": "/game/status" }))])),
        // 第 2 轮：越权点名 write（深度 1 的执行口必须拒、绝不能执行）。
        MockStep::Reply(tool_step(vec![(
            "write",
            json!({ "path": "/game/in/move", "content": "7,7" }),
        )])),
        // 第 3 轮：收束结论（零工具调用 = 最终文本）。
        MockStep::Reply(pure_text("结论：黑方 row0 有三连威胁，B 点最优。")),
    ]));
    let subagent = Arc::new(SubagentLoop::new(
        Arc::new(LlmClient::Mock(Arc::clone(&sub_script))),
        clone_ctx(&ctx),
    ));
    let mut on = clone_ctx(&ctx);
    on.subagent_enabled = true;
    on.subagent = Some(subagent);

    let out = act(&on, "delegate", json!({ "task": "评估候选点" })).await.expect("delegate 成功");
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["conclusion"], json!("结论：黑方 row0 有三连威胁，B 点最优。"), "子循环结论回父");

    // 深度 1 的证据（子循环留档的请求原文）：三轮请求的工具面都只有 read/grep
    // ——没有 delegate（结构上无从嵌套）、没有 write/submit。
    let recorded = sub_script.recorded();
    assert_eq!(recorded.len(), 3, "三步剧本恰好走完：{}", recorded.len());
    for req in &recorded {
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read", "grep"], "子循环工具面只读：{names:?}");
    }
    // 独立上下文的起点：第 1 轮请求只有任务文本一条 user 消息，且带子代理系统提示词。
    assert_eq!(recorded[0].messages.len(), 1, "从任务文本空史起跑");
    assert!(
        recorded[0].system.contains("read-only analysis subagent"),
        "子代理有自己的系统提示词：{}",
        recorded[0].system
    );
    // 越权 write 被拒：错误文本回填进第 3 轮请求的消息史。
    let rejected = recorded[2]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            Block::ToolResult { content, is_error: true, .. } => Some(content.as_str()),
            _ => None,
        })
        .any(|c| c.contains("not available to the subagent"));
    assert!(rejected, "越权点名要被拒（错误文本入史）：{:?}", recorded[2].messages);

    // 副作用零落地：越权 write 没有进暂存区、没有落子（同一个 Arc 共享可证）。
    assert_eq!(ctx.staging.peek(InFile::Move), None, "越权 write 不得入暂存区");
    assert_eq!(ctx.player.snapshot()["moveCount"], json!(0), "越权 write 不得落子");
}

/// 独立小预算：打满即以 Err 回父代理（不占父循环预算，也不静默续跑）。
#[tokio::test]
async fn delegate_子代理预算打满回错() {
    let ctx = home_ctx();
    let sub_script = Arc::new(MockScript::new(vec![
        tool_step(vec![("read", json!({ "path": "/game/status" }))]),
        tool_step(vec![("grep", json!({ "pattern": "to_move", "path": "/game/status" }))]),
    ]));
    let mut runner = SubagentLoop::new(Arc::new(LlmClient::Mock(Arc::clone(&sub_script))), clone_ctx(&ctx));
    runner.max_llm_calls = 1; // 独立预算压到 1：第一轮工具调用后必须收手
    let err = runner.run("深思".into()).await.unwrap_err();
    assert!(err.contains("budget exhausted"), "{err}");
    assert_eq!(sub_script.recorded().len(), 1, "预算=1 次调用后不得再调 LLM");
}

/* ---------------- 纯文字轮不终止 ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 纯文字轮不终止() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();
    let front = rig.front.clone();
    drive_front_places(front.clone(), vec![(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)]);

    // 剧本：①零工具调用的纯文字（循环必须注入提醒继续）→ ②只 chat 的回合
    // （会话照常存活）→ ③正常落子到终局 → 聊天垫步 + 告别。
    let mut steps = vec![pure_text("我想想该下哪……（只回了文字）"), ws("/game/in/chat", "我在想。")];
    steps.extend(double_moves(&[(7, 2), (8, 2), (9, 2), (10, 2), (11, 2), (12, 2)]));
    steps.push(ws("/game/in/chat", "好局！"));
    steps.push(ws("/game/in/chat", "好局！"));
    let script = Arc::new(MockScript::new(steps));

    let (_stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "white");
    let outcome = tokio::time::timeout(Duration::from_secs(120), handle)
        .await
        .expect("循环应收束")
        .expect("循环任务不 panic");

    // 会话未被纯文字轮终止：一路打到 GameOver。
    assert!(matches!(outcome, LoopStop::GameOver), "纯文字后循环必须继续到终局：{outcome:?}");
    // 注入提醒进了下一轮请求：第 2 次请求（首次响应后）含 TextOnly 提醒文案；
    // 第 3 次（chat+submit 之后）不再提醒（有工具调用的轮次是行动）。
    let recorded = script.recorded();
    assert!(recorded.len() >= 3, "至少三轮 LLM 调用：{}", recorded.len());
    let second = last_user_text(&recorded[1]);
    assert!(second.contains("text is NOT an action"), "第 2 轮请求应注入 TextOnly 提醒：{second}");
    assert!(
        second.contains("Game events since your last turn:")
            || second.contains("Waiting for the opponent"),
        "提醒随事件块/心跳同一条 user 消息注入：{second}"
    );
    let third = last_user_text(&recorded[2]);
    assert!(!third.contains("text is NOT an action"), "工具调用轮不得再提醒：{third}");
    assert_eq!(agent.snapshot()["phase"], json!("playing"), "会话未停");
}

/* ---------------- 内置事件自动推送（无需查询工具） ---------------- */

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 事件自动推送_无需查询工具() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig("gomoku", 15, SeatColor::Black).await;
    let agent = rig.ctx.player.clone();

    // 人先落子：事件进队列后循环才起跑——下一轮注入的消息必须含坐标，
    // 且剧本里没有任何等待/查询类工具（事件不是模型问来的）。
    rig.front.cmd(UiCommand::Place { x: 7, y: 7 });
    until(|| rig.queue.pending() > 0, 15, "人落子的 move 事件应进队列").await;

    let script = Arc::new(MockScript::new(vec![
        ws("/game/in/chat", "看到你的第一手了。"),
        peek("/game/status"),
        peek("/game/status"),
        ws("/game/in/chat", "继续。"),
    ]));
    let (stop, handle) = spawn_loop(rig.ctx, Arc::clone(&script), "white");

    // 第 2 轮请求发出即停（第 2 轮的注入文本来自第 1 轮后的排空）。
    let s = Arc::clone(&stop);
    let probe = Arc::clone(&script);
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while probe.recorded().len() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        s.store(true, Ordering::Relaxed);
    });
    let outcome = tokio::time::timeout(Duration::from_secs(60), handle)
        .await
        .expect("循环应收束")
        .expect("循环任务不 panic");
    assert!(matches!(outcome, LoopStop::Stopped), "stop 置位应收束 Stopped：{outcome:?}");

    // 断言：第 2 轮请求的 user 注入消息里含 move 事件坐标（自动推送的直接证据）。
    let recorded = script.recorded();
    assert!(recorded.len() >= 2, "应至少两轮请求：{}", recorded.len());
    let second = last_user_text(&recorded[1]);
    assert!(second.contains("<event>"), "注入 <event> 块：{second}");
    assert!(second.contains(r#""t":"move""#), "move 事件在场：{second}");
    assert!(second.contains(r#""x":7,"y":7"#), "坐标 (7,7) 在场：{second}");
    // 内置模式工具面无轮询工具（wait_events 仅 MCP）——事件不可能被查来。
    assert!(
        recorded[0].tools.iter().all(|t| t.name != "wait_events"),
        "内置模式不得携带 wait_events"
    );
    // 会话未停（Stopped ≠ 局散）。
    assert_eq!(agent.snapshot()["phase"], json!("playing"));
}

/* ---------------- compact：循环侧接线与超窗兜底 ---------------- */

/// compact 挂点在循环里的接线（计划测试项「usage 计量」的循环形）：provider
/// usage 为主 + 其后消息估算的触发线在**循环内**真实触发压缩——第 3 步上报
/// input_tokens=50_000（纯 chars/4 估算的旧计量在这里只有 ≈30k，够不到触发线
/// 43616）。home 相位会话没有可 resign 的活局，循环以 max_llm_calls 预算收束。
/// 摘要请求是独立一次 LLM 调用、不带工具面；压缩后的请求以
/// <compaction-summary> 摘要消息开头。
#[tokio::test]
async fn compact_循环接线_usage触发与摘要换血() {
    let ctx = home_ctx();
    let big = "x".repeat(120_000); // ≈30k tokens：把可切空间撑过 KEEP_RECENT(20k)
    let script = Arc::new(MockScript::new(vec![
        pure_text("我在想。"),
        ws("/game/in/chat", "先打个招呼。"),
        ChatResponse {
            content: big,
            tool_calls: vec![],
            stop: StopReason::EndTurn,
            usage: Usage { input_tokens: 50_000, output_tokens: 5, cache_read_tokens: 0, cache_write_tokens: 0 },
        },
        pure_text("## Game state\n(gomoku 15x15, 对局摘要)"), // summarize 的独立请求
        pure_text("压缩换血后我接着看局面。"), // 压缩后的第一轮（预算内最后一轮）
    ]));
    // ctx_limit=60_000 → 触发线 43616 < usage 50000：压缩发生本身即「provider
    // usage 参与判定」的直接证据；max_llm_calls=4 只数主循环调用（摘要不计）。
    let deps = build_deps(
        ctx,
        Arc::clone(&script),
        "white",
        LoopConfig { max_llm_calls: 4, ctx_limit: 60_000 },
    );
    let outcome = tokio::time::timeout(Duration::from_secs(60), run(deps))
        .await
        .expect("循环应收束");
    assert!(matches!(outcome.stop, LoopStop::BudgetExhausted), "{outcome:?}");
    assert_eq!(outcome.stats.compactions, 1, "usage 触发线应真实触发一次压缩：{outcome:?}");

    // 请求序：0/1/2 正常轮 → 3 摘要请求（1 条消息、无工具）→ 4 压缩后的第一轮。
    let recorded = script.recorded();
    assert_eq!(recorded.len(), 5, "四主调用+一次摘要：{}", recorded.len());
    let summary_req = &recorded[3];
    assert_eq!(summary_req.messages.len(), 1, "摘要请求只有一条 user 消息");
    assert!(summary_req.tools.is_empty(), "摘要模型不得携带工具面");
    let convo = last_user_text(summary_req);
    assert!(convo.contains("<conversation>") && convo.contains("conversation to summarize"), "{convo}");
    // 压缩换血：压缩前 5 条 → 压缩后 6 条（摘要消息替换了前 2 条 + 大行史仍在尾段）。
    assert_eq!(recorded[2].messages.len(), 5, "压缩前的正常轮");
    let after = &recorded[4];
    assert_eq!(after.messages.len(), 6, "压缩后：摘要 + 保留尾段");
    let head = match &after.messages[0].content[0] {
        Block::Text { text } => text,
        other => panic!("首条应是文本块，got {other:?}"),
    };
    assert!(head.contains("<compaction-summary>"), "压缩后结构以摘要消息开头：{head}");
}

/// 超窗兜底链（计划 compact 节）：ContextWindowExceeded → 强制压缩（摘要请求）
/// → 同轮重试一次（压缩后的请求以摘要消息开头）→ 预算内收束；错误体解析出的
/// 模型实限生效（生效上限 = min(用户上限, 模型实限)，50_000 < 176_000）。
#[tokio::test]
async fn compact_超窗兜底_强制压缩后同轮重试() {
    let ctx = home_ctx();
    let script = Arc::new(MockScript::new_steps(vec![
        MockStep::Error(LlmError::ContextWindowExceeded { model_limit: Some(50_000) }),
        MockStep::Reply(pure_text("## Game state\n(紧急压缩摘要)")), // summarize
        MockStep::Reply(pure_text("压缩后这轮我重新读局面。")), // 重试轮
    ]));
    let deps = build_deps(ctx, Arc::clone(&script), "white", LoopConfig { max_llm_calls: 2, ..LoopConfig::default() });
    let outcome = tokio::time::timeout(Duration::from_secs(60), run(deps))
        .await
        .expect("循环应收束");
    assert!(matches!(outcome.stop, LoopStop::BudgetExhausted), "压缩后重试应走到正常轮：{outcome:?}");
    assert_eq!(outcome.stats.compactions, 1, "超窗兜底的强制压缩");

    // 请求序：0 初次请求（超窗）→ 1 摘要请求 → 2 重试轮（以摘要消息开头）。
    let recorded = script.recorded();
    assert_eq!(recorded.len(), 3, "初次+摘要+重试：{}", recorded.len());
    assert_eq!(recorded[1].messages.len(), 1, "摘要请求单消息");
    let head = match &recorded[2].messages[0].content[0] {
        Block::Text { text } => text,
        other => panic!("重试轮首条应是摘要消息，got {other:?}"),
    };
    assert!(head.contains("<compaction-summary>"), "强制压缩已换血：{head}");
}

/// 重试一次仍超窗 → Fatal（兜底链的终点，不空转烧钱）。
#[tokio::test]
async fn compact_超窗兜底_再失败fatal() {
    let ctx = home_ctx();
    let script = Arc::new(MockScript::new_steps(vec![
        MockStep::Error(LlmError::ContextWindowExceeded { model_limit: None }),
        MockStep::Reply(pure_text("(紧急压缩摘要)")),
        MockStep::Error(LlmError::ContextWindowExceeded { model_limit: None }),
    ]));
    let deps = build_deps(ctx, Arc::clone(&script), "white", LoopConfig::default());
    let outcome = tokio::time::timeout(Duration::from_secs(60), run(deps))
        .await
        .expect("循环应收束");
    match outcome.stop {
        LoopStop::Fatal(m) => {
            assert_eq!(m, "context window exceeded even after emergency compaction");
        }
        other => panic!("二次超窗应 Fatal：{other:?}"),
    }
    // 三次调用：初次、摘要、重试——再失败即收，没有第四次。
    assert_eq!(script.recorded().len(), 3);
}
