//! Agent 出口 —— Web 端内置模式（feature `agent`，阶段⑤契约 §2.2b/§5.3.1）。
//!
//! 桌面壳（src-tauri/src/agent.rs）的本端镜像：结会话对、跑
//! `agent_loop::run` 决策循环，全部在 `spawn_local` 微任务里转；本模块持有运行
//! 表（run id → [`Run`]）并把循环侧的状态/统计/暂存着法/工具日志翻译成
//! `agent_*` 导出的线上契约。决策与工具的全部语义都在 goptop-agent，这里一行
//! 不重做；与桌面的唯一冻结点是 §5.3.1 的 JSON 契约（代码是镜像，不是共享）。
//!
//! **会话拓扑（与桌面同构）**：A'（人的专用会话）由**前端**经
//! `WasmSession.new_agent` 创建并 `agent_bind(key)` 登记；Hub 只创建 B（无头席，
//! HookHost 包 WebHost）并把 A'、B 结对。方向随执色：
//! - **我执黑**：A' 邀请（bind 代发）。`agent_start` 后 Hub 读 A' 的邀请链接、
//!   携链创建 B，泵到双端 playing。
//! - **我执白**：B 先建局邀请，链接经 `agent_status.detail` 送回前端；前端以该
//!   链接经 Boot 路径创建 A' 并 bind——A' 的回执经同页 BC 自动送达 B。
//!
//! **与桌面的三处结构性差异**（契约 §6 偏差清单）：
//! 1. **取消是协作式**：wasm 单线程没有 `select!` 硬取消——`agent_stop` 先认输、
//!    `delay(600)` 定拍，再置 `abort`；配对轮询与循环的既有检查点（每拍
//!    `is_live()` / 每轮 LLM 调用前）收口。
//! 2. **B 的泵是 spawn_local 常驻循环**（50ms 一拍），随 Run 拆除置停机标志退出；
//!    泵走 [`drain_agent`]——Emit/Notice/Nav/SetStorage 四类 Effect 过 HookHost
//!    （watch 推送、ack 物化、身份不落盘），其余落回 bridge（与桌面
//!    `bridge::pump(core, host)` 的宿主中介同形）。
//! 3. **配对原语在本模块落地**（`pair.rs` 以 NativeSession 为硬类型不上 wasm）：
//!    A' 归前端、B 由 Hub 建，镜像桌面壳的 pair_seats/wait_invite/wait_playing。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use goptop_agent::agent_loop::{LoopConfig, LoopDeps, LoopStop, SubagentLoop, DEFAULT_CTX_LIMIT, DEFAULT_MAX_LLM_CALLS};
use goptop_agent::llm::{
    Block, ChatRequest, HttpChannel, LlmClient, LlmConfig, LlmError, Msg, Protocol, Role,
    WebHttpChannel,
};
use goptop_agent::player::{
    EmitWatch, EventQueue, HookHost, PlatformHost, PlayerHandle, event_baseline, run_event_pump,
};
use goptop_agent::prompt::{PromptCfg, build_system_prompt};
use goptop_agent::registry::ToolCtx;
use goptop_agent::store::VfsStore;
use goptop_agent::store_web::WebStore;
use goptop_agent::vfs::{InFile, Staging};
use goptop_agent::{Driver, agent_loop};
use goptop_net::identity::gen_user_id;
use goptop_net::session::{Event, Session, UiCommand};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::io::load_stun_urls_from;
use crate::{SharedCore, bridge, drain_core, fresh_peer_id, now_ms, queue_event_to, rand_u32, window};

/// agent_stop 的认输定拍（与桌面同值）：cmd(Resign) 在本地状态机即刻生效，但要
/// 给 RTC 数据面一拍把「认输」发给对面——立刻拆会话的话对面看到的就不是认输
/// 而是断线。
const RESIGN_SETTLE_MS: u64 = 600;

/// 等 agent_bind 的登记值出现的时长（桌面 BIND_WAIT_SECS 同值同因：覆盖前端的
/// 等待/建会话/登记全链路，我执白方向前端 waitAgentLink 50s 后才 bind）。
const BIND_WAIT_SECS: u64 = 90;

/* ---------------- 用户执色（pair.rs 是 native 专用，本模块自带镜像） ---------------- */

/// 用户（人）执色 —— 决定配对方向（值域/语义与 goptop_agent::pair::SeatColor 一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeatColor {
    /// 人执黑（默认）：A' 邀请，B 入局。
    Black,
    /// 人执白：B 邀请，A' 携链入局。
    White,
}

impl SeatColor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::White => "white",
        }
    }

    fn opponent(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

fn parse_seat(s: &str) -> Result<SeatColor, String> {
    match s {
        "black" => Ok(SeatColor::Black),
        "white" => Ok(SeatColor::White),
        other => Err(format!("myColor 必须是 \"black\" 或 \"white\"，得到 {other:?}")),
    }
}

/// 开局配置（`agent_start` 的 cfgJson，契约字段：driver/kind/size/myColor/
/// agentName/uiLang；agentName 缺省「Agent」，uiLang 缺省按简体中文）。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartCfg {
    driver: Driver,
    kind: String,
    size: u16,
    my_color: String,
    agent_name: Option<String>,
    ui_lang: Option<String>,
}

/* ---------------- B 席：注册表、WebPlayer、宿主感知的泵 ---------------- */

/// B 席注册表条目：核的弱引用（强引用归泵任务与配对/循环任务）+ 宿主
/// （WebPlayer 的泵要过 HookHost，见 [`drain_agent`]）。
struct SeatEntry {
    core: Weak<RefCell<crate::Core>>,
    hook: Arc<HookHost>,
}

thread_local! {
    /// A' 注册表（id → 弱引用）：A' 归 JS 所有，弱引用使 dispose 后条目自动失效；
    /// 单局互斥由 Hub 保证 → 至多一个 A'，旧条目自然过期。
    static FRONT: RefCell<HashMap<String, Weak<RefCell<crate::Core>>>> = RefCell::new(HashMap::new());
    static FRONT_NEXT: Cell<u32> = Cell::new(1);
    /// B 席注册表（id → 条目）：WebPlayer 只带 id（字段是纯数值 → 自动
    /// Send+Sync，满足 PlayerHandle 的界；Rc 经此现取，wasm 单线程下成立）。
    static SEATS: RefCell<HashMap<u32, SeatEntry>> = RefCell::new(HashMap::new());
    static SEAT_NEXT: Cell<u32> = Cell::new(1);
}

/// A' 发号并登记（`WasmSession::new_agent` 调用；id 即 `agent_id()`/`agent_bind`
/// 的线上形态——u32 的十进制串）。
pub(crate) fn front_register(core: &SharedCore) -> String {
    let id = FRONT_NEXT.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    });
    let key = id.to_string();
    FRONT.with(|m| m.borrow_mut().insert(key.clone(), Rc::downgrade(core)));
    key
}

