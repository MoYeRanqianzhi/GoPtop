//! 进程内配对 —— A'（人的专用会话）与 B（Agent 无头会话）结成对局。
//!
//! 复刻 `crates/goptop-transport-native/tests/headless.rs:90-125` 已验证的路径：
//! 邀请方 `PickKind/PickSize → CreateInvite` → 轮询 `inviteUrl` 含 `"rtc="`（≤40s）
//! → 受邀方以该 URL **经 Boot 路径**创建 → 双方 `phase=="playing"` 且 `peerConnected`。
//! 不用 `AcceptInvite` 粘贴路径：它在「无服务器 + 链接带 rtc」时不发 `CreatePeer`，
//! 是既有实现的空档（headless.rs:84-89 注释），两个方向都要绕开它。
//!
//! **方向随执子选择**（计划「会话对」节第 3 条）：「邀请人=黑」是会话构造的固有
//! 结果（lobby.rs 的构造恰好实现谁邀请谁执黑），所以：
//! - 我执黑（默认）= A' 先 `CreateInvite`，B 以其链接 Boot 入局；
//! - 我执白 = 反向——B（无头会话）先 `CreateInvite`，A' 携 B 的链接经 Boot 创建
//!   （`session_new` 的 url 参数就是为这条路留的）。
//!
//! **单局互斥的前提**：进程内 BC hub 全局无局号（bc.rs 头注自证缺口），同一时刻
//! 只允许一个 Agent 局；拦截面（拒绝主会话开局类命令）在 src-tauri 的 AgentHub，
//! 不在本 crate——本 crate 只管把一局结起来、结不起来就清理干净。
//!
//! **仅 native**（阶段⑤契约 §3.3）：本文件以 `NativeSession` 为硬类型，而 web 的
//! A' 由前端持有（WasmSession.new_agent）、B 由 Hub 建——pair() 的「双向双建」
//! 形态在 web 无对应物；web 侧配对原语在 goptop-transport 的 agent 模块镜像
//! src-tauri 壳内做法（A' 归前端、B 由 Hub 建）。
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use goptop_net::session::UiCommand;
use goptop_transport_native::{Host, NativeSession, SessionConfig};

use crate::player::{EmitWatch, HookHost, NativePlayer, PlayerHandle};

/// 用户（人）执色 —— 决定配对方向（见模块注）。
///
/// **为什么不是存个「谁邀请谁」的布尔**：执色是对局语义、邀请方向是它的实现后果，
/// 配置面（设置卡「我执黑/我执白」）与测试断言都以执色为准，方向只在 pair 内部成立。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatColor {
    /// 人执黑（默认）：A' 邀请，B 入局。
    Black,
    /// 人执白：B 邀请，A' 携链入局。
    White,
}

