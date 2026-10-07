//! 玩家席与事件物化 —— Agent 的 B 席怎么开、对手那边发生了什么怎么知道。
//!
//! 三个角色：
//! - [`PlayerHandle`]：对「一席玩家」的最小操作面（UiCommand 唯一命令口 + snapshot
//!   唯一读口 + 条件等待）。决策循环与工具层只认它，不认 `NativeSession`——
//!   将来 MCP 外部 Agent / 真人代打才有缝可接（计划「Agent 注册大厅」的扩展缝）。
//! - [`NativePlayer`]：无头会话的适配实现（就是 `Arc<NativeSession>` 的转述）。
//! - [`HookHost`]：装饰 `Arc<dyn Host>`。B 的会话需要一个与 A' 不同的**会话级临时
//!   随机 userId**——bc.rs:129-131 按会话 userId 过滤回声（`from==me` 丢弃），B 若
//!   与 A' 同 ID，双方都会把对方的消息当自己的回声吃掉。这个值只是去重键，无任何
//!   身份语义：不注册、不落盘、局散即弃。
//!
//! 事件物化（计划「事件物化」节，零 transport 改动）：[`EmitWatch`] 每拍快照与
//! 上一拍 diff → 同一事件流喂两个出口——**事件队列**（[`EventQueue`]，新事件待消费）
//! 与 **/game/events 全量历史文件**（append-only，seq 单调）。事件种类：history 增长
//! → move/pass（附坐标）、chatLog 增长 → chat（附全文）、confirmReq 出现/消失 →
//! request_received/request_resolved、scoring 翻真 → scoring_started、scoreResult
//! 出现 → score_result、winner 出现 → game_over。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession};
use tokio::sync::watch;

/// 对一席玩家的最小操作面。
///
/// **全部方法同步**：`NativeSession::pump` 是同步泵（内部 `tokio::spawn` 走
/// `enter_runtime` 的进程级运行时），同步面让本 trait 保持 dyn 兼容
/// （`ToolCtx` 里存 `Arc<dyn PlayerHandle>`）。阻塞式条件等待在 native 侧可接受
/// （B 的泵就是它的推进方式）；web 端阶段⑤换驱动时再按 cfg 收窄。
pub trait PlayerHandle: Send + Sync {
    /// 发一条 UiCommand（落子/停一手/聊天/协商/计分确认/认输……唯一命令口）。
    /// 命令是否生效**不由返回值表达**（内部无返回）：对局规则错误（未轮到/占点）
    /// 由状态机静默拒绝，调用方在 settle 后重读 snapshot 判定——与真人按钮同一条路。
    fn cmd(&self, cmd: UiCommand);

    /// 读当前快照（`Session::snapshot()` 的 Value 形态）。字段名 camelCase，
    /// 契约见 goptop-net 的 snapshot.rs——vfs 的动态合成器与事件 diff 只认它。
    fn snapshot(&self) -> serde_json::Value;

    /// 泵一次：把 IO 回调入队的事件消化进状态机并执行 Effect。
    /// **不泵就没有推进**：native 的 IO 回调只入队（bc.rs / ws / rtc 各自的 tokio
    /// 任务），事件要过泵才进状态机。
    fn pump(&self);

    /// 轮询到谓词成立或超时。每拍自泵一次再验谓词（先泵后验——IO 事件可能正等在
    /// 队列里）；50ms 一拍，与前端拉模式同节奏。
    ///
    /// 只泵自己这一席：对手席的推进是它自己宿主的事（桌面=前端 poll，测试=测试代码
    /// 泵两端）。谓词参数是**最新快照**（每次重读，不缓存）。
    fn wait_until(&self, pred: &mut dyn FnMut(&serde_json::Value) -> bool, timeout: Duration) -> bool;
}

/// [`PlayerHandle`] 的无头会话实现：一条 `Arc<NativeSession>` 的直述。
///
/// 持 `Arc` 而非值：pair() 要把同一会话交给调用方（A' 给前端轮询），
/// 生命周期跨层共享；`NativeSession` 的 Drop 置停机标志，drop 最后一份即全线停机。
/// `Clone`：同一席要多处持有（ctx.player 与测试 driver 各持一份，都泵同一会话）。
#[derive(Clone)]
pub struct NativePlayer {
    session: Arc<NativeSession>,
}

impl NativePlayer {
    /// 包一条已创建的无头会话。
    #[must_use]
    pub fn new(session: Arc<NativeSession>) -> Self {
        Self { session }
    }

    /// 取回底层会话句柄（AgentHub 要把 A' 的 id 报给前端、调用 `start_pump` 等场合）。
    #[must_use]
    pub fn session(&self) -> &Arc<NativeSession> {
        &self.session
    }
}

impl PlayerHandle for NativePlayer {
    fn cmd(&self, cmd: UiCommand) {
        self.session.cmd(cmd);
    }

    fn snapshot(&self) -> serde_json::Value {
        // 锁坏时 session.snapshot() 已回 "null" 字符串，这里保持同一兜底口径：
        // 坏快照 → Null，diff/合成器全字段取不到 → 不产事件、不出文件，绝不 panic。
        serde_json::from_str(&self.session.snapshot()).unwrap_or(serde_json::Value::Null)
    }

    fn pump(&self) {
        self.session.pump();
    }