/// 取登记的 A'（配对目标；条目归前端，这里只借句柄）。
fn front_core(key: &str) -> Option<SharedCore> {
    FRONT.with(|m| m.borrow().get(key).and_then(|w| w.upgrade()))
}

/// B 席的无头会话句柄：注册表键 + 现取核/宿主。字段纯数值 → 自动 Send+Sync。
#[derive(Clone)]
struct WebPlayer {
    id: u32,
}

impl WebPlayer {
    /// 登记一席（构造即入表；随 [`Run`] 记账，`retire` 时摘除）。
    fn register(core: &SharedCore, hook: Arc<HookHost>) -> Self {
        let id = SEAT_NEXT.with(|n| {
            let v = n.get();
            n.set(v + 1);
            v
        });
        SEATS.with(|m| {
            m.borrow_mut().insert(
                id,
                SeatEntry { core: Rc::downgrade(core), hook },
            );
        });
        Self { id }
    }

    fn core(&self) -> Option<SharedCore> {
        SEATS.with(|m| m.borrow().get(&self.id).and_then(|e| e.core.upgrade()))
    }

    fn hook(&self) -> Option<Arc<HookHost>> {
        SEATS.with(|m| m.borrow().get(&self.id).map(|e| e.hook.clone()))
    }

    /// 摘除（终局/停止的拆除步骤；弱引用本就随强引用消失而失效，这里显式清表）。
    fn retire(&self) {
        SEATS.with(|m| m.borrow_mut().remove(&self.id));
    }
}

impl PlayerHandle for WebPlayer {
    fn cmd(&self, cmd: UiCommand) {
        let Some(core) = self.core() else { return };
        queue_event_to(&core, Event::Ui(cmd));
        self.pump();
    }

    fn snapshot(&self) -> serde_json::Value {
        // 锁坏/席已拆 → Null：diff/合成器全字段取不到 → 不产事件、不出文件，
        // 与 NativePlayer 的兜底同一口径，绝不 panic。
        let Some(core) = self.core() else { return serde_json::Value::Null };
        let text = core.borrow().session.snapshot();
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    }

    fn pump(&self) {
        if let (Some(core), Some(hook)) = (self.core(), self.hook()) {
            drain_agent(&core, &hook);
        }
    }
}

/// B 席的宿主感知泵一拍：Emit/Notice/Nav/SetStorage 四类 Effect 过 HookHost
/// （watch 推送=事件物化源头、协商 ack 物化、B 不导航、身份键不落盘——与桌面
/// `bridge::pump(core, host)` 的宿主中介逐条同形），其余效果落回 bridge 的
/// wasm 执行器。**B 的 drain 必须走这里**：走全局执行器的话 B 的 emit 会戳到
/// 人的 React、userId 会被写进人的存储。
fn drain_agent(core: &SharedCore, hook: &HookHost) {
    loop {
        let ev = core.borrow_mut().queue.pop_front();
        let Some(ev) = ev else { break };
        let ctx = goptop_net::session::ReduceCtx {
            now_ms: now_ms(),
            rand: [rand_u32(), rand_u32(), rand_u32(), rand_u32()],
        };
        let effects = goptop_net::session::reduce(&mut core.borrow_mut().session, ev, &ctx);
        for e in effects {
            match e {
                goptop_net::session::Effect::Emit => {
                    let snap = core.borrow().session.snapshot();
                    hook.emit(&snap);
                }
                goptop_net::session::Effect::Notice(text, ms) => hook.notice(text.as_deref(), ms),
                goptop_net::session::Effect::Nav(_) => {
                    // 恒吞：B 是无头席（Hub 场景下不该拽动页面路由）。
                }
                goptop_net::session::Effect::SetStorage { key, value } => {
                    // 过宿主：HookHost 拦下 userId 写（B 的临时身份绝不落盘），
                    // 其余键透传全局（与桌面 HookHost→TauriHost 同形）。
                    hook.storage_set(&key, value.as_deref());
                }
                other => bridge::run_effect(core, other),
            }
        }
    }
}

/// B 的常驻泵：50ms 一拍，随 Run 拆除（`pump_stop`）退出。决策循环每拍只泵一次
/// 之外，对手的数据面消息全靠它喂进状态机。
fn start_agent_pump(run: &Arc<Run>, player: &WebPlayer, hook: Arc<HookHost>) {
    let Some(core) = player.core() else { return };
    let stop = run.pump_stop.clone();
    spawn_local(async move {
        loop {
            sleep_ms(50).await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            drain_agent(&core, &hook);
        }
    });
}

