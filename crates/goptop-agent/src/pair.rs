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
//!  （`session_new` 的 url 参数就是为这条路留的）。
//!
//! **单局互斥的前提**：进程内 BC hub 全局无局号（bc.rs 头注自证缺口），同一时刻
//! 只允许一个 Agent 局；拦截面（拒绝主会话开局类命令）在 src-tauri 的 AgentHub，
//! 不在本 crate——本 crate 只管把一局结起来、结不起来就清理干净。

use std::sync::Arc;

use goptop_transport_native::Host;

use crate::player::{EmitWatch, HookHost, NativePlayer};

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
        todo!()
    }

    /// 对面席的执色。
    #[must_use]
    pub fn opponent(self) -> Self {
        todo!()
    }
}

/// 配对参数。棋种/路数/执色以用户 UI 配置为权威（MCP 模式下外部 Agent 的
/// `game_start` 无权改——它只有「认领席位」的份）。
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
    todo!()
}