    fn wait_until(&self, pred: &mut dyn FnMut(&serde_json::Value) -> bool, timeout: Duration) -> bool {
        // 先泵后验（IO 事件可能正等在队列里）、每拍重读快照（谓词永远看最新态）。
        // 阻塞式 std 睡眠在 native 侧可接受（见 trait 注）——调用点要么在同步线程，
        // 要么明知占用一个运行时线程换「像前端一样拉」的简单模型。
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.pump();
            let snap = self.snapshot();
            if pred(&snap) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// 会话快照推送流：`(seq, snapshot 文本)`，由 [`HookHost::emit`] 推送
/// （命中 bridge.rs 的 `Effect::Emit` 钩子，不改 transport/net 一行）。
///
/// **为什么 watch 而非 mpsc**：关心的是「最新」，积压的中间态没有价值——对手连下
/// 三手后，等待方只需要最后一拍（diff 物化也以最新一拍为基线，中间拍 diff 出来的
/// 事件不完整反而有害）。seq 单调递增，供消费方判断「有没有新拍」。
pub struct EmitWatch(watch::Receiver<(u64, String)>);

impl EmitWatch {
    /// 从 watch 接收端构造。
    #[must_use]
    pub fn new(rx: watch::Receiver<(u64, String)>) -> Self {
        Self(rx)
    }

    /// 克隆一个接收端（watch 的 Receiver 可克隆；`changed(&mut)` 需要独占借用，
    /// 多处消费（工具谓词等待 + wait_events）各持克隆互不干扰）。
    #[must_use]
    pub fn clone_rx(&self) -> watch::Receiver<(u64, String)> {
        self.0.clone()
    }

    /// 当前最新一拍（不等待）。
    #[must_use]
    pub fn latest(&self) -> (u64, String) {
        self.0.borrow().clone()
    }

    /// 等下一拍（返回 `(seq, snapshot 文本)`）。sender 全部 drop（局散）时回 Err。
    pub async fn changed(&mut self) -> Result<(u64, String), watch::error::RecvError> {
        self.0.changed().await?;
        Ok(self.0.borrow().clone())
    }
}

/// B 席的装饰宿主：拦截三个键、覆写 emit 与 notice，其余全部透传给内层宿主
/// （桌面=TauriHost，测试=HeadlessHost）。
///
/// 三处拦截（计划「会话对」节第 2 条）：
/// - `goptop:userId` 读 → 本局临时随机值（**不写存储**；写路径同样吞掉，
///   免得会话初始化把人的持久设备身份覆盖成临时值）；
/// - `goptop:stun` 读 → 强制 `"[]"`（免 ICE gathering 的 8s 超时兜底，
///   headless.rs:26-30 的既有手法——Agent 局要的是秒级配对，不是最优链路）；
/// - `emit` → 先推 [`EmitWatch`] 再透传（事件物化的源头）；
/// - `notice` → 协商结果的 Ack 落在提示条而非快照（`UndoAck{ok:false}` 只出
///   Notice「对方拒绝了悔棋」）——Agent 作为**请求方**时这是被拒的唯一本地载体，
///   由 [`ack_event_from_notice`] 物化成 request_resolved 事件喂事件队列
///   （`bind_events` 绑定；未绑定即纯透传）。
pub struct HookHost {
    inner: Arc<dyn Host>,
    /// 本局临时随机 userId（u- 前缀，与持久身份同形——只求 bc.rs 的回声过滤认它）。
    user_id: String,
    watch_tx: watch::Sender<(u64, String)>,
    seq: AtomicU64,
    /// 事件队列的弱引用（notice 拦截的落点；Weak——局散时队列先死也不悬挂）。
    events: std::sync::Mutex<Option<std::sync::Weak<EventQueue>>>,
}

impl HookHost {
    /// 装饰内层宿主并建 watch，一次拿全（宿主进 `NativeSession::new`，watch 给事件泵）。
    ///
    /// 临时 userId 用 `identity::gen_user_id` + transport 的 `rand4()`：与既有身份
    /// 生成同一条路，熵来源（进程级 RandomState + 时间 + 自增）已验证防同毫秒撞车
    ///（lib.rs 的 rand4 说明——两个会话同 userId 会让挑战发给自己）。
    pub fn wrap(inner: Arc<dyn Host>) -> (Arc<Self>, EmitWatch) {
        let (watch_tx, watch_rx) = tokio::sync::watch::channel((0u64, String::new()));
        // 临时 userId 与既有身份生成同一条路（gen_user_id + rand4）：u- 前缀同形，
        // 熵来源（进程级 RandomState + 自增）已验证同毫秒不撞车——两个会话同 userId
        // 会让 bc.rs 的回声过滤把对方消息当自己的回声吃掉。
        let rand = goptop_transport_native::rand4();
        let user_id = goptop_net::identity::gen_user_id(goptop_transport_native::now_ms(), rand[0]);
        let host = Arc::new(Self {
            inner,
            user_id,
            watch_tx,
            seq: AtomicU64::new(0),
            events: std::sync::Mutex::new(None),
        });
        (host, EmitWatch::new(watch_rx))
    }

    /// 绑定事件队列（notice 拦截的物化落点）。装配层（pair 的调用方）在起事件泵
    /// 前调用一次；不绑定则 notice 纯透传（对局功能不受影响，只是请求方的
    /// 协商结果不进事件流）。
    pub fn bind_events(&self, queue: &Arc<EventQueue>) {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(queue));
    }

    /// 本局的临时 userId（测试断言「两端 ID 不同」、日志归因用）。
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
}

impl Host for HookHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        match key {
            // 身份读：恒回本局临时随机值，绝不读内层——读了就把人的持久设备身份
            // 带进 B 席（同 ID 即回声互吞，见 struct 注）。仅去重键，无身份语义。
            "goptop:userId" => Some(self.user_id.clone()),
            // STUN 读：强制空表——本地候选即刻收集完，免 ICE gathering 的 8s 超时兜底
            //（headless.rs:26-30 既有手法）；Agent 局要的是秒级配对，不是最优链路。
            "goptop:stun" => Some("[]".into()),
            _ => self.inner.storage_get(key),
        }
    }

    fn storage_set(&self, key: &str, value: Option<&str>) {
        // 身份写路径同样吞掉：会话初始化/设置流若把 userId 落盘，覆盖的将是
        // **人的**持久设备身份——这是本装饰存在的第一理由，读写两侧都要堵死。
        if key == "goptop:userId" {
            return;
        }
        self.inner.storage_set(key, value);
    }

    fn copy(&self, text: &str) {
        self.inner.copy(text);
    }

    fn notice(&self, text: Option<&str>, ms: Option<u32>) {
        // 协商 Ack 的物化口：请求方的被拒/被允只见提示条（不改快照），不拦就丢。
        // seq 由队列锁内统一分配（与 watch diff 泵同一入口，绝不撞号）。
        if let Some(t) = text {
            if let Some(ev) = ack_event_from_notice(t) {
                let sink = self.events.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(q) = sink.as_ref().and_then(std::sync::Weak::upgrade) {
                    q.extend_new(vec![ev]);
                }
            }
        }
        self.inner.notice(text, ms);
    }

    fn nav(&self, path: &str) {
        self.inner.nav(path);
    }

    fn emit(&self, snapshot_json: &str) {
        // 事件物化的源头：先推 watch（seq 单调；接收方只关心最新一拍，中间态被
        // watch 覆盖是特性——diff 也以最新拍为基线），再透传内层（TauriHost 还要
        // 推给前端，HeadlessHost 还要计数）。无人接收时 send 回 Err：正常（泵任务
        // 未起/已退），watch 里仍留最新值供后来者取基线。
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.watch_tx.send((seq, snapshot_json.to_string()));
        self.inner.emit(snapshot_json);
    }
}