/// setTimeout 的 Promise 化（spawn_local 微任务里的一拍；rtc 轮询同款手法）。
async fn sleep_ms(ms: u64) {
    let p = js_sys::Promise::new(&mut |resolve, _reject| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

/// B 席配置（会话构造参数；server_mode 恒 false）。
struct BSeatCfg {
    name: String,
    share_origin: String,
    kind: String,
    size: u16,
}

/// 页面同源（B 的链接基座）：A'/B 是**同页**两个 Core（各自 BC 实例、同名
/// `goptop-game-{gid}` 互相可达），链接同源即可被前端携链 Boot。
fn page_origin() -> String {
    window().location().origin().unwrap_or_default()
}

/// 建 B 席（crate 内 fn；镜像 goptop-transport-native/src/session.rs:47-86 的
/// 装配序）：user_id/avatar/stun **全部经 hook.storage_get**（HookHost 的临时 id
/// 与 `stun="[]"` 才能生效），server_mode 恒 false；Boot 入队后首 drain。
fn agent_session_new(cfg: &BSeatCfg, hook: Arc<HookHost>, href: &str) -> WebPlayer {
    console_error_panic_hook::set_once();
    let user_id = hook.storage_get("goptop:userId").unwrap_or_else(|| gen_user_id(now_ms(), rand_u32()));
    let peer_id = fresh_peer_id();
    let avatar = hook.storage_get("goptop:avatar");
    let stun = load_stun_urls_from(|k| hook.storage_get(k));
    let mut session = Session::new(user_id, peer_id, cfg.name.clone(), avatar, false, stun, cfg.share_origin.clone());
    session.kind = cfg.kind.clone();
    session.size = cfg.size;
    session.engine = goptop_core::game::GameState::new(goptop_net::session::make_engine_kind(&session.kind, session.size));
    let core = crate::new_core(session, None, true);
    let player = WebPlayer::register(&core, hook.clone());
    queue_event_to(&core, Event::Boot { href: href.to_string() });
    crate::io::start_presence(&core);
    // server_mode 恒 false → 不连服务器 WS（与桌面 B 席同口径）。
    drain_agent(&core, &hook);
    player
}

/* ---------------- 运行表与拦截面 ---------------- */

/// 状态字面量（契约值域，与桌面逐字）。
const ST_PAIRING: &str = "pairing";
const ST_THINKING: &str = "thinking";
const ST_WAITING: &str = "waiting";
const ST_DONE: &str = "done";
const ST_ERROR: &str = "error";

/// 一次运行的可变账本（桌面 RunInner 的镜像）。
struct RunInner {
    state: &'static str,
    detail: Option<String>,
    /// 循环收尾后的账面统计（stats_final 前不回，避免半截数字）。
    llm_calls: u64,
    tokens_in: u64,
    tokens_out: u64,
    compactions: u32,
    stats_final: bool,
    /// B 席（agent_stop 的认输落点；配对完成前为 None）。
    player: Option<WebPlayer>,
    /// B 的暂存区（agent_status 的 stagedMove 读数——与循环/工具面同一份）。
    staging: Option<Arc<Staging>>,
    /// B 的执色（ticker 的 thinking/waiting 判定：轮到 B = thinking）。
    agent_color: Option<String>,
    /// A' 的注册表键（= agent_bind 的登记值；配对任务取走绑定槽后记账——
    /// 绑定槽已消费，豁免靠 guard 的 exempt 续到局终）。
    #[allow(dead_code)]
    front_key: Option<String>,
}

/// 一次 Agent 对局的运行时记录。任务持有它的 Arc，导出面经 HUB 查它。
struct Run {
    id: u32,
    /// 用户中止标志（LoopDeps.stop；循环每轮 LLM 调用前检查——点了停止就不再
    /// 花钱）。协作式取消的总闸（桌面为 Notify 硬取消，wasm 无 select，见模块注）。
    abort: Arc<AtomicBool>,
    /// B 泵的停机标志（拆除时置位，spawn_local 循环见之即退）。
    pump_stop: Arc<AtomicBool>,
    /// 工具日志环（≤200；agent_events 的数据源）。
    events: Arc<EventRing>,
    /// 实时 LLM 调用计数（LogHttp 每真实请求 +1；终态后被账面值取代）。
    llm_calls: Arc<AtomicU64>,
    inner: Mutex<RunInner>,
}

impl Run {
    fn new(id: u32) -> Self {
        Self {
            id,
            abort: Arc::new(AtomicBool::new(false)),
            pump_stop: Arc::new(AtomicBool::new(false)),
            events: Arc::new(EventRing::default()),
            llm_calls: Arc::new(AtomicU64::new(0)),
            inner: Mutex::new(RunInner {
                state: ST_PAIRING,
                detail: None,
                llm_calls: 0,
                tokens_in: 0,
                tokens_out: 0,
                compactions: 0,
                stats_final: false,
                player: None,
                staging: None,
                agent_color: None,
                front_key: None,
            }),
        }
    }

    fn is_live(&self) -> bool {
        !matches!(self.state(), ST_DONE | ST_ERROR)
    }

    fn state(&self) -> &'static str {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state
    }

    fn set_front_key(&self, key: &str) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).front_key = Some(key.to_string());
    }

    fn set_detail(&self, detail: Option<String>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).detail = detail;
    }

    /// 运行中状态切换（thinking/waiting/pairing 互转）。**不覆盖终态**：ticker
    /// 与循环收尾存在竞窗，终态一旦落账就不许被运行中状态盖掉。
    fn set_running(&self, state: &'static str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(g.state, ST_DONE | ST_ERROR) {
            g.state = state;
        }
    }

    /// 终态落账（done/error，一锤定音）。
    fn set_terminal(&self, state: &'static str, detail: Option<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.state = state;
        g.detail = detail;
    }
}

thread_local! {
    /// 运行表（单局互斥由 `any_live` 保证；RefCell——wasm 单线程）。
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
    /// agent_bind 登记的 A' 键（配对目标；配对任务取走后，豁免续记在 guard 的
    /// exempt 与存活运行的 front_key 上）。
    static CMD_GUARD: RefCell<Option<GuardToken>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Hub {
    runs: HashMap<u32, Arc<Run>>,
    next_run: u32,
    bound: Option<String>,
}

/// 拦截面豁免令牌：持豁免 A' 的弱引用（契约 §2.2b 的 CMD_GUARD 槽）。
struct GuardToken {
    exempt: Option<Weak<RefCell<crate::Core>>>,
}

fn hub_insert(run: Arc<Run>) {
    HUB.with(|h| h.borrow_mut().runs.insert(run.id, run));
}

fn hub_run(id: u32) -> Option<Arc<Run>> {
    HUB.with(|h| h.borrow().runs.get(&id).cloned())
}

fn hub_remove(id: u32) {
    HUB.with(|h| h.borrow_mut().runs.remove(&id));
}

fn hub_next_id() -> u32 {
    HUB.with(|h| {
        let mut b = h.borrow_mut();
        b.next_run += 1;
        b.next_run
    })
}

/// 是否有存活的 Agent 局（拦截面的总闸）：pairing/thinking/waiting 都算存活；
/// done/error 的历史运行不拦路——局终了就不该再挡人开新局。
fn any_live() -> bool {
    HUB.with(|h| h.borrow().runs.values().any(|r| r.is_live()))
}

/// 拦截面豁免的 A'（两段来源，与桌面 exempt_session 同口径）：guard 记账的
/// exempt（配对任务取走绑定槽后）；绑定槽还满着时（bind → 取走的窗口）登记值
/// 指向的会话。
fn exempt_core() -> Option<SharedCore> {
    if let Some(c) = CMD_GUARD
        .with(|g| g.borrow().as_ref().and_then(|t| t.exempt.clone()))
        .and_then(|w| w.upgrade())
    {
        return Some(c);
    }
    let bound = HUB.with(|h| h.borrow().bound.clone());
    if let Some(c) = bound.as_deref().and_then(front_core) {
        return Some(c);
    }
    // guard 的弱引用已失效（A' 被 dispose）时回退存活运行的 front_key——与桌面
    // exempt_session 的「绑定槽 → 存活运行 front_id」两段口径对齐。
    HUB.with(|h| {
        h.borrow()
            .runs
            .values()
            .filter(|r| r.is_live())
            .find_map(|r| {
                let k = r.inner.lock().unwrap_or_else(|e| e.into_inner()).front_key.clone();
                k.and_then(|k| front_core(&k))
            })
    })
}

/// 拦截面判定（`WasmSession::cmd` 入队前调）：Agent 局存活（guard 在位）期间，
/// 非豁免会话的开局类命令一律拒绝，回提示条且**不入队**——与桌面
/// src-tauri/src/session.rs 的 session_cmd 拦截逐字同语义同文案。
pub(crate) fn intercept_cmd(me: &SharedCore, cmd: &UiCommand) -> bool {
    if !is_opening_cmd(cmd) {
        return false;
    }
    let armed = CMD_GUARD.with(|g| g.borrow().is_some());
    if !armed {
        return false;
    }
    if exempt_core().is_some_and(|c| Rc::ptr_eq(&c, me)) {
        return false;
    }
    bridge::call_hook("goptopNotice", &serde_json::json!({ "text": "Agent 对局进行中", "ms": 2400 }));
    true
}

/// 开局类命令（桌面 is_opening_cmd 的判定表逐字）：这些命令会把一个**非 Agent**
/// 的对局拉起来，在 Agent 局存活期间一律拒绝；落子/聊天/协商等对局内命令不拦。
fn is_opening_cmd(cmd: &UiCommand) -> bool {
    matches!(
        cmd,
        UiCommand::CreateInvite
            | UiCommand::AcceptInvite { .. }
            | UiCommand::AcceptReceipt(_)
            | UiCommand::ServerChallenge(_)
            | UiCommand::AcceptChallenge
    )
}

/// 布防/收防：guard 在位 ⟺ 有存活 Agent 局（agent_start 布、终态/停止收）。
fn guard_arm(exempt: Option<SharedCore>) {
    CMD_GUARD.with(|g| {
        *g.borrow_mut() =
            Some(GuardToken { exempt: exempt.as_ref().map(Rc::downgrade) });
    });
}

fn guard_set_exempt(core: &SharedCore) {
    CMD_GUARD.with(|g| {
        if let Some(t) = g.borrow_mut().as_mut() {
            t.exempt = Some(Rc::downgrade(core));
        }
    });
}

fn guard_disarm() {
    CMD_GUARD.with(|g| *g.borrow_mut() = None);
}

/// 终局/停止的公共拆除：停 B 泵、摘 B 席、撤拦截面。done/error 的运行**留在表
/// 里**（桌面同款：status 可回看，any_live 已放行下一局）。
fn finish_task(run: &Run) {
    run.pump_stop.store(true, Ordering::Relaxed);
    let player = run.inner.lock().unwrap_or_else(|e| e.into_inner()).player.take();
    if let Some(p) = player {
        p.retire();
    }
    guard_disarm();
}

/* ---------------- 工具日志环与三处包装层（桌面同形镜像） ---------------- */

/// 工具日志环（≤200，`agent_events` 的增量拉取按 `total` 游标推进）。
#[derive(Default)]
struct EventRing {
    inner: Mutex<RingInner>,
}

#[derive(Default)]
struct RingInner {
    items: VecDeque<serde_json::Value>,
    total: u64,
}

impl EventRing {
    fn push(&self, tool: &str, ok: bool, ms: u64, summary: String) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.items.len() >= 200 {
            g.items.pop_front();
        }
        g.items.push_back(serde_json::json!({
            "ts": now_ms(), "tool": tool, "ok": ok, "ms": ms, "summary": summary,
        }));
        g.total += 1;
    }

    /// 回 `(next, items)`：next=总条数（下一轮的 since），items=下标 ≥ since 的事件。
    fn snapshot(&self, since: u64) -> (u64, Vec<serde_json::Value>) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let first = g.total - g.items.len() as u64;
        let items = g
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| first + *i as u64 >= since)
            .map(|(_, it)| it.clone())
            .collect();
        (g.total, items)
    }
}

