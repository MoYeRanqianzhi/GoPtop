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
//! 上一拍 diff → 同一事件流喂两个出口：**事件队列**（[`EventQueue`]，新事件待消费）
//! + **/game/events 全量历史文件**（append-only，seq 单调）。事件种类：history 增长
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
        todo!()
    }

    fn snapshot(&self) -> serde_json::Value {
        todo!()
    }

    fn pump(&self) {
        todo!()
    }

    fn wait_until(&self, pred: &mut dyn FnMut(&serde_json::Value) -> bool, timeout: Duration) -> bool {
        todo!()
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

/// B 席的装饰宿主：拦截三个键、覆写 emit，其余全部透传给内层宿主
/// （桌面=TauriHost，测试=HeadlessHost）。
///
/// 三处拦截（计划「会话对」节第 2 条）：
/// - `goptop:userId` 读 → 本局临时随机值（**不写存储**；写路径同样吞掉，
///   免得会话初始化把人的持久设备身份覆盖成临时值）；
/// - `goptop:stun` 读 → 强制 `"[]"`（免 ICE gathering 的 8s 超时兜底，
///   headless.rs:26-30 的既有手法——Agent 局要的是秒级配对，不是最优链路）；
/// - `emit` → 先推 [`EmitWatch`] 再透传（事件物化的源头）。
pub struct HookHost {
    inner: Arc<dyn Host>,
    /// 本局临时随机 userId（u- 前缀，与持久身份同形——只求 bc.rs 的回声过滤认它）。
    user_id: String,
    watch_tx: watch::Sender<(u64, String)>,
    seq: AtomicU64,
}

impl HookHost {
    /// 装饰内层宿主并建 watch，一次拿全（宿主进 `NativeSession::new`，watch 给事件泵）。
    ///
    /// 临时 userId 用 `identity::gen_user_id` + transport 的 `rand4()`：与既有身份
    /// 生成同一条路，熵来源（进程级 RandomState + 时间 + 自增）已验证防同毫秒撞车
    ///（lib.rs 的 rand4 说明——两个会话同 userId 会让挑战发给自己）。
    pub fn wrap(inner: Arc<dyn Host>) -> (Arc<Self>, EmitWatch) {
        todo!()
    }

    /// 本局的临时 userId（测试断言「两端 ID 不同」、日志归因用）。
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
}

impl Host for HookHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        todo!()
    }

    fn storage_set(&self, key: &str, value: Option<&str>) {
        todo!()
    }

    fn copy(&self, text: &str) {
        todo!()
    }

    fn notice(&self, text: Option<&str>, ms: Option<u32>) {
        todo!()
    }

    fn nav(&self, path: &str) {
        todo!()
    }

    fn emit(&self, snapshot_json: &str) {
        todo!()
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
#[serde(tag = "t", rename_all_fields = "camelCase")]
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
        todo!()
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
        todo!()
    }

    /// 追加一条事件（seq 由生成方按 [`next_seq`] 预先填好）。
    ///
    /// **必须按 seq 顺序推入**：seq 是 /game/events 的行序，乱序等于文件错行；
    /// debug 断言炸出（release 不查——生成方只有 diff_events 一个，契约内部闭环）。
    pub fn push(&self, ev: GameEvent) {
        todo!()
    }

    /// 排空全部未消费事件（游标推到末尾）。空队列回空 Vec——
    /// wait_events 的「空超时=`[]`（正常返回非错误）」语义就建立在空 Vec 上。
    pub fn drain(&self) -> Vec<GameEvent> {
        todo!()
    }

    /// 未消费条数（等待谓词「队列非空」用，免得每次都拷贝整个 tail）。
    #[must_use]
    pub fn pending(&self) -> usize {
        todo!()
    }

    /// 全量历史（/game/events 文件内容：JSONL，每行一条，含已消费的）。
    #[must_use]
    pub fn history_jsonl(&self) -> String {
        todo!()
    }

    /// 全量历史的结构化拷贝（测试断言「seq 连续、事件完整」用）。
    #[must_use]
    pub fn history(&self) -> Vec<GameEvent> {
        todo!()
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
    todo!()
}

/// watch → 队列的物化泵：每拍 diff 并把新事件推进队列。
///
/// 独立 tokio 任务跑（调用方 `tokio::spawn`）；首拍只记基线不产事件（快照是全量的，
/// 首拍把存量聊天/落子当事件重发一遍是噪声）。Agent 局解散 = watch 的 sender 随
/// [`HookHost`] drop，`changed()` 回 Err 后本函数返回、任务自然退出——**不需要也不允许
/// 外部强杀**（强杀会把 Arc 留给别处，泄漏路径见 transport 的停机标志讨论）。
pub async fn run_event_pump(mut watch: EmitWatch, queue: Arc<EventQueue>) {
    todo!()
}