impl SeatColor {
    /// 线上执色字符串（快照 myColor 同款值域：`"black"` / `"white"`）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::White => "white",
        }
    }

    /// 对面席的执色。
    #[must_use]
    pub fn opponent(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

/// 配对参数。棋种/路数/执色以用户 UI 配置为权威（MCP 模式下外部 Agent 的
/// `game_start` 无权改——它只有「认领席位」的份）。
///
/// `Clone`：mcp 出口的认领段把配置按值喂 `pair()`，失败时要把待局**原样还回**
/// 槽里供外部 Agent 重试——克隆一份进去，原件留槽（字段全是 String/数值/Arc
/// 宿主句柄，克隆廉价且共享同一宿主）。
#[derive(Clone)]
pub struct PairConfig {
    /// 棋种（`"gomoku"` / `"go"`；非法组合由状态机按 make_engine_kind 回退默认，
    /// 与真人主页选棋种同一套守卫）。
    pub kind: String,
    /// 路数（9/13/19 围棋、15 五子棋）。
    pub size: u16,
    /// 用户执色 → 配对方向。
    pub my_color: SeatColor,
    /// A' 的展示名（进聊天记录的用户名）。
    pub front_name: String,
    /// B 的展示名（默认「Agent」，仅聊天展示，**非身份**——Agent 无身份）。
    pub agent_name: String,
    /// 分享基地址（链接构造用；桌面=云端部署常量，测试=任意 http 原点）。
    pub share_origin: String,
    /// A' 的宿主（桌面=TauriHost 裸用——A' 的快照由前端 poll，不需要 watch；
    /// 测试=HeadlessHost）。
    pub front_host: Arc<dyn Host>,
    /// B 的宿主基座——pair() 内部把它包进 [`HookHost`]（临时 userId / stun 空 /
    /// emit 推 watch），**不要在调用方先包**（双层 HookHost 会把临时 ID 藏进里层）。
    pub agent_host: Arc<dyn Host>,
}

/// 配对产物：两席 + B 的事件源头。
pub struct Paired {
    /// A'：人的专用会话（前端渲染与轮询它；AgentHub 的拦截面豁免它自己的 id）。
    pub front: NativePlayer,
    /// B：Agent 的无头会话（循环驱动它；决策循环与工具层经 PlayerHandle 用它）。
    pub agent: NativePlayer,
    /// B 的快照推送流（事件物化的源头；工具谓词等待与 wait_events 从这里长出来）。
    pub agent_watch: EmitWatch,
    /// B 的装饰宿主（取临时 userId 用；生命周期与整局同长，局散随 [`Paired`] drop）。
    pub agent_hook: Arc<HookHost>,
}

/// 结成一局。40s 上限（headless.rs 同款）：ICE gathering 耗时随环境波动（无 STUN
/// 也可能走到超时兜底），死等固定轮数必 flaky——所以是「轮询到谓词或超时」。
///
/// 成功路径：两席各自创建（受邀方创建即 `start_pump()`，它的 IO 任务要自己转）；
/// 等到双方 `phase=="playing"` 且 `peerConnected` 才返回——在此之前落子会被
/// can_place 静默拒绝，把「半连接」的局交出去只会收获一份「为什么我下不了棋」。
///
/// 失败路径：任何一步超时/失败 → 两席就地 drop（`NativeSession` 的 Drop 置停机
/// 标志，RTC 连接与订阅任务随停），回人话错误文本、可整体重试。**不允许返回半成品**
/// ——半连接的 `Paired` 会让上层在错误的等待点上再等 40s，两层超时叠出难归因的卡死。
pub async fn pair(cfg: PairConfig) -> Result<Paired, String> {
    // B 一律在本次包装进 HookHost（临时 userId / stun 空 / emit 推 watch）——
    // 若调用方已先包一层，临时 ID 就藏进了里层、这里读到的是外层的持久身份。
    let (agent_hook, agent_watch) = HookHost::wrap(cfg.agent_host.clone());
    // 两会话固定无服务器（server_mode:false，见 PairConfig 注）；邀请方的 Boot 基座
    // 链接走 p2p 意图（on_boot 只认意图不落页），与 headless.rs 同款。
    let base = format!("{}/p2p", cfg.share_origin.trim_end_matches('/'));
    let front_cfg = SessionConfig {
        name: cfg.front_name.clone(),
        server_mode: false,
        share_origin: cfg.share_origin.clone(),
        kind: cfg.kind.clone(),
        size: cfg.size,
    };
    let agent_cfg = SessionConfig {
        name: cfg.agent_name.clone(),
        server_mode: false,
        share_origin: cfg.share_origin.clone(),
        kind: cfg.kind.clone(),
        size: cfg.size,
    };

    // 方向随执色（模块注）：邀请人=黑是 lobby.rs 的构造结果，谁邀请谁执黑。
    // 两个方向都走「CreateInvite → 等 rtc= → 受邀方携链接 Boot」这条已验证路径，
    // 绝不碰 AcceptInvite 粘贴路径（它在链接带 rtc 时不发 CreatePeer，是既有空档）。
    match cfg.my_color {
        SeatColor::Black => {
            let front_session = Arc::new(NativeSession::new(front_cfg, cfg.front_host.clone(), &base));
            let front = NativePlayer::new(front_session);
            create_invite(&front).await?;
            let link = invite_link(&front)
                .ok_or_else(|| "邀请链接未就绪（缺 rtc=）".to_string())?;
            // 受邀方创建即 start_pump：B 的 IO 任务要自己转——决策循环每拍只泵一次，
            // 对手的数据面消息全靠常驻泵喂进状态机。
            let agent_session = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &link));
            agent_session.start_pump();
            let agent = NativePlayer::new(agent_session);
            wait_playing(&front, &agent).await?;
            Ok(Paired { front, agent, agent_watch, agent_hook })
        }
        SeatColor::White => {
            // B（无头会话）先建先邀请：它无论坐哪席都要自转，理由同上。
            let agent_session = Arc::new(NativeSession::new(agent_cfg, agent_hook.clone(), &base));
            agent_session.start_pump();
            let agent = NativePlayer::new(agent_session);
            create_invite(&agent).await?;
            let link = invite_link(&agent)
                .ok_or_else(|| "邀请链接未就绪（缺 rtc=）".to_string())?;
            // A' 此方向是受邀方：创建即 start_pump（同一条契约——没有别人为它转泵，
            // 前端轮询只读快照，不消化 IO 回调入队的事件）。
            let front_session = Arc::new(NativeSession::new(front_cfg, cfg.front_host.clone(), &link));
            front_session.start_pump();
            let front = NativePlayer::new(front_session);
            wait_playing(&front, &agent).await?;
            Ok(Paired { front, agent, agent_watch, agent_hook })
        }
    }
}