/// 玩家命令包装：B 席的每个 UiCommand 记一条日志（摘要口径与桌面逐字同文案）。
struct LogPlayer {
    inner: WebPlayer,
    ring: Arc<EventRing>,
}

impl PlayerHandle for LogPlayer {
    fn cmd(&self, cmd: UiCommand) {
        let t0 = now_ms();
        self.inner.cmd(cmd.clone());
        let (kind, summary) = action_summary(&cmd);
        self.ring.push(&format!("submit:{kind}"), true, now_ms().saturating_sub(t0), summary);
    }

    fn snapshot(&self) -> serde_json::Value {
        self.inner.snapshot()
    }

    fn pump(&self) {
        self.inner.pump();
    }
}

/// UiCommand → 日志的 (工具名, 摘要)。文本一律截断——日志是给人看的进度条。
fn action_summary(cmd: &UiCommand) -> (String, String) {
    fn clip(s: &str) -> String {
        s.chars().take(60).collect()
    }
    match cmd {
        UiCommand::Place { x, y } => ("move".into(), format!("落子 ({x},{y})")),
        UiCommand::Pass => ("pass".into(), "停一手".into()),
        UiCommand::Resign => ("resign".into(), "认输".into()),
        UiCommand::SendChat(t) => ("chat".into(), format!("发送消息：{}", clip(t))),
        UiCommand::RequestUndo => ("request".into(), "请求悔棋".into()),
        UiCommand::RequestReset => ("request".into(), "请求重开".into()),
        UiCommand::RequestSwap => ("request".into(), "请求换棋".into()),
        UiCommand::ConfirmApprove => ("confirm".into(), "同意对方请求".into()),
        UiCommand::ConfirmDecline => ("confirm".into(), "拒绝对方请求".into()),
        UiCommand::ConfirmScore => ("score".into(), "确认计分".into()),
        UiCommand::ToggleDead { x, y } => ("dead".into(), format!("标记死子 ({x},{y})")),
        _ => ("other".into(), "其他会话命令".into()),
    }
}

/// HTTP 通道包装：每次真实 LLM 请求记一条日志并给运行表 +1 实时调用计数。
struct LogHttp {
    inner: WebHttpChannel,
    ring: Arc<EventRing>,
    calls: Arc<AtomicU64>,
}

// ?Send 界见 goptop-agent llm/mod.rs 的 trait 注（wasm 全部 spawn_local）。
#[async_trait::async_trait(?Send)]
impl HttpChannel for LogHttp {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: String,
    ) -> Result<(u16, String), String> {
        let t0 = now_ms();
        let result = self.inner.post_json(url, headers, body).await;
        let n = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let ms = now_ms().saturating_sub(t0);
        match &result {
            Ok((status, text)) => self.ring.push(
                "llm",
                *status < 400,
                ms,
                format!("第 {n} 次模型调用：HTTP {status}，{} 字节回复", text.len()),
            ),
            Err(e) => self.ring.push("llm", false, ms, format!("第 {n} 次模型调用失败：{e}")),
        }
        result
    }
}

/// 记忆存储包装：/memory 的写删记日志（读写不记——读是模型的本分，刷屏无益）。
struct LogStore {
    inner: WebStore,
    ring: Arc<EventRing>,
}

impl VfsStore for LogStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        self.inner.read(ns, path)
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        let r = self.inner.write(ns, path, content);
        self.ring.push(
            "memory",
            r.is_ok(),
            0,
            format!("记忆写入 /memory/{path}（{} 字节）", content.len()),
        );
        r
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        let r = self.inner.delete(ns, path);
        self.ring.push("memory", r.as_ref().is_ok_and(|&d| d), 0, format!("记忆删除 /memory/{path}"));
        r
    }

    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String> {
        self.inner.list(ns, prefix)
    }

    fn usage(&self, ns: &str) -> Result<u64, String> {
        self.inner.usage(ns)
    }
}

