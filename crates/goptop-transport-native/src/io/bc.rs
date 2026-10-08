//! 同源通道（原生）—— 进程内广播。
//!
//! wasm 侧的 BroadcastChannel 解决的是「同一浏览器里两个标签页互通」；native
//! 单实例下没有这个场景，但 `Effect::Broadcast` / `SendPresence` 仍需要一个分发点，
//! 所以这里用 `tokio::sync::broadcast` 做进程内广播：订阅者可以是未来的第二个窗口，
//! 也可以什么都不是（没人订阅时发送是无害的）。
//!
//! **行为差异记录**：wasm 端 BC 的 announce 语义在本层不成立（没有「另一个页面」
//! 可宣告），因此 presence 的跨实例发现能力在 native 端目前是空的。这不影响
//! 服务器模式与 P2P 直连（那两条路走 WS 与 RTC），只影响「同机双窗口自动发现」。
//!
//! 还有一处**尚未收敛**的差异：wasm 的对局通道是**按局名**分的
//!（`goptop-game-{gameId}`，跨局天然隔离），本层只有一个全局 hub，而 `GameMsg` 里
//! 也没有局号可过滤——同进程的两个窗口若处在不同局，一方广播的对局消息会被另一方
//! 的 presence 分类器收下（`Event::Net`）并送进它自己的状态机。要修得把通道名做成
//! hub 的 topic（`JoinChannel` 收到的 `name` 正是它），且要保证两端此时已切到同一
//! 个局名，属于跨层改动，不在本层单独收敛。

use std::sync::Arc;

use std::sync::OnceLock;

use tokio::sync::broadcast;

use crate::SharedCore;

/// 广播载荷：`(主题, 发送者 userId, 文本)`。
///
/// **必须带发送者**：浏览器的 `BroadcastChannel` 不回送给发送者自己，状态机因此
/// 从不自过滤。进程内广播若原样回送，发送端会把自己的消息再处理一遍、又发一次，
/// 形成回路——实测表现为「A 同时发出 accept 与 reject」，对端刚进对局就被踢回主页。
///
/// **必须带主题**（用户拍板 2026-10-08 重大 bug 的根治）：旧实现全局单频道，
/// 旧局的离场/认输广播会被同进程**新开的对局会话**当成自己的对局消息（空盘
/// 「黑胜」直接终局）——正是 bc.rs 顶部记录过的跨局串扰缺口。主题=presence
/// 常驻频道或 `goptop-game-{gameId}` 对局频道，投递按会话的主题允许集过滤
/// （与 wasm 侧 per-game BroadcastChannel 同构）。
type Payload = (String, String, String);

/// presence 常驻主题（announce/challenge/peers——全大厅共享）。
pub const PRESENCE_TOPIC: &str = "goptop-presence-v1";

/// 对局主题名（与 wasm 侧 `goptop-game-{gameId}` 频道名同构）。
fn game_topic(game_id: &str) -> String {
    format!("goptop-game-{game_id}")
}

/// **进程内共享**的广播通道。
///
/// 关键：不能每个 `Core` 各建一个——那样同进程里的两个会话（同机双窗口、无头测试的
/// 两端）永远收不到对方的消息，而 wasm 侧 BroadcastChannel 恰恰是同源页面互通的。
/// 用一个全局 channel 才能对等实现「同源广播」的语义。跨局隔离靠主题过滤（见上）。
fn hub() -> &'static broadcast::Sender<Payload> {
    static HUB: OnceLock<broadcast::Sender<Payload>> = OnceLock::new();
    HUB.get_or_init(|| broadcast::channel::<Payload>(256).0)
}

/// 进程内广播通道句柄。
pub struct Bc {
    /// 本句柄的发送主题（presence 句柄=常驻主题）。
    name: String,
    /// 本会话的 userId：发送时带上，供订阅端过滤自己。
    me: String,
}

impl Bc {
    pub fn send(&self, v: serde_json::Value) {
        // 没有订阅者时 send 返回 Err——不是错误，忽略
        if std::env::var("GOPTOP_TRACE_BC").is_ok() {
            eprintln!("[bc {} send {}] {}", self.name, self.me, v.to_string().chars().take(160).collect::<String>());
        }
        let _ = hub().send((self.name.clone(), self.me.clone(), v.to_string()));
    }

    /// 关闭通道：**当前无调用点**（`bridge` 的 `LeaveChannel` 有意不再撤订阅，见那里的
    /// 说明），保留是为了与 wasm 侧 `Bc::close` 的调用面一致。全局 hub 也不因单个会话
    /// 关闭而销毁（其他会话还在用），订阅任务只在自己的 `rx` 失效时退出。
    pub fn close(&self) {
        // 保留方法（见上），无实际动作。
    }