/// 邀请方的半场：`CreateInvite` → 轮询 `inviteUrl` 含 `rtc=`（≤40s）。
/// 同源链接一开始就有，但**不含 rtc= 的不算就绪**——那意味着 offer 还没编进链接。
async fn create_invite(inviter: &NativePlayer) -> Result<(), String> {
    inviter.cmd(UiCommand::CreateInvite);
    let ok = pump_until(inviter, inviter, || invite_link(inviter).is_some(), 40).await;
    if ok {
        return Ok(());
    }
    Err("40 秒内未生成含 rtc 的邀请链接（ICE gathering 未完成？）".into())
}

/// 收官判定：双方 `phase=="playing"` 且 `peerConnected`。在此之前落子会被 can_place
/// 静默拒绝——把半连接的局交出去，只会收获一份「为什么我下不了棋」。
async fn wait_playing(front: &NativePlayer, agent: &NativePlayer) -> Result<(), String> {
    let ok = pump_until(
        front,
        agent,
        || {
            is_str(front, "phase", "playing")
                && is_str(agent, "phase", "playing")
                && connected(front)
                && connected(agent)
        },
        40,
    )
    .await;
    if ok {
        return Ok(());
    }
    // 失败要带两席的现场：可归因（卡在 waiting=信令没走到、phase=playing 但
    // peerConnected=false=ICE 没通）比一句「超时」有用得多。
    Err(format!(
        "40 秒内未双双进入对局态：front phase={} connected={} / agent phase={} connected={}",
        str_of(front, "phase"),
        front.snapshot()["peerConnected"],
        str_of(agent, "phase"),
        agent.snapshot()["peerConnected"],
    ))
}

/// 邀请链接是否就绪（`rtc=` 参数在场）。
fn invite_link(p: &NativePlayer) -> Option<String> {
    p.snapshot()["inviteUrl"].as_str().filter(|u| u.contains("rtc=")).map(str::to_string)
}

fn is_str(p: &NativePlayer, key: &str, want: &str) -> bool {
    p.snapshot()[key].as_str() == Some(want)
}

fn str_of(p: &NativePlayer, key: &str) -> String {
    p.snapshot()[key].as_str().unwrap_or_default().to_string()
}

fn connected(p: &NativePlayer) -> bool {
    p.snapshot()["peerConnected"] == serde_json::Value::Bool(true)
}

/// 泵两席到谓词成立（或超时）。**条件等待而非固定轮数**：ICE gathering 耗时随环境
/// 波动（无 STUN 也可能走到超时兜底），死等固定时长必然 flaky（headless.rs:62-82 手法）。
/// pair 是配对期的临时「测试夹具」，所以这里泵**两**席——PlayerHandle::wait_until 只泵
/// 自席的纪律是对决策层说的，配对期两席都归本函数驱动。
async fn pump_until(a: &NativePlayer, b: &NativePlayer, pred: impl Fn() -> bool, secs: u64) -> bool {
    let deadline = crate::time_compat::now_ms() + secs * 1000;
    while crate::time_compat::now_ms() < deadline {
        a.pump();
        b.pump();
        if pred() {
            return true;
        }
        crate::time_compat::delay(50).await;
    }
    a.pump();
    b.pump();
    pred()
}

#[cfg(test)]
mod tests {
    // SERIAL 锁必须横跨整个用例的 await（串行化的本意）——与 loop_headless.rs /
    // headless.rs 的既有纪律同款，不是「锁忘放」。
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::player::{EventQueue, GameEvent, event_baseline, run_event_pump};
    use goptop_transport_native::HeadlessHost;
    use serde_json::{Value, json};

    /// 进程内 BC hub 全局无局号（bc.rs 头注自证缺口）——配对用例必须串行，
    /// 并行跑的用例会互相收到对方的消息（headless.rs 同款纪律）。
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 无 STUN 的宿主：本地候选即刻收集完，省掉 gathering 的 8s 超时兜底。
    fn headless_host() -> Arc<HeadlessHost> {
        let h = Arc::new(HeadlessHost::default());
        h.storage_set("goptop:stun", Some("[]"));
        h
    }