/* ---------------- 配置装载（store 链 → llm 层类型；桌面 parse_llm_cfg 的镜像） ---------------- */

/// LLM 连接配置（JSON：protocol/baseUrl/model/maxOutputTokens/replyLang/enableSubagent）。
const KEY_LLM_CONFIG: &str = "goptop:llm-config";
/// API key（**明文**，风险如实注明，计划 R1；与配置分开存）。
const KEY_LLM_KEY: &str = "goptop:llm-key";
/// 上下文上限 tokens（clamp [8k, 1M]，缺省 DEFAULT_CTX_LIMIT）。
const KEY_CTX_LIMIT: &str = "goptop:agent-ctx-limit";

/// 上下文上限的合法区间（与桌面同值）。
const CTX_MIN: u32 = 8_000;
const CTX_MAX: u32 = 1_000_000;

/// store 里 goptop:llm-config 的 JSON 形态（与前端设置卡同键同形）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmCfgJson {
    protocol: String,
    base_url: String,
    model: String,
    max_output_tokens: Option<u32>,
    reply_lang: Option<String>,
    enable_subagent: Option<bool>,
}

/// 协议拼写归一（桌面 parse_protocol 逐字镜像）：crate serde 值域是 snake_case，
/// 前端写 kebab 风格——分隔符与大小写一律抹平后比对。
fn parse_protocol(raw: &str) -> Result<Protocol, String> {
    let t: String = raw.trim().to_lowercase().chars().filter(|c| *c != '-' && *c != '_').collect();
    match t.as_str() {
        "anthropic" => Ok(Protocol::Anthropic),
        "openairesponses" => Ok(Protocol::OpenAiResponses),
        "openaichat" => Ok(Protocol::OpenAiChat),
        _ => Err(format!(
            "LLM 配置的 protocol 无法识别：{raw:?}（可选 anthropic / openai-responses / openai-chat）"
        )),
    }
}

/// 读 store 链聚齐 LLM 配置：`(LlmConfig, api_key, ctx_limit, subagent_enabled)`。
/// 取数经宿主钩子（web=localStorage；键名与桌面同一份 store.json 链）。
fn load_llm_cfg(get: &dyn Fn(&str) -> Option<String>, ui_lang: &str) -> Result<(LlmConfig, String, u32, bool), String> {
    let raw = get(KEY_LLM_CONFIG)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("尚未配置 LLM：请在「Agent 对战」设置卡选择协议、填写端点与模型")?;
    let j: LlmCfgJson =
        serde_json::from_str(&raw).map_err(|e| format!("LLM 配置解析失败：{e}（{raw}）"))?;
    let key = get(KEY_LLM_KEY)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("尚未配置 API Key：请在「Agent 对战」设置卡填写后重试")?;
    let ctx_limit = get(KEY_CTX_LIMIT)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_CTX_LIMIT)
        .clamp(CTX_MIN, CTX_MAX);
    let cfg = LlmConfig {
        protocol: parse_protocol(&j.protocol)?,
        base_url: j.base_url.trim().trim_end_matches('/').to_string(),
        model: j.model.trim().to_string(),
        max_output_tokens: j.max_output_tokens.unwrap_or(1024).max(64),
        reply_lang: Some(
            j.reply_lang
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ui_lang.to_string()),
        ),
    };
    if cfg.base_url.is_empty() || cfg.model.is_empty() {
        return Err("LLM 配置不完整：端点与模型都不能为空".into());
    }
    Ok((cfg, key, ctx_limit, j.enable_subagent.unwrap_or(false)))
}