/// 一条对局事件 —— /game/events 的 JSONL 行形（字段照计划「格式样例（权威基线）」）。
///
/// serde 内部 tag = `"t"`，行例（与计划逐字）：
///
/// ```json
/// {"seq":1,"t":"chat","from":"小明","text":"你好，请多指教"}
/// {"seq":2,"t":"move","by":"black","x":7,"y":7}
/// {"seq":4,"t":"request_received","kind":"undo","from":"小明"}
/// {"seq":5,"t":"request_resolved","kind":"undo","approved":true}
/// ```
///
/// 每个变体都带 `seq`：它是翻页/回查的主键（read offset/limit、grep 回查任意过往
/// 事件），由 [`EventQueue`] 分配、单调递增，绝不由生成方自报。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
// 变体名 snake_case（"request_received" 而非 "RequestReceived"）——t 值照计划
// 「格式样例（权威基线）」逐字；字段名 camelCase 由 rename_all_fields 管。
#[serde(tag = "t", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum GameEvent {
    /// 落子（history 新增 Place 条目；坐标即该手坐标）。
    Move { seq: u64, by: String, x: u16, y: u16 },
    /// 停一手（history 新增 Pass 条目；围棋双 pass 由此走到计分）。
    Pass { seq: u64, by: String },
    /// 聊天（附全文——chat 文件是散文、事件流是结构化，grep 事件即检索全文）。
    Chat { seq: u64, from: String, text: String },
    /// 对手发来协商请求（undo/reset/swap；confirmReq 出现）。
    RequestReceived { seq: u64, kind: String, from: String },
    /// 请求有了结果（对手答复；approved=false 也算 resolved——等待方必须看到拒绝）。
    RequestResolved { seq: u64, kind: String, approved: bool },
    /// 进入终局计分态（围棋双 pass 后；循环见此应确认计分）。
    ScoringStarted { seq: u64 },
    /// 双方确认后的计分结果（字段与快照 scoreResult 同形：black/white/winner/deadRemoved）。
    ScoreResult { seq: u64, black: f32, white: f32, winner: String, dead_removed: u32 },
    /// 终局（winner 出现；认输也走这里——five-exit 的第一退出条件的信号源）。
    GameOver { seq: u64, winner: String },
}

impl GameEvent {
    /// 该事件的 seq（消费方按 seq 判断新旧/翻页，不必 match 全部变体）。
    #[must_use]
    pub fn seq(&self) -> u64 {
        match self {
            Self::Move { seq, .. }
            | Self::Pass { seq, .. }
            | Self::Chat { seq, .. }
            | Self::RequestReceived { seq, .. }
            | Self::RequestResolved { seq, .. }
            | Self::ScoringStarted { seq }
            | Self::ScoreResult { seq, .. }
            | Self::GameOver { seq, .. } => *seq,
        }
    }
}

/// 就地改写事件 seq（[`EventQueue::extend_new`] 的锁内重编号用；生成方先填 0 占位）。
fn set_seq(ev: &mut GameEvent, seq: u64) {
    match ev {
        GameEvent::Move { seq: s, .. }
        | GameEvent::Pass { seq: s, .. }
        | GameEvent::Chat { seq: s, .. }
        | GameEvent::RequestReceived { seq: s, .. }
        | GameEvent::RequestResolved { seq: s, .. }
        | GameEvent::ScoringStarted { seq: s }
        | GameEvent::ScoreResult { seq: s, .. }
        | GameEvent::GameOver { seq: s, .. } => *s = seq,
    }
}

/// 事件队列 = 事件流的两个出口共用的那份数据。
///
/// 「队列」与「全量历史」是**一份数据的两个读法**：未消费游标之前的叫待处理
/// （内置模式工具间隙自动排空 / MCP 模式 wait_events 排空），全部行连起来就是
/// /game/events 文件内容。append-only；seq 由本结构分配、单调递增。
///
/// 锁面用 `std::sync::Mutex`：临界区只有 Vec 追加/切片拷贝，无 await，
/// 同步工具执行与异步事件泵两处都要碰它，std 锁最省心（锁中毒即 bug，静默回空
/// 会把事件流撕出缺口，故中毒按 panic 语义处理）。
#[derive(Default)]
pub struct EventQueue {
    inner: Mutex<EventQueueInner>,
}