    /// 克隆一个发送端句柄（`Effect::Broadcast` 要在不持 Core 锁的情况下发）。
    pub fn clone_handle(&self) -> BcHandle {
        BcHandle { name: self.name.clone(), me: self.me.clone() }
    }
}

/// 脱离 Core 生命周期的发送句柄。
#[derive(Clone)]
pub struct BcHandle {
    name: String,
    me: String,
}

impl BcHandle {
    pub fn send(&self, v: serde_json::Value) {
        let _ = hub().send((self.name.clone(), self.me.clone(), v.to_string()));
    }
}

/// 加入通道：订阅全局 hub，把收到的文本交给 presence 分类器。
///
/// **必须幂等**：`Effect::JoinChannel` 在对局开始/重开/受邀入局时都会触发，
/// 每次调用都挂一个新订阅者的话，同一条广播会被处理多遍——实测表现为
/// 「challenge 被受理 4 次」，而受理之后残留的重复处理又发 reject，
/// 把刚进对局的受邀者踢回主页。
///
/// `name` = 本会话的 presence 主题（常驻收信集的第一员）。对局主题经
/// [`join_topic`] 在 `Effect::JoinChannel` 时追加——**只扩允许集，不新挂订阅者**：
/// 订阅必须幂等（见下），主题集的增减天然幂等。
pub fn join(core: &SharedCore, name: &str) {
    // 订阅拿到手就先占位，**登记与检查必须在同一把锁里**：分两步（先查、出锁、再登记）
    // 的话，两个并发泵（`cmd` 的同步泵与 50ms 后台泵同时跑）会各自通过检查、各挂一个
    // 订阅者，同一条广播被处理两遍——正是上面「challenge 被受理 4 次」那类故障。
    // 提前返回时多订的那个 rx 立即析构，无害。
    let mut rx = hub().subscribe();
    let (me, stop) = {
        let Ok(mut c) = core.lock() else { return };
        if c.presence.is_some() {
            // 重复 join：只补主题（JoinChannel 与 bind 并发到达时允许集不丢项）。
            if let Ok(mut t) = c.bc_topics.lock() {
                t.insert(name.to_string());
            }
            return;
        }
        let me = c.session.user_id.clone();
        c.presence = Some(Bc { name: name.to_string(), me: me.clone() });
        if let Ok(mut t) = c.bc_topics.lock() {
            t.insert(name.to_string());
        }
        (me, c.stop.clone())
    };

    let sub = core.clone();
    tokio::spawn(async move {
        // 停机轮询（与后台泵同款 50ms）：旧退出条件——`recv()` 返回 Closed——在这
        // 进程里**永远等不到**，hub 是 `'static` 的 `OnceLock` sender（见上），永不
        // drop。不查标志的话，会话释放后本任务抱着 `Arc<Core>`（连带 RTC 连接与
        // socket）永生，每切一次页面漏一份。
        let mut halt = tokio::time::interval(std::time::Duration::from_millis(50));
        loop {
            let got = tokio::select! {
                _ = halt.tick() => {
                    if stop.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }
                got = rx.recv() => got,
            };
            match got {
                Ok((topic, from, text)) => {
                    // 不回送发送者自己——对齐 BroadcastChannel 语义（见 Payload 的说明）；
                    // 主题不在允许集 → 别的局的流量，整条忽略（跨局串扰的根治点）。
                    if from == me {
                        continue;
                    }
                    let allowed = sub
                        .lock()
                        .ok()
                        .and_then(|c| c.bc_topics.lock().ok().map(|t| t.contains(&topic)))
                        .unwrap_or(false);
                    if !allowed {
                        continue;
                    }
                    presence::on_presence_text(&sub, &me, &text);
                }
                // Lagged 只是落后容量丢了几条，receiver **仍在订阅**、可继续 recv。
                // 当终结处理的话，一次 256 条积压（快速连发/调度延迟）就永久静默这条
                // 会话的 BC 链路——presence 与对局消息三路之一，且无任何自愈与提示。
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    if std::env::var("GOPTOP_TRACE_BC").is_ok() {
                        eprintln!("[bc {me}] 落后 {n} 条已丢弃，继续消费");
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// 启动 presence（与 wasm 侧 `start_presence` 对应）。
pub fn start_presence(core: &Arc<std::sync::Mutex<crate::Core>>) {
    join(core, PRESENCE_TOPIC);
}

/// 追加收信主题（`Effect::JoinChannel` 的对局频道）——只扩允许集，不新挂订阅
///（订阅幂等的理由见 [`join`]）。对局两端的 gameId 在握手后一致，主题一致。
///
/// `name` 是状态机给的**裸 gameId**，这里拼上 `goptop-game-` 前缀——与 wasm 侧
/// `bridge`（`format!("goptop-game-{gid}")`）同构：transport 层负责频道名的
/// 组装，[`send_game`] 发的主题同样经 [`game_topic`]，两侧必须逐字一致，
/// 否则允许集过滤会把对局消息全部静默丢弃（实测：BC 主题化第一版漏拼前缀，
/// 发送正常、接收零投递，headless 三用例超时/断言失败）。
pub fn join_topic(core: &SharedCore, name: &str) {
    let topic = game_topic(name);
    if let Ok(c) = core.lock() {
        if let Ok(mut t) = c.bc_topics.lock() {
            t.insert(topic);
        }
    }
}

/// 对局消息发送（`Effect::Broadcast` 的 BC 腿）：主题=本局 gameId 频道。
/// 不在对局（无 gameId）→ 静默丢弃——对局消息只该在对局主题上飞。
pub fn send_game(core: &SharedCore, v: serde_json::Value) {
    let Ok(c) = core.lock() else { return };
    let Some(game_id) = c.session.game_id.as_deref().filter(|g| !g.is_empty()) else {
        if std::env::var("GOPTOP_TRACE_BC").is_ok() {
            eprintln!("[bc send_game] 丢弃：无 gameId");
        }
        return;
    };
    let topic = game_topic(game_id);
    let me = c.session.user_id.clone();
    drop(c);
    if std::env::var("GOPTOP_TRACE_BC").is_ok() {
        eprintln!("[bc {topic} send {me}] {}", v.to_string().chars().take(160).collect::<String>());
    }
    let _ = hub().send((topic, me, v.to_string()));
}

/// presence 文本 → 事件（与 wasm 侧 `io/mod.rs::start_presence` 的分类逐条对应）。
pub mod presence {
    use goptop_net::session::{Event, PresenceEvt};

    use crate::{SharedCore, bridge};

    fn sv(v: &serde_json::Value, k: &str) -> String {
        v[k].as_str().unwrap_or("").to_string()
    }

    pub fn on_presence_text(core: &SharedCore, me: &str, text: &str) {
        if std::env::var("GOPTOP_TRACE_BC").is_ok() {
            eprintln!("[bc recv me={me}] {}", text.chars().take(160).collect::<String>());
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };

        // **先分对局消息**：广播通道送两类东西——presence（带 `t`）与对局 GameMsg
        //（带 `kind`/`sender`/`seq`）。只认 `t` 的话，GameMsg 会被整条丢掉，
        // 表现为「对手的连接是通的、广播也发出去了，但这边的棋盘不动」
        //（实测：`on_net` 从未被调用，因为消息在 presence 分类器里就被忽略了）。
        if v["kind"].is_object() && v["sender"].is_string() {
            if let Ok(msg) = serde_json::from_value::<goptop_net::protocol::GameMsg>(v) {
                bridge::queue(core, Event::Net(msg));
            }
            return;
        }

        let t = v["t"].as_str().unwrap_or("");
        match t {
            "announce" => {
                // 与 wasm 侧同款简化：收到的 announce 仅当存活信号，下发空名册
                bridge::queue(core, Event::Presence(PresenceEvt::Peers { peers: Vec::new() }));
            }
            "challenge" if v["to"].as_str() == Some(me) => {
                bridge::queue(
                    core,
                    Event::Presence(PresenceEvt::Challenge {
                        from: sv(&v, "from"),
                        from_name: sv(&v, "fromName"),
                        pwd: v["pwd"].as_str().map(str::to_string),
                        kind: sv(&v, "kind"),
                        size: v["size"].as_u64().unwrap_or(15) as u16,
                        game_id: sv(&v, "gameId"),
                        rtc_ans: v["rtcAns"].as_str().map(str::to_string),
                    }),
                );
            }
            "accept" if v["to"].as_str() == Some(me) => {
                bridge::queue(core, Event::Presence(PresenceEvt::Accept { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
            }
            "reject" if v["to"].as_str() == Some(me) => {
                bridge::queue(core, Event::Presence(PresenceEvt::Reject { from: sv(&v, "from"), game_id: sv(&v, "gameId") }));
            }
            _ => {}
        }
    }
}

/// 生命周期与容错的回归（Arc 计数只有 crate 内够得到，故放这里而非 tests/）。
#[cfg(test)]
mod tests {
    use super::*;

    use goptop_net::session::{Event, PresenceEvt};

    use crate::test_support;

    /// 订阅任务必须随会话停机退出并交还 Core。
    ///
    /// 修复前任务唯一的退出条件（`recv()` 返回 Closed）在这进程里永远不成立——
    /// hub 的 sender 是 `'static` 的——`Arc<Core>` 永不回落，每释放一个会话漏一份。
    #[tokio::test]
    async fn 订阅任务随会话停机退出() {
        let core = test_support::core("u-halt");
        join(&core, "goptop-presence-v1");
        assert_eq!(Arc::strong_count(&core), 2, "会话侧 + 订阅任务各持一份");
        let stop = core.lock().unwrap().stop.clone();
        let weak = Arc::downgrade(&core);
        drop(core); // 模拟 NativeSession 被释放（Drop 置位同一个 stop）
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // 停机轮询 50ms 一拍，给足几拍
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            if weak.upgrade().is_none() {
                break;
            }
        }
        assert!(weak.upgrade().is_none(), "停机置位后订阅任务必须退出并交还 Core");
    }

    /// Lagged 不是终结：落后容量后继续消费，challenge 仍能进队列。
    ///
    /// 修复前 `while let Ok` 把首个 Lagged 当 Closed，订阅任务当场死亡；而
    /// `join()` 因 presence 已挂不会重订，这条 BC 链路永久失聪且无提示。
    #[tokio::test]
    async fn lagged后订阅继续_不再失聪() {
        let core = test_support::core("u-lag");
        join(&core, "goptop-presence-v1");
        // 让订阅任务先跑起来、停在 recv 上（current_thread 下 spawn 的任务只在
        // 本任务 await 时才被调度）
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        // 同步连发 260 条噪声（hub 容量 256，必然制造 Lagged）。噪声既无 `t`
        // 也无 `kind`/`sender`，分类器整条忽略，不会污染队列。
        for i in 0..260u64 {
            let _ = hub().send((crate::io::bc::PRESENCE_TOPIC.to_string(), "other".into(), i.to_string()));
        }
        // 给订阅任务时间消化积压（经历 Lagged）
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // 积压之后再补一条指向本会话的 challenge：任务若已死，这里 send 的
        // 返回值仍是 Ok（receiver 名义上还在），只有队列能作证。
        let challenge = serde_json::json!({
            "t": "challenge", "to": "u-lag", "from": "other", "fromName": "对手",
            "kind": "gomoku", "size": 15, "gameId": "g-lag",
        });
        hub().send((crate::io::bc::PRESENCE_TOPIC.to_string(), "other".into(), challenge.to_string())).expect("订阅任务应仍在订阅");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let got = core.lock().unwrap().queue.iter().any(|ev| {
            matches!(ev, Event::Presence(PresenceEvt::Challenge { .. }))
        });
        assert!(got, "Lagged 之后订阅必须继续：challenge 应进队列");
    }

    /// 跨局隔离：别的局（主题不在允许集）的对局消息不得进本会话；本局主题
    /// 加入后才收。这是 2026-10-08「空盘黑胜」串扰事故的回归锁——旧全局
    /// hub 下任何一局广播全体会话都收，新局把旧局消息当成自己的对局消息。
    ///
    /// 顺带锁 `join_topic` 的**前缀拼装**：状态机给的是裸 gameId，允许集里存的
    /// 必须是与 `send_game` 发送侧逐字一致的 `goptop-game-{gid}`（第一版漏拼
    /// 前缀，发送正常、接收零投递）。
    #[tokio::test]
    async fn 对局消息按主题隔离_加入本局主题后才收() {
        let core = test_support::core("u-iso");
        join(&core, "goptop-presence-v1");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let game_msg = serde_json::json!({
            "kind": { "type": "Chat", "text": "hi" },
            "sender": "p-other", "seq": 1u32, "userId": "u-other",
        })
        .to_string();

        // 别人的局的广播：主题不在允许集，必须整条丢弃（队列空）。
        hub().send(("goptop-game-g-other".into(), "u-other".into(), game_msg.clone()))
            .expect("发送应有订阅者");
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let leaked = core.lock().unwrap().queue.iter().any(|ev| matches!(ev, Event::Net(_)));
        assert!(!leaked, "别局的对局消息不得进本会话（跨局串扰的根治点）");

        // 加入本局主题（裸 gameId，与状态机 JoinChannel 的载荷一致）后再发：应进队列。
        join_topic(&core, "g-mine");
        hub().send(("goptop-game-g-mine".into(), "u-other".into(), game_msg))
            .expect("发送应有订阅者");
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let got = core.lock().unwrap().queue.iter().any(|ev| matches!(ev, Event::Net(_)));
        assert!(got, "本局主题的对局消息必须进队列——join_topic 漏拼前缀时此处失败");
    }
}