/// 暂存着法文本 → `(x, y)`（agent_status 的 stagedMove；pass/残缺 → None）。
fn parse_staged_move(text: &str) -> Option<(u16, u16)> {
    let t = text.trim();
    if t.is_empty() || t == "pass" {
        return None;
    }
    let (x, y) = t.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/* ---------------- 内置模式任务：配对 → 循环（spawn_local 驱动） ---------------- */

/// 配对成功后的材料（桌面 Seats 的镜像；A' 不进 Seats——它的条目与泵归前端）。
struct Seats {
    agent: WebPlayer,
    agent_watch: EmitWatch,
    agent_hook: Arc<HookHost>,
    front_key: String,
}

/// 内置模式的整局装配。全程 spawn_local 微任务；取消是协作式的（每拍 `is_live` /
/// 每轮 LLM 调用前的 stop 检查收口，见模块注）。
async fn run_builtin_task(run: Arc<Run>, cfg: StartCfg, seat: SeatColor) {
    let ui_lang = cfg.ui_lang.clone().unwrap_or_else(|| "简体中文".into());
    let agent_name = cfg.agent_name.clone().unwrap_or_else(|| "Agent".into());

    // —— MCP 不是本端的能力（条件编译：MCP 桌面专属，主计划拍板）——人话拒绝。
    if cfg.driver == Driver::Mcp {
        run.set_terminal(ST_ERROR, Some("MCP 对战仅桌面版可用".into()));
        finish_task(&run);
        return;
    }

    // —— 配置先于配对（与桌面同序）：LLM 没配好就不该结会话对。
    let hook_host = goptop_agent::wasm::WebHost::new();
    let (llm_cfg, api_key, ctx_limit, subagent_enabled) = match load_llm_cfg(&|k: &str| PlatformHost::storage_get(&hook_host, k), &ui_lang) {
        Ok(x) => x,
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(e));
            finish_task(&run);
            return;
        }
    };
    let memory: Arc<dyn VfsStore> = match WebStore::from_hook() {
        Ok(s) => Arc::new(LogStore { inner: s, ring: run.events.clone() }),
        Err(e) => {
            run.set_terminal(ST_ERROR, Some(e));
            finish_task(&run);
            return;
        }
    };
    let http = Arc::new(LogHttp {
        inner: WebHttpChannel::new(),
        ring: run.events.clone(),
        calls: run.llm_calls.clone(),
    });
    let llm = Arc::new(match llm_cfg.protocol {
        Protocol::Anthropic => LlmClient::Anthropic { cfg: llm_cfg.clone(), api_key, http: http.clone() },
        Protocol::OpenAiResponses => LlmClient::OpenAiResponses { cfg: llm_cfg.clone(), api_key, http: http.clone() },
        Protocol::OpenAiChat => LlmClient::OpenAiChat { cfg: llm_cfg.clone(), api_key, http },
    });

    // —— 配对（B 席的宿主基座进 HookHost：临时 userId / stun 空 / emit 推 watch；
    //      调用方不预包，wrap 的临时 ID 才生效——与桌面 pair_seats 同一纪律）。
    let (agent_hook, agent_watch) = HookHost::wrap(Arc::new(goptop_agent::wasm::WebHost::new()));
    let paired = pair_seats(&run, &cfg, seat, agent_hook.clone(), agent_watch).await;
    let seats = match paired {
        Ok(s) => s,
        Err(e) => {
            // 取消竞窗：停止与失败同时到时，停止优先（桌面同款，不报误导性错误）。
            if run.state() == ST_DONE {
                finish_task(&run);
                return;
            }
            run.set_terminal(ST_ERROR, Some(format!("配对失败：{e}")));
            finish_task(&run);
            return;
        }
    };
    let Seats { agent, agent_watch, agent_hook, front_key } = seats;
    let agent_color = seat.opponent().as_str().to_string();
    {
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.player = Some(agent.clone());
        g.agent_color = Some(agent_color.clone());
    }
    run.set_detail(Some(format!("已配对（A' 会话 id={front_key}），Agent 开始思考")));

    // —— 事件物化（基线在 spawn 前本任务同步取定，晚起的泵也吞不掉基线后的事件）。
    let queue = Arc::new(EventQueue::new());
    agent_hook.bind_events(&queue);
    let baseline = event_baseline(&agent_watch);
    spawn_local(run_event_pump(EmitWatch::new(agent_watch.clone_rx()), queue.clone(), baseline));

    // —— 工具面：主循环与子代理各持一份 ToolCtx，Arc 底座共享同一局。
    let staging = Arc::new(Staging::new());
    run.inner.lock().unwrap_or_else(|e| e.into_inner()).staging = Some(staging.clone());
    let make_ctx = || ToolCtx {
        player: Arc::new(LogPlayer { inner: agent.clone(), ring: run.events.clone() }),
        watch: EmitWatch::new(agent_watch.clone_rx()),
        events: queue.clone(),
        staging: staging.clone(),
        memory: memory.clone(),
        memory_ns: Driver::Builtin.memory_ns(),
        driver: Driver::Builtin,
        subagent_enabled,
        subagent: None,
    };
    let sub_ctx = make_ctx();
    let mut ctx = make_ctx();
    if subagent_enabled {
        ctx.subagent = Some(Arc::new(SubagentLoop::new(llm.clone(), sub_ctx)));
    }

    let system = build_system_prompt(&PromptCfg {
        agent_name,
        my_color: agent_color.clone(),
        kind: cfg.kind.clone(),
        size: cfg.size,
        reply_lang: llm_cfg.reply_lang.clone().unwrap_or_else(|| ui_lang.clone()),
        driver: Driver::Builtin,
        subagent_enabled,
    });
    let deps = LoopDeps {
        llm,
        llm_cfg,
        cfg: LoopConfig { max_llm_calls: DEFAULT_MAX_LLM_CALLS, ctx_limit },
        tools: ctx,
        stop: run.abort.clone(),
        system,
    };

    // —— 状态 ticker：轮到 B=thinking、轮到对手=waiting（循环本体是一口阻塞调用，
    //      内部相位它不外报；用快照的 toMove 反推是最诚实的近似——桌面同款）。
    let ticker_run = run.clone();
    let ticker_player = agent.clone();
    let ticker_color = agent_color;
    spawn_local(async move {
        loop {
            sleep_ms(500).await;
            if !ticker_run.is_live() {
                break;
            }
            let to_move = ticker_player
                .snapshot()
                .get("toMove")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if to_move.as_deref() == Some(ticker_color.as_str()) {
                ticker_run.set_running(ST_THINKING);
            } else {
                ticker_run.set_running(ST_WAITING);
            }
        }
    });

    // —— 循环（协作式取消：agent_stop 置 abort，循环在每轮 LLM 调用前自查）；
    //      收尾把 LoopStats 记账（与桌面 run_builtin_task 的收尾逐字同口径）。
    let outcome = agent_loop::run(deps).await;
    {
        let mut g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stats_final = true;
        g.llm_calls = u64::from(outcome.stats.llm_calls);
        g.tokens_in = outcome.stats.input_tokens;
        g.tokens_out = outcome.stats.output_tokens;
        g.compactions = outcome.stats.compactions;
        let detail = match &outcome.stop {
            LoopStop::GameOver => "对局结束".to_string(),
            LoopStop::Resigned => "Agent 已认输".to_string(),
            LoopStop::Stopped => "已停止".to_string(),
            LoopStop::BudgetExhausted => "思考预算用尽（LLM 调用达上限），对局中止".to_string(),
            LoopStop::Fatal(m) => {
                g.state = ST_ERROR;
                format!("Agent 异常退出：{m}")
            }
        };
        if g.state != ST_ERROR {
            g.state = ST_DONE;
        }
        g.detail = Some(detail);
    }
    finish_task(&run);
}

/// 结对两席（桌面 pair_seats 的镜像；取消经 run 状态传播——终态即 Err("已停止")）。
async fn pair_seats(
    run: &Arc<Run>,
    cfg: &StartCfg,
    seat: SeatColor,
    agent_hook: Arc<HookHost>,
    agent_watch: EmitWatch,
) -> Result<Seats, String> {
    let origin = page_origin();
    let agent_cfg = BSeatCfg {
        name: cfg.agent_name.clone().unwrap_or_else(|| "Agent".into()),
        share_origin: origin.clone(),
        kind: cfg.kind.clone(),
        size: cfg.size,
    };

    // 方向决定先后（桌面同款互锁）：Black 先等登记（bind 先于 agent_start），
    // White 先出链（bind 后于 agent_start）——两段 wait_bound 不能对调。
    let (front_key, agent) = match seat {
        SeatColor::Black => {
            let front_key = wait_bound(run, BIND_WAIT_SECS)
                .await
                .ok_or_else(|| "等待 agent_bind 超时：请先在前端登记 A' 会话".to_string())?;
            let front = front_core(&front_key)
                .ok_or_else(|| format!("登记的 A' 会话（id={front_key}）不在注册表里"))?;
            guard_set_exempt(&front);
            // 邀请方是 A'：agent_bind 已代发（空置时）；这里兜底补发并等 rtc 编入链接。
            if invite_link_of(&front).is_none() {
                cmd_core(&front, UiCommand::CreateInvite);
            }
            run.set_detail(Some("等待 A' 的邀请链接就绪…".into()));
            wait_invite(run, &front).await?;
            let link =
                invite_link_of(&front).ok_or_else(|| "A' 的邀请链接未就绪（缺 rtc=）".to_string())?;
            let agent = agent_session_new(&agent_cfg, agent_hook.clone(), &link);
            start_agent_pump(run, &agent, agent_hook.clone());
            (front_key, agent)
        }
        SeatColor::White => {
            // B 先建局邀请（基座 /p2p），链接经 detail 送前端，前端携链 session_new
            // 创建 A'（Boot 路径）后 agent_bind 接驳；A' 的回执经同页 BC 自动到 B。
            let base = format!("{}/p2p", origin.trim_end_matches('/'));
            let agent = agent_session_new(&agent_cfg, agent_hook.clone(), &base);
            start_agent_pump(run, &agent, agent_hook.clone());
            run.set_detail(Some("Agent 席生成邀请中…".into()));
            agent.cmd(UiCommand::CreateInvite);
            let agent_core =
                agent.core().ok_or_else(|| "B 席未就绪（注册表条目失效）".to_string())?;
            wait_invite(run, &agent_core).await?;
            let link =
                invite_link_of(&agent_core).ok_or_else(|| "B 的邀请链接未就绪（缺 rtc=）".to_string())?;
            run.set_detail(Some(format!(
                "B 已就绪，请以此邀请链接创建 A'（携链 Boot 入局）：{link}"
            )));
            let front_key = wait_bound(run, BIND_WAIT_SECS)
                .await
                .ok_or_else(|| "等待前端携链创建 A' 并 agent_bind 超时（90 秒）".to_string())?;
            guard_set_exempt_opt(front_core(&front_key));
            (front_key, agent)
        }
    };

    // 早记 B 席（agent_stop 的认输落点、ticker 的判定都要它——桌面 record_b_seat 同款）。
    run.inner.lock().unwrap_or_else(|e| e.into_inner()).player = Some(agent.clone());
    let front = front_core(&front_key)
        .ok_or_else(|| format!("登记的 A' 会话（id={front_key}）不在注册表里"))?;
    wait_playing(run, &front, &agent).await?;
    Ok(Seats { agent, agent_watch, agent_hook, front_key })
}