#[derive(Default)]
struct EventQueueInner {
    events: Vec<GameEvent>,
    /// 已消费游标（drain 只吐 `events[consumed..]`）。
    consumed: usize,
}

impl EventQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 下一个将分配的 seq（生成方 [`diff_events`] 的 start_seq 取这里）。
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.lock().events.last().map_or(1, |e| e.seq() + 1)
    }

    /// 追加一条事件（seq 由生成方按 [`next_seq`] 预先填好）。
    ///
    /// **必须按 seq 顺序推入**：seq 是 /game/events 的行序，乱序等于文件错行；
    /// debug 断言炸出（release 不查——生成方只有 diff_events 一个，契约内部闭环）。
    pub fn push(&self, ev: GameEvent) {
        let mut g = self.lock();
        debug_assert_eq!(
            ev.seq(),
            g.events.last().map_or(1, |e| e.seq() + 1),
            "事件 seq 必须连续：乱序推入 = /game/events 错行、read/grep 回查错位"
        );
        g.events.push(ev);
    }

    /// 排空全部未消费事件（游标推到末尾）。空队列回空 Vec——
    /// wait_events 的「空超时=`[]`（正常返回非错误）」语义就建立在空 Vec 上。
    pub fn drain(&self) -> Vec<GameEvent> {
        let mut g = self.lock();
        let out = g.events[g.consumed..].to_vec();
        g.consumed = g.events.len();
        out
    }

    /// 追加一批事件，seq 由本方法**在锁内**统一分配（入参里的 seq 只是占位）。
    ///
    /// 为什么不用「next_seq() 取号 → push() 入队」两步式：生成方有两个——watch
    /// diff 泵与 [`HookHost`] 的 notice 拦截（悔棋/重开/换棋被拒时协议只发
    /// Notice 不改快照，见模块注与 [`ack_event_from_notice`]），并发取号会撞号
    /// 把 /game/events 撕出重号。单锁内「分配+追加」原子化，seq 连续由结构保证。
    pub fn extend_new(&self, evs: Vec<GameEvent>) {
        let mut g = self.lock();
        for mut ev in evs {
            let seq = g.events.last().map_or(1, |e| e.seq() + 1);
            set_seq(&mut ev, seq);
            g.events.push(ev);
        }
    }

    /// 未消费条数（等待谓词「队列非空」用，免得每次都拷贝整个 tail）。
    #[must_use]
    pub fn pending(&self) -> usize {
        let g = self.lock();
        g.events.len() - g.consumed
    }

    /// 全量历史（/game/events 文件内容：JSONL，每行一条，含已消费的）。
    #[must_use]
    pub fn history_jsonl(&self) -> String {
        let g = self.lock();
        g.events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 全量历史的结构化拷贝（测试断言「seq 连续、事件完整」用）。
    #[must_use]
    pub fn history(&self) -> Vec<GameEvent> {
        self.lock().events.clone()
    }

    /// 锁入口统一在此：临界区只有 Vec 追加/切片，无 await。中毒即持锁方 panic
    /// （bug），但 append-only 单值追加下 into_inner 拿到的 Vec 仍是一致的旧状态
    /// ——比把毒扩散成新 panic 稳，也绝不静默回空（那会把事件流撕出缺口）。
    fn lock(&self) -> std::sync::MutexGuard<'_, EventQueueInner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 快照 diff 物化：prev → curr 的增量转成事件（seq 自 `start_seq` 起连续编号）。
///
/// `prev = None` 表示首拍：没有基线，返回空 Vec（调用方把 curr 记为下一拍的 prev）。
/// 对应关系（计划「事件物化」节）：history 增长 → Move/Pass（附坐标，按新条目逐条
/// 展开）、chatLog 增长 → Chat（附新条目全文）、confirmReq 出现/消失 →
/// RequestReceived/RequestResolved、scoring 翻真 → ScoringStarted、scoreResult 出现
/// → ScoreResult、winner 出现 → GameOver。
///
/// **事件顺序即发生顺序**：同一拍里多项变化按上述清单顺序展开（落子先于终局、
/// 请求先于其结果），seq 连续无空洞。
#[must_use]
pub fn diff_events(
    prev: Option<&serde_json::Value>,
    curr: &serde_json::Value,
    start_seq: u64,
) -> Vec<GameEvent> {
    let Some(p) = prev else { return Vec::new() };
    let mut out: Vec<GameEvent> = Vec::new();
    // seq 由本函数统一编码（生成方只有这一个，契约内部闭环）；展开顺序即发生顺序
    //（模块注的清单序），中间不跳号。
    let mut seq = start_seq;

    // 1) history 增长 → Move/Pass。协议 HistoryEntry 只有坐标/"pass"，不带行棋方——
    //    执色按序数反推：黑先、悔棋后序号永远从 1 重编，序数奇偶即行棋方。
    let (prev_hist, curr_hist) = (arr_len(p, "history"), arr_len(curr, "history"));
    for i in prev_hist..curr_hist {
        let by = if (i + 1) % 2 == 1 { "black" } else { "white" };
        let entry = &curr["history"][i];
        if entry.is_string() {
            out.push(GameEvent::Pass { seq, by: by.into() });
        } else {
            out.push(GameEvent::Move {
                seq,
                by: by.into(),
                x: entry["x"].as_u64().unwrap_or(0) as u16,
                y: entry["y"].as_u64().unwrap_or(0) as u16,
            });
        }
        seq += 1;
    }

    // 2) chatLog 增长 → Chat（附全文，含自己发的——事件流是全量历史，不只对手的动作）。
    let (prev_chat, curr_chat) = (arr_len(p, "chatLog"), arr_len(curr, "chatLog"));
    for i in prev_chat..curr_chat {
        let entry = &curr["chatLog"][i];
        let from = entry["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| entry["userId"].as_str())
            .unwrap_or_default();
        out.push(GameEvent::Chat {
            seq,
            from: from.into(),
            text: entry["text"].as_str().unwrap_or_default().into(),
        });
        seq += 1;
    }

    // 3) confirmReq 出现/消失/换头 → request_received/request_resolved。换头（队列化
    //    弹窗消费了队首、下一请求顶上）在事件上=先「旧请求有了结果」再「新请求来了」。
    let prev_req = p["confirmReq"].as_object();
    let curr_req = curr["confirmReq"].as_object();
    match (prev_req, curr_req) {
        (Some(pv), Some(cv)) => {
            let (pk, ck) =
                (pv["kind"].as_str().unwrap_or_default(), cv["kind"].as_str().unwrap_or_default());
            if pk != ck {
                out.push(GameEvent::RequestResolved {
                    seq,
                    kind: pk.into(),
                    approved: resolved_approved(pk, p, curr),
                });
                seq += 1;
                out.push(GameEvent::RequestReceived { seq, kind: ck.into(), from: req_from(cv) });
                seq += 1;
            }
        }
        (Some(pv), None) => {
            let kind = pv["kind"].as_str().unwrap_or_default();
            out.push(GameEvent::RequestResolved {
                seq,
                kind: kind.into(),
                approved: resolved_approved(kind, p, curr),
            });
            seq += 1;
        }
        (None, Some(cv)) => {
            out.push(GameEvent::RequestReceived {
                seq,
                kind: cv["kind"].as_str().unwrap_or_default().into(),
                from: req_from(cv),
            });
            seq += 1;
        }
        (None, None) => {}
    }

    // 4) scoring 翻真 → scoring_started（围棋双 pass 终局；回落不产事件）。
    if !p["scoring"].as_bool().unwrap_or(false) && curr["scoring"].as_bool().unwrap_or(false) {
        out.push(GameEvent::ScoringStarted { seq });
        seq += 1;
    }

    // 5) scoreResult 出现 → score_result（字段与快照 scoreResult 同形）。
    if p["scoreResult"].is_null() {
        if let Some(r) = curr["scoreResult"].as_object() {
            out.push(GameEvent::ScoreResult {
                seq,
                black: r["black"].as_f64().unwrap_or(0.0) as f32,
                white: r["white"].as_f64().unwrap_or(0.0) as f32,
                winner: r["winner"].as_str().unwrap_or_default().into(),
                dead_removed: r["deadRemoved"].as_u64().unwrap_or(0) as u32,
            });
            seq += 1;
        }
    }

    // 6) winner 出现 → game_over（五连/认输/计分都汇到这里——five-exit 第一退出条件的信号源）。
    if p["winner"].is_null() {
        if let Some(w) = curr["winner"].as_str() {
            out.push(GameEvent::GameOver { seq, winner: w.into() });
        }
    }

    out
}

/// 快照某数组字段的长度（缺失/类型不符按 0——diff 只关心增量，坏字段不产事件）。
fn arr_len(v: &serde_json::Value, key: &str) -> usize {
    v[key].as_array().map_or(0, |a| a.len())
}

/// confirmReq 的 from 取数：fromName（展示名，计划样例「小明」）优先，缺了回 from。
fn req_from(req: &serde_json::Map<String, serde_json::Value>) -> String {
    req["fromName"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| req["from"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// resolved 的 approved 反推：快照 diff 看不到「批复了什么」，只能从批复的确定性
/// 副作用取证——同意悔棋/重开必缩 history、同意换棋必翻执色（且重开清盘）、同意
/// 计分必出 scoreResult；拒绝则纹丝不动。反推不出（如空盘上的重开）按未同意保守上报，
/// 宁可让等待方多看一眼局面，不可谎报「已同意」。
fn resolved_approved(kind: &str, prev: &serde_json::Value, curr: &serde_json::Value) -> bool {
    match kind {
        "undo" | "reset" => arr_len(curr, "history") < arr_len(prev, "history"),
        "swap" => prev["myColor"] != curr["myColor"] || arr_len(curr, "history") < arr_len(prev, "history"),
        "score-confirm" => !curr["scoreResult"].is_null(),
        // spec-chat / wrong-pwd 与 Agent 席无关（无观战、无钥匙错路），不可观测即未同意。
        _ => false,
    }
}

/// 协商 Ack 提示文案 → 事件（HookHost::notice 的物化规则）。
///
/// 文案来自 goptop-net 的 matchplay（UndoAck/ResetAck/SwapAck 的本地提示，中文、
/// 本仓内稳定）：「同意/拒绝 + 悔棋/重开/换棋」。按关键词双匹配而非整句等值——
/// 文案微调（时长/标点）不破物化，只有 Ack 类提示同时含两者。
/// Agent 作为**批复方**时不出这些提示（批复走本地 confirmReq diff），两条物化
/// 路径互不重叠、不会双报。
fn ack_event_from_notice(text: &str) -> Option<GameEvent> {
    let kind = if text.contains("悔棋") {
        "undo"
    } else if text.contains("重开") {
        "reset"
    } else if text.contains("换棋") {
        "swap"
    } else {
        return None;
    };
    let approved = if text.contains("同意") {
        true
    } else if text.contains("拒绝") {
        false
    } else {
        return None;
    };
    Some(GameEvent::RequestResolved { seq: 0, kind: kind.into(), approved })
}

/// 事件基线：spawn 前由调用方在**本任务**同步取（[`EmitWatch::latest`] 一拍现状），
/// 随参数传给 [`run_event_pump`]。基线取定先于泵任务起跑——晚起的泵不可能把基线
/// 之后的事件当存量吞掉（旧实现泵内自取基线 + 调用方 sleep 屏障，是死等固定时长，
/// 负载下泵首拍晚到必 flaky）。
///
/// seq==0（还没任何一拍）回 None：首拍也只当基线（快照是全量的，首拍把存量聊天/
/// 落子当事件重发一遍是噪声）。
#[must_use]
pub fn event_baseline(watch: &EmitWatch) -> Option<serde_json::Value> {
    let (seq, text) = watch.latest();
    (seq != 0).then(|| parse_snap(&text))
}

/// watch → 队列的物化泵：每拍 diff 并把新事件推进队列。基线经
/// [`event_baseline`] 在 spawn 前取定后由 `prev` 传入。
///
/// 独立 tokio 任务跑（调用方 `tokio::spawn`）；Agent 局解散 = watch 的 sender 随
/// [`HookHost`] drop，`changed()` 回 Err 后本函数返回、任务自然退出——**不需要也不允许
/// 外部强杀**（强杀会把 Arc 留给别处，泄漏路径见 transport 的停机标志讨论）。
pub async fn run_event_pump(
    mut watch: EmitWatch,
    queue: Arc<EventQueue>,
    mut prev: Option<serde_json::Value>,
) {
    // diff 的 start_seq 每拍现取（队列推进到哪就接着编哪），拍与拍之间 seq 连续无空洞。
    loop {
        let (_seq, text) = match watch.changed().await {
            Ok(frame) => frame,
            // sender 全部 drop = 局散（Paired drop → HookHost drop）：任务自然收摊。
            Err(_) => return,
        };
        let curr = parse_snap(&text);
        if let Some(p) = prev.as_ref() {
            // seq 占位 0，extend_new 在锁内统一重编号（与 notice 拦截共入口，绝不撞号）。
            queue.extend_new(diff_events(Some(p), &curr, 0));
        }
        prev = Some(curr);
    }
}

/// 快照文本 → Value（坏文本按 Null：diff 全字段取不到 → 不产事件，与 NativePlayer
/// 的快照兜底同一口径）。
fn parse_snap(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use goptop_transport_native::HeadlessHost;
    use serde_json::json;

    /* ---------------- HookHost ---------------- */

    #[test]
    fn hook_host_user_id临时且不落盘() {
        let inner = Arc::new(HeadlessHost::default());
        let (host, _watch) = HookHost::wrap(inner.clone());
        let uid = host.user_id().to_string();
        assert!(uid.starts_with("u-"), "临时 userId 与持久身份同形：{uid}");
        // 读恒回临时值，但绝不写进内层存储
        assert_eq!(host.storage_get("goptop:userId").as_deref(), Some(uid.as_str()));
        assert!(
            !inner.storage.lock().unwrap().contains_key("goptop:userId"),
            "临时 userId 不落盘"
        );
        // 写路径同样吞掉：会话初始化/设置流不得把人的持久身份覆盖成临时值
        host.storage_set("goptop:userId", Some("u-hacked"));
        assert!(!inner.storage.lock().unwrap().contains_key("goptop:userId"));
        assert_eq!(host.storage_get("goptop:userId").as_deref(), Some(uid.as_str()));
    }

    #[test]
    fn hook_host_stun强制空_其余透传() {
        let inner = Arc::new(HeadlessHost::default());
        inner.storage_set("goptop:name", Some("甲"));
        let (host, _w) = HookHost::wrap(inner.clone());
        assert_eq!(host.storage_get("goptop:stun").as_deref(), Some("[]"), "stun 强制空，免 8s gathering");
        assert!(!inner.storage.lock().unwrap().contains_key("goptop:stun"), "stun 是覆写不是写盘");
        assert_eq!(host.storage_get("goptop:name").as_deref(), Some("甲"), "其余键读透传");
        host.storage_set("goptop:name", Some("乙"));
        assert_eq!(inner.storage_get("goptop:name").as_deref(), Some("乙"), "其余键写透传");
        host.notice(Some("提示"), None);
        host.nav("/p2p");
        host.copy("文本");
        assert_eq!(inner.notices.lock().unwrap().len(), 1);
        assert_eq!(inner.navs.lock().unwrap().first().map(String::as_str), Some("/p2p"));
    }

    #[test]
    fn hook_host两次包装得不同临时id() {
        let (h1, _) = HookHost::wrap(Arc::new(HeadlessHost::default()));
        let (h2, _) = HookHost::wrap(Arc::new(HeadlessHost::default()));
        assert_ne!(h1.user_id(), h2.user_id(), "同毫秒生成也不许撞车：同 ID = 回声互吞");
    }

    #[test]
    fn hook_host_emit推watch且透传() {
        let inner = Arc::new(HeadlessHost::default());
        let (host, watch) = HookHost::wrap(inner.clone());
        host.emit(r#"{"phase":"home"}"#);
        host.emit(r#"{"phase":"waiting"}"#);
        let (seq, text) = watch.latest();
        assert_eq!(seq, 2, "seq 单调递增，接收方取到最新一拍");
        assert_eq!(text, r#"{"phase":"waiting"}"#);
        assert!(
            inner.emits.load(Ordering::Relaxed) >= 2,
            "emit 仍透传内层（TauriHost 还要推前端）"
        );
    }

    /* ---------------- GameEvent / EventQueue ---------------- */

    #[test]
    fn game_event_serde逐字对齐计划样例() {
        // /game/events 的行形（计划「格式样例（权威基线）」逐字）——t 值 snake_case
        let chat: GameEvent =
            serde_json::from_str(r#"{"seq":1,"t":"chat","from":"小明","text":"你好，请多指教"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&chat).unwrap(),
            json!({"seq":1,"t":"chat","from":"小明","text":"你好，请多指教"})
        );
        let req: GameEvent =
            serde_json::from_str(r#"{"seq":4,"t":"request_received","kind":"undo","from":"小明"}"#).unwrap();
        assert_eq!(serde_json::to_value(&req).unwrap(), json!({"seq":4,"t":"request_received","kind":"undo","from":"小明"}));
        let res: GameEvent =
            serde_json::from_str(r#"{"seq":5,"t":"request_resolved","kind":"undo","approved":true}"#).unwrap();
        assert_eq!(serde_json::to_value(&res).unwrap(), json!({"seq":5,"t":"request_resolved","kind":"undo","approved":true}));
        let mv: GameEvent = serde_json::from_str(r#"{"seq":2,"t":"move","by":"black","x":7,"y":7}"#).unwrap();
        assert_eq!(mv.seq(), 2);
    }

    #[test]
    fn 事件队列_追加排空与历史() {
        let q = EventQueue::new();
        assert_eq!(q.next_seq(), 1);
        assert_eq!(q.drain(), Vec::new(), "空队列排空回空（wait_events 空超时=[] 的地基）");
        q.push(GameEvent::Move { seq: 1, by: "black".into(), x: 7, y: 7 });
        q.push(GameEvent::Pass { seq: 2, by: "white".into() });
        assert_eq!(q.next_seq(), 3);
        assert_eq!(q.pending(), 2);
        assert_eq!(q.drain().len(), 2, "排空吐出全部未消费");
        assert_eq!(q.pending(), 0);
        assert_eq!(q.drain(), Vec::new(), "消费后再排空为空");
        // 历史仍在（含已消费），seq 分配不受消费游标影响
        assert_eq!(q.history().len(), 2);
        assert_eq!(q.next_seq(), 3);
        let jsonl = q.history_jsonl();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2, "每行一条 JSONL");
        assert!(lines[0].contains(r#""t":"move""#), "线上 t 值照计划样例：{jsonl}");
        assert!(lines[1].contains(r#""t":"pass""#));
    }

    /* ---------------- diff_events ---------------- */

    #[test]
    fn extend_new锁内重编号_与push共存连续() {
        let q = EventQueue::new();
        q.push(GameEvent::Move { seq: 1, by: "black".into(), x: 7, y: 7 });
        // extend_new 忽略入参 seq（0 占位），锁内接着队尾编号。
        q.extend_new(vec![
            GameEvent::RequestResolved { seq: 0, kind: "undo".into(), approved: false },
            GameEvent::Chat { seq: 0, from: "甲".into(), text: "hi".into() },
        ]);
        let hist = q.history();
        let seqs: Vec<u64> = hist.iter().map(|e| e.seq()).collect();
        assert_eq!(seqs, vec![1, 2, 3], "两路生成方共入一口，seq 连续：{seqs:?}");
    }

    #[test]
    fn notice拦截_协商ack物化为request_resolved() {
        let (host, _watch) = HookHost::wrap(Arc::new(HeadlessHost::default()));
        let q = Arc::new(EventQueue::new());
        host.bind_events(&q);
        // 请求方视角的被拒/被允提示（matchplay 的本地 Ack 文案）。
        host.notice(Some("对方拒绝了悔棋"), Some(2400));
        host.notice(Some("对方已同意重开"), Some(2400));
        host.notice(Some("双方连续停一手，进入终局计分"), Some(5200)); // 非 Ack，不物化
        assert_eq!(
            q.history(),
            vec![
                GameEvent::RequestResolved { seq: 1, kind: "undo".into(), approved: false },
                GameEvent::RequestResolved { seq: 2, kind: "reset".into(), approved: true },
            ],
            "Ack 提示物化为事件、非 Ack 透传不物化"
        );
        // 未绑定的宿主：notice 纯透传不炸（弱引用 None 分支）。
        let (host2, _w2) = HookHost::wrap(Arc::new(HeadlessHost::default()));
        host2.notice(Some("对方拒绝了悔棋"), None);
    }

    #[test]
    fn diff_首拍与无变化都回空() {
        let snap = json!({"history": [], "chatLog": [], "confirmReq": null, "scoring": false, "scoreResult": null, "winner": null});
        assert!(diff_events(None, &snap, 1).is_empty(), "首拍没有基线，只记不发");
        assert!(diff_events(Some(&snap), &snap, 1).is_empty(), "无变化无事件");
    }

    #[test]
    fn diff_history增长展开move与pass() {
        let prev = json!({"history": []});
        let curr = json!({"history": [{"x":7,"y":7}, "pass", {"x":8,"y":8}]});
        assert_eq!(
            diff_events(Some(&prev), &curr, 1),
            vec![
                GameEvent::Move { seq: 1, by: "black".into(), x: 7, y: 7 },
                GameEvent::Pass { seq: 2, by: "white".into() },
                GameEvent::Move { seq: 3, by: "black".into(), x: 8, y: 8 },
            ],
            "逐条展开、执色按序数交替（黑先）、seq 连续"
        );
    }

    #[test]
    fn diff_chat增长附全文() {
        let prev = json!({"chatLog": [{"userId":"u-1","name":"小明","text":"你好"}]});
        let curr = json!({"chatLog": [{"userId":"u-1","name":"小明","text":"你好"},{"userId":"u-2","name":"Agent","text":"请多指教"}]});
        assert_eq!(
            diff_events(Some(&prev), &curr, 9),
            vec![GameEvent::Chat { seq: 9, from: "Agent".into(), text: "请多指教".into() }],
            "只发新增条目，附全文"
        );
    }

    #[test]
    fn diff_confirm出现与消失() {
        let base = json!({"history": [{"x":0,"y":0}], "confirmReq": null});
        let with_req = json!({"history": [{"x":0,"y":0}], "confirmReq": {"kind":"undo","from":"u-1","fromName":"小明","queued":1}});
        assert_eq!(
            diff_events(Some(&base), &with_req, 1),
            vec![GameEvent::RequestReceived { seq: 1, kind: "undo".into(), from: "小明".into() }],
            "出现→received，from 取展示名"
        );
        // 同意悔棋：请求消失且 history 缩短（批复的确定性副作用）→ approved=true
        let approved = json!({"history": [], "confirmReq": null});
        assert_eq!(
            diff_events(Some(&with_req), &approved, 2),
            vec![GameEvent::RequestResolved { seq: 2, kind: "undo".into(), approved: true }]
        );
        // 拒绝：请求消失而局面纹丝不动 → approved=false（等待方必须看到拒绝）
        let declined = json!({"history": [{"x":0,"y":0}], "confirmReq": null});
        assert_eq!(
            diff_events(Some(&with_req), &declined, 2),
            vec![GameEvent::RequestResolved { seq: 2, kind: "undo".into(), approved: false }]
        );
    }

    #[test]
    fn diff_队首换头_先结果后新请求() {
        let undo_head = json!({"myColor":"black","confirmReq": {"kind":"undo","from":"u-1","fromName":"小明","queued":2}});
        let reset_head = json!({"myColor":"black","confirmReq": {"kind":"reset","from":"u-1","fromName":"小明","queued":1}});
        assert_eq!(
            diff_events(Some(&undo_head), &reset_head, 4),
            vec![
                GameEvent::RequestResolved { seq: 4, kind: "undo".into(), approved: false },
                GameEvent::RequestReceived { seq: 5, kind: "reset".into(), from: "小明".into() },
            ],
            "旧请求先有了结果、新请求才顶上（因果顺序）"
        );
    }

    #[test]
    fn diff_终局链_计分_结果_胜者() {
        let prev = json!({"scoring": false, "scoreResult": null, "winner": null});
        let scoring = json!({"scoring": true, "scoreResult": null, "winner": null});
        assert_eq!(
            diff_events(Some(&prev), &scoring, 1),
            vec![GameEvent::ScoringStarted { seq: 1 }]
        );
        let scored = json!({"scoring": true, "scoreResult": {"black": 9.0, "white": 3.5, "winner": "black", "deadRemoved": 2}, "winner": "black"});
        assert_eq!(
            diff_events(Some(&scoring), &scored, 7),
            vec![
                GameEvent::ScoreResult { seq: 7, black: 9.0, white: 3.5, winner: "black".into(), dead_removed: 2 },
                GameEvent::GameOver { seq: 8, winner: "black".into() },
            ],
            "scoreResult 先于 game_over（同一拍的清单展开序）"
        );
        // 认输直接见 winner（不进 history）
        let resigned = json!({"scoring": false, "scoreResult": null, "winner": "white"});
        assert_eq!(
            diff_events(Some(&prev), &resigned, 3),
            vec![GameEvent::GameOver { seq: 3, winner: "white".into() }]
        );
    }

    /* ---------------- NativePlayer / run_event_pump（真会话） ---------------- */

    fn test_player() -> NativePlayer {
        let host: Arc<dyn Host> = Arc::new(HeadlessHost::default());
        NativePlayer::new(Arc::new(NativeSession::new(
            goptop_transport_native::SessionConfig {
                name: "测试".into(),
                server_mode: false,
                share_origin: "https://goptop.pages.dev".into(),
                kind: "gomoku".into(),
                size: 15,
            },
            host,
            "http://localhost/p2p",
        )))
    }

    #[tokio::test]
    async fn native_player_命令快照与等待() {
        let player = test_player();
        // snapshot 是 Value 形态、字段 camelCase
        assert_eq!(player.snapshot()["phase"], json!("home"));
        assert_eq!(player.snapshot()["kind"], json!("gomoku"));
        // cmd 生效：同步泵让命令当场落状态
        player.cmd(UiCommand::SetName("新名".into()));
        assert_eq!(player.snapshot()["name"], json!("新名"));
        // wait_until：已成立的谓词立即真
        assert!(player.wait_until(&mut |s| s["phase"] == json!("home"), Duration::from_millis(200)));
        // 永不成立的谓词按超时回假，不提前返回
        let started = std::time::Instant::now();
        assert!(!player.wait_until(&mut |_s| false, Duration::from_millis(200)));
        assert!(started.elapsed() >= Duration::from_millis(200), "超时前不得提前返回假");
    }

    #[tokio::test]
    async fn 事件泵_watch每拍diff进队列_局散即退() {
        let (host, watch) = HookHost::wrap(Arc::new(HeadlessHost::default()));
        let queue = Arc::new(EventQueue::new());
        // 基线在 spawn 前取定（此刻 seq==0 → None，第一拍也只当基线）——不需要
        // sleep 等「泵先起」，晚起的泵也吞不掉基线之后的事件。
        let baseline = event_baseline(&watch);
        let handle =
            tokio::spawn(run_event_pump(EmitWatch::new(watch.clone_rx()), queue.clone(), baseline));
        host.emit(r#"{"chatLog":[{"name":"小明","text":"你好"}]}"#);
        tokio::time::sleep(Duration::from_millis(50)).await;
        host.emit(r#"{"chatLog":[{"name":"小明","text":"你好"},{"name":"Agent","text":"请多指教"}]}"#);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            queue.history(),
            vec![GameEvent::Chat { seq: 1, from: "Agent".into(), text: "请多指教".into() }],
            "基线拍存量不重发，增量才产事件"
        );
        assert_eq!(queue.pending(), 1);
        // 局散：HookHost（watch sender 持有者）drop → changed() 回 Err → 任务自然退出
        drop(host);
        let done = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(done.is_ok(), "sender 全 drop 后泵任务必须自然退出（不需要外部强杀）");
    }
}