    fn cfg(kind: &str, size: u16, color: SeatColor) -> PairConfig {
        PairConfig {
            kind: kind.into(),
            size,
            my_color: color,
            front_name: "甲".into(),
            agent_name: "Agent".into(),
            share_origin: "http://localhost".into(),
            front_host: headless_host(),
            agent_host: headless_host(),
        }
    }

    // 配对用例用多线程运行时：B 有常驻后台泵、wait_until 是阻塞等待（std 睡眠）——
    // 与桌面壳的多线程 tokio 同构。current_thread 下阻塞等待会饿死 IO 任务，
    // 那是测试运行时的形状问题，不是被测代码的。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 我执黑_甲邀请_agent携链入局() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let agent_base = headless_host();
        let mut c = cfg("gomoku", 15, SeatColor::Black);
        c.agent_host = agent_base.clone();
        let paired = pair(c).await.expect("配对应成功");
        let (f, a) = (&paired.front, &paired.agent);
        assert_eq!(str_of(f, "phase"), "playing");
        assert_eq!(str_of(a, "phase"), "playing");
        assert_eq!(f.snapshot()["peerConnected"], Value::Bool(true));
        assert_eq!(a.snapshot()["peerConnected"], Value::Bool(true));
        // 邀请人=黑：人执黑、Agent 执白
        assert_eq!(str_of(f, "myColor"), "black");
        assert_eq!(str_of(a, "myColor"), "white");
        // B 的临时 userId：与 A' 不同、同形、不落 B 的宿主存储（Agent 无身份）
        let uid = paired.agent_hook.user_id();
        assert!(uid.starts_with("u-"), "临时 userId 与持久身份同形：{uid}");
        assert_ne!(str_of(a, "userId"), str_of(f, "userId"), "两端 ID 不同，否则回声互吞");
        assert!(
            !agent_base.storage.lock().unwrap().contains_key("goptop:userId"),
            "B 的临时 userId 绝不落盘"
        );
        assert!(paired.agent_watch.latest().0 > 0, "B 的 emit 应已推 watch");

        // 事件泵端到端：基线在 spawn 前于本任务同步取定，再把整局驱动起来——
        // 晚起的泵也吞不掉基线之后的事件（固定 sleep 屏障是死等，负载下必 flaky）。
        let queue = Arc::new(EventQueue::new());
        let baseline = event_baseline(&paired.agent_watch);
        tokio::spawn(run_event_pump(
            EmitWatch::new(paired.agent_watch.clone_rx()),
            queue.clone(),
            baseline,
        ));

        // 人落子 → B 收到；事件物化出 move
        f.cmd(UiCommand::Place { x: 7, y: 7 });
        assert!(
            a.wait_until(&mut |s| s["moveCount"] == json!(1), Duration::from_secs(15)),
            "黑方落子应同步到 B"
        );
        assert_eq!(f.snapshot()["board"], a.snapshot()["board"], "两端棋盘必须一致");
        // B 的聊天 → 人收到；事件物化出 chat
        a.cmd(UiCommand::SendChat("你好，请多指教".into()));
        assert!(
            f.wait_until(
                &mut |s| s["chatLog"].as_array().is_some_and(|l| l.iter().any(|m| m["text"] == json!("你好，请多指教"))),
                Duration::from_secs(15),
            ),
            "B 的聊天应送达 A'"
        );
        // 事件队列：B 视角先 move 后 chat（清单展开序），seq 连续无空洞
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while queue.pending() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let hist = queue.history();
        assert!(
            matches!(&hist[..], [GameEvent::Move { .. }, GameEvent::Chat { .. }]),
            "B 应物化出 [move, chat]，实际 {hist:?}"
        );
        assert_eq!(hist[0].seq(), 1);
        assert_eq!(hist[1].seq(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 我执白_agent邀请_甲携链入局() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // 围棋 9 路顺带验 kind/size 经链接带到对面
        let paired = pair(cfg("go", 9, SeatColor::White)).await.expect("反向配对应成功");
        // B 邀请=黑，人执白
        assert_eq!(str_of(&paired.agent, "myColor"), "black");
        assert_eq!(str_of(&paired.front, "myColor"), "white");
        assert_eq!(str_of(&paired.front, "kind"), "go");
        assert_eq!(paired.front.snapshot()["size"], json!(9));
        assert_eq!(paired.front.snapshot()["peerConnected"], Value::Bool(true));
        assert_eq!(paired.agent.snapshot()["peerConnected"], Value::Bool(true));
    }
}