/// 等 agent_bind 的登记值（轮询 HUB 的绑定槽；终态即撤）。取走即在本运行的
/// `front_key` 记账——绑定槽消费后，拦截面豁免靠 guard 的 exempt 续到局终。
async fn wait_bound(run: &Arc<Run>, secs: u64) -> Option<String> {
    let deadline = now_ms() + secs * 1000;
    loop {
        let key = HUB.with(|h| h.borrow_mut().bound.take());
        if let Some(key) = key {
            run.set_front_key(&key);
            return Some(key);
        }
        if !run.is_live() || now_ms() >= deadline {
            return None;
        }
        sleep_ms(200).await;
    }
}

fn guard_set_exempt_opt(core: Option<SharedCore>) {
    if let Some(c) = core {
        guard_set_exempt(&c);
    }
}

/// 直接对一个核下命令（A' 由前端所有，Hub 只借句柄代发/泵）。
fn cmd_core(core: &SharedCore, cmd: UiCommand) {
    queue_event_to(core, Event::Ui(cmd));
    drain_core(core);
}

/// 一条核的快照 Value（坏文本按 Null——与 WebPlayer 同一口径）。
fn snap_of(core: &SharedCore) -> serde_json::Value {
    let text = core.borrow().session.snapshot();
    serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
}

/// 邀请链接是否就绪（`rtc=` 参数在场——同源链接一开始就有，不含 rtc 不算就绪）。
fn invite_link_of(core: &SharedCore) -> Option<String> {
    snap_of(core)["inviteUrl"]
        .as_str()
        .filter(|u| u.contains("rtc="))
        .map(str::to_string)
}

/// 等一条会话的 inviteUrl 编入 rtc（≤40s；pair.rs/桌面同口径同文案）。边泵边等。
async fn wait_invite(run: &Arc<Run>, core: &SharedCore) -> Result<(), String> {
    let deadline = now_ms() + 40 * 1000;
    loop {
        drain_core(core);
        if invite_link_of(core).is_some() {
            return Ok(());
        }
        if run.state() == ST_DONE {
            return Err("已停止".into());
        }
        if now_ms() >= deadline {
            return Err("40 秒内未生成含 rtc 的邀请链接（ICE gathering 未完成？）".into());
        }
        sleep_ms(50).await;
    }
}

/// 泵两席到双双进入对局态（≤40s；桌面 wait_playing 同款与同文案）。A' 走
/// [`drain_core`]（on_change 生效），B 走 WebPlayer::pump（宿主感知）。
async fn wait_playing(run: &Arc<Run>, front: &SharedCore, agent: &WebPlayer) -> Result<(), String> {
    let deadline = now_ms() + 40 * 1000;
    loop {
        drain_core(front);
        agent.pump();
        let (fs, as_) = (snap_of(front), agent.snapshot());
        if fs["phase"].as_str() == Some("playing")
            && as_["phase"].as_str() == Some("playing")
            && fs["peerConnected"] == serde_json::Value::Bool(true)
            && as_["peerConnected"] == serde_json::Value::Bool(true)
        {
            return Ok(());
        }
        if run.state() == ST_DONE {
            return Err("已停止".into());
        }
        if now_ms() >= deadline {
            // 失败带两席现场：卡在 waiting=信令没走到、playing 但未连=ICE 没通。
            return Err(format!(
                "40 秒内未双双进入对局态：front phase={} connected={} / agent phase={} connected={}",
                fs["phase"].as_str().unwrap_or_default(),
                fs["peerConnected"],
                as_["phase"].as_str().unwrap_or_default(),
                as_["peerConnected"],
            ));
        }
        sleep_ms(50).await;
    }
}

/* ---------------- agent_* 导出面（契约 §5.3.1：JSON 进出、跨边界不 throw） ---------------- */

fn ok_json(v: serde_json::Value) -> String {
    v.to_string()
}

fn err_json(e: String) -> String {
    serde_json::json!({ "ok": false, "error": e }).to_string()
}

/// `agent_start(cfgJson) -> {"ok":true,"id":N} | {"ok":false,"error"}`。
///
/// 单局互斥（any_live）与参数校验的文案与桌面逐字一致；任务在 spawn_local 里
/// 自转，本导出只登记就返回。
#[wasm_bindgen]
pub fn agent_start(cfg_json: &str) -> String {
    console_error_panic_hook::set_once();
    match agent_start_inner(cfg_json) {
        Ok(id) => ok_json(serde_json::json!({ "ok": true, "id": id })),
        Err(e) => err_json(e),
    }
}

fn agent_start_inner(cfg_json: &str) -> Result<u32, String> {
    let cfg: StartCfg =
        serde_json::from_str(cfg_json).map_err(|e| format!("cfg 解析失败: {e}"))?;
    let seat = parse_seat(&cfg.my_color)?;
    if cfg.kind != "gomoku" && cfg.kind != "go" {
        return Err(format!("kind 必须是 \"gomoku\" 或 \"go\"，得到 {:?}", cfg.kind));
    }
    if cfg.size < 5 {
        return Err(format!("size 过小（{}）", cfg.size));
    }
    if any_live() {
        // 单局互斥：进程内 BC hub 全局无局号，并行两局会互串。
        return Err("已有 Agent 对局进行中，请先停止当前对局".into());
    }
    let run = Arc::new(Run::new(hub_next_id()));
    run.set_running(ST_PAIRING);
    run.set_detail(Some("配对中…".into()));
    // 拦截面布防：bind 先行时豁免即刻生效（桌面 exempt_session 读绑定槽的同口径）。
    let bound = HUB.with(|h| h.borrow().bound.clone());
    guard_arm(bound.as_deref().and_then(front_core));
    hub_insert(run.clone());
    let run2 = run.clone();
    spawn_local(async move {
        run_builtin_task(run2, cfg, seat).await;
    });
    Ok(run.id)
}

/// `agent_stop(id) -> {"ok":true}`（异步导出回 Promise；永不 reject）。
///
/// 顺序有契约（不可换，与桌面逐字）：认输先落地（对面要看到终局有因，而不是
/// 看到断线），`RESIGN_SETTLE_MS` 定拍给数据面留发送窗口，再置 abort——配对
/// 轮询与循环的既有检查点随即收口，最后拆泵/撤防/摘表。
#[wasm_bindgen]
pub async fn agent_stop(id: u32) -> String {
    console_error_panic_hook::set_once();
    let Some(run) = hub_run(id) else { return err_json("run not found".into()) };
    // 1) 局中先认输（B 席）。锁面必须先收：锁不能跨 await。
    let resigned = {
        let g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.player.as_ref() {
            Some(p) => {
                let snap = p.snapshot();
                let live = snap.get("phase").and_then(|v| v.as_str()) == Some("playing")
                    && snap.get("winner").is_none_or(serde_json::Value::is_null);
                if live {
                    p.cmd(UiCommand::Resign);
                }
                live
            }
            None => false,
        }
    };
    if resigned {
        sleep_ms(RESIGN_SETTLE_MS).await;
    }
    // 2) 终止：abort 置位（循环每轮 LLM 调用前自查；配对轮询看终态）。
    run.abort.store(true, Ordering::Relaxed);
    run.set_terminal(ST_DONE, Some("已停止".into()));
    // 3) 拆泵/摘 B 席/撤拦截面 + 4) 摘表（此后 status/events 报 not found）。
    finish_task(&run);
    hub_remove(id);
    ok_json(serde_json::json!({ "ok": true }))
}

/// `agent_status(id)`：状态 JSON（与桌面 agent_status 逐字同形：
/// state/detail/stagedMove/llmCalls/tokensIn/tokensOut/compactions）。
/// 运行不在表里回 `{"ok":false,"error":"run not found"}`。
#[wasm_bindgen]
pub fn agent_status(id: u32) -> String {
    let Some(run) = hub_run(id) else { return err_json("run not found".into()) };
    let g = run.inner.lock().unwrap_or_else(|e| e.into_inner());
    let staged = g
        .staging
        .as_ref()
        .and_then(|s| s.peek(InFile::Move))
        .and_then(|t| parse_staged_move(&t));
    let llm_calls = if g.stats_final { g.llm_calls } else { run.llm_calls.load(Ordering::Relaxed) };
    serde_json::json!({
        "state": g.state,
        "detail": g.detail,
        "stagedMove": staged.map(|(x, y)| serde_json::json!({"x": x, "y": y})),
        "llmCalls": llm_calls,
        "tokensIn": g.tokens_in,
        "tokensOut": g.tokens_out,
        "compactions": g.compactions,
    })
    .to_string()
}

/// `agent_events(id, since)`：工具日志环（≤200）的增量拉取。`since` 用 u32
/// （wasm-bindgen 的 u64 映射 BigInt，环 ≤200 无需——契约偏差 3）。
#[wasm_bindgen]
pub fn agent_events(id: u32, since: u32) -> String {
    let Some(run) = hub_run(id) else { return err_json("run not found".into()) };
    let (next, items) = run.events.snapshot(u64::from(since));
    serde_json::json!({ "next": next, "items": items }).to_string()
}

/// `agent_bind(sessionId)`：登记人类侧 A' 会话键（配对目标 + 拦截面豁免）。
///
/// **开局邀请代发**（桌面同款）：登记时若该会话还空置（phase=home）就代发一次
/// `CreateInvite`——A' 建在 /p2p 基座上不会自发邀请，本命令是链路里唯一能替 A'
/// 按下「开启对战」的点；**仅在 home 时发**（我执白方向的 A' 是携链 Boot 的受邀席，
/// 误发会拆掉它正在进行的受理）。会话暂不在注册表也照存登记值（桌面同款不报错）。
#[wasm_bindgen]
pub fn agent_bind(session_id: &str) -> String {
    let key = session_id.trim().to_string();
    if key.is_empty() {
        return err_json("sessionId 不能为空".into());
    }
    HUB.with(|h| h.borrow_mut().bound = Some(key.clone()));
    if let Some(core) = front_core(&key) {
        if snap_of(&core).get("phase").and_then(serde_json::Value::as_str) == Some("home") {
            cmd_core(&core, UiCommand::CreateInvite);
        }
    }
    ok_json(serde_json::json!({ "ok": true }))
}

/// `agent_llm_test()`：发一次最小真请求验证配置，`"ok"` 或人话错误（异步导出，
/// 真请求最坏 60s 超时 × 3 次尝试）。**不硬编码任何端点/key**：配置全部读 store 链。
#[wasm_bindgen]
pub async fn agent_llm_test() -> String {
    console_error_panic_hook::set_once();
    let host = goptop_agent::wasm::WebHost::new();
    let (cfg, key, _ctx, _sub) = match load_llm_cfg(&|k: &str| PlatformHost::storage_get(&host, k), "简体中文") {
        Ok(x) => x,
        Err(e) => return e,
    };
    let http: Arc<dyn HttpChannel> = Arc::new(WebHttpChannel::new());
    // 协议按配置分派（与 run_builtin_task 的装配同一张表，两处永不分叉）。
    let client = match cfg.protocol {
        Protocol::Anthropic => LlmClient::Anthropic { cfg, api_key: key, http },
        Protocol::OpenAiResponses => LlmClient::OpenAiResponses { cfg, api_key: key, http },
        Protocol::OpenAiChat => LlmClient::OpenAiChat { cfg, api_key: key, http },
    };
    let req = ChatRequest {
        system: "You are a connectivity probe. Reply with the single word: ok.".into(),
        messages: vec![Msg { role: Role::User, content: vec![Block::Text { text: "ping".into() }] }],
        tools: Vec::new(),
        max_output_tokens: 16,
    };
    match client.chat(req).await {
        Ok(_) => "ok".into(),
        // 人话错误（契约）：分类文本直出——llm 层的错误文本本就面向展示。
        Err(e) => match e {
            LlmError::Fatal(m) => format!("连接失败：{m}"),
            LlmError::Transient(m) => format!("端点不可用（重试后仍失败）：{m}"),
            LlmError::ContextWindowExceeded { .. } => {
                "上下文上限配置超过模型窗口，请调小上下文上限".into()
            }
        },
    }
}
