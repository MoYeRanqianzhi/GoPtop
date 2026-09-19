//! RTCPeerConnection 封装（原生）—— `webrtc-rs` 0.20。
//!
//! 与 wasm 侧 `goptop-transport/src/io/rtc.rs` **行为逐条对齐**：
//! - **双端 negotiated DataChannel（id=0）**：不依赖 `on_data_channel` 事件——
//!   该事件在部分时序下会丢失（被动端 SRD 后派发竞争），negotiated 是经典可靠解。
//! - **全量 gathering**：offer/answer 都等 ICE gathering 完成才产出（无 trickle），
//!   完了以 `Event::RtcReady` 回喂状态机。
//! - 状态变化 → `Event::PeerState`；远端 `GameMsg` → `Event::Net`。
//!
//! **API 形态差异（0.20 重构后）**：webrtc-rs 现在拆成 `rtc`（sans-IO 核心）+
//! `webrtc`（async facade），事件从「闭包回调」改成了「trait 处理器」，连接构造走
//! `PeerConnectionBuilder` 且必须显式注入 `Runtime`；DataChannel 的消息也不再是回调，
//! 而是 `poll()` 拉取 `DataChannelEvent`。行为语义没变，只是接线方式不同。

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use goptop_net::session::Event;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::runtime::{Runtime, default_runtime};

use crate::{SharedCore, bridge};

/// 进程内共享的运行时。
///
/// **必须复用**：`default_runtime()` 每次调用都新建一个——per-connection 各建一个
/// 不只浪费，还会让「同一进程不同连接」跑在互不相干的时钟/调度上。官方示例也是
/// 用 `OnceLock` 解析一次再克隆句柄。
static RUNTIME: OnceLock<Arc<dyn Runtime>> = OnceLock::new();

fn runtime() -> Arc<dyn Runtime> {
    Arc::clone(RUNTIME.get_or_init(|| default_runtime().expect("webrtc runtime-tokio 未启用")))
}

/// 对端句柄。
pub struct RtcPeer {
    pc: Arc<dyn PeerConnection>,
    dc: Arc<tokio::sync::Mutex<Option<Arc<dyn DataChannel>>>>,
    closed: std::sync::atomic::AtomicBool,
    /// ICE gathering 完成信号（offer/answer 都要等它）。
    gather: Arc<tokio::sync::Notify>,
    /// 完成标志。**不能只靠 Notify**：`notify_waiters()` 只唤醒已注册的等待者、
    /// 不存许可，若 gathering 在 wait 之前就完成，通知会丢、只能等 8s 超时。
    gather_done: Arc<std::sync::atomic::AtomicBool>,
}

impl RtcPeer {
    /// 本连接是否已被本端关闭（诊断用；对端关闭与否由 `PeerState` 事件反映）。
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn close(&self) {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let pc = self.pc.clone();
        tokio::spawn(async move {
            let _ = pc.close().await;
        });
    }
}

/// 事件处理器：webrtc-rs 的回调 → 我们的 `Event`。
///
/// 与 wasm 侧那组 `Closure` 一一对应，只是这里靠 trait 方法而非 JS 回调。
struct Handler {
    core: SharedCore,
    tag: String,
    /// ICE gathering 完成的信号。wasm 侧靠轮询 `iceGatheringState`，这边有事件回调，
    /// 用它比轮询干净（`wait_gathering` 仍留 8s 超时兜底）。
    gather: Arc<tokio::sync::Notify>,
    gather_done: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl PeerConnectionEventHandler for Handler {
    /// 连接状态 → PeerState（与 wasm 侧同一套 open/closed/failed 判定）。
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
            eprintln!("[rtc {}] conn={state:?}", self.tag);
        }
        let (opened, closed, failed) = match state {
            RTCPeerConnectionState::Connected => (true, false, false),
            RTCPeerConnectionState::Failed => (false, true, true),
            RTCPeerConnectionState::Closed => (false, true, false),
            _ => (false, false, false),
        };
        if opened || closed {
            bridge::queue(
                &self.core,
                Event::PeerState { tag: self.tag.clone(), opened, closed, failed },
            );
        }
    }

    /// ICE 候选收集完成 → 唤醒等待者（全量 gathering 模型的关键信号）。
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
            eprintln!("[rtc {}] gathering state = {state:?}", self.tag);
        }
        if state == RTCIceGatheringState::Complete {
            // 先置标志再通知：等待方先查标志，晚到的等待者不会错过
            self.gather_done.store(true, std::sync::atomic::Ordering::SeqCst);
            self.gather.notify_waiters();
        }
    }

    /// 被动侧：若对端不是 negotiated DC（配置回退），仍要接住并挂上事件循环。
    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        poll_channel(self.core.clone(), self.tag.clone(), dc);
    }
}

/// 起一个任务持续 poll 该 DataChannel 的事件（webrtc-rs 0.20 是拉模型）。
fn poll_channel(core: SharedCore, tag: String, dc: Arc<dyn DataChannel>) {
    tokio::spawn(async move {
        while let Some(ev) = dc.poll().await {
            match ev {
                DataChannelEvent::OnOpen => {
                    bridge::queue(&core, Event::PeerState { tag: tag.clone(), opened: true, closed: false, failed: false });
                }
                DataChannelEvent::OnClose => {
                    bridge::queue(&core, Event::PeerState { tag: tag.clone(), opened: false, closed: true, failed: false });
                    return;
                }
                DataChannelEvent::OnMessage(msg) => {
                    let txt = String::from_utf8_lossy(&msg.data);
                    if let Ok(m) = serde_json::from_str::<goptop_net::protocol::GameMsg>(&txt) {
                        bridge::queue(&core, Event::Net(m));
                    }
                }
                _ => {}
            }
        }
    });
}

/// 建连接。`inviter=true` 先建 DC 并生成 offer；false 侧等远端 offer 喂进来。
pub fn create(core: &SharedCore, tag: String, inviter: bool, _spectator: bool) {
    let core2 = core.clone();
    let tag2 = tag.clone();
    tokio::spawn(async move {
        match build(&core2, &tag2).await {
            Ok(peer) => {
                let pc = peer.pc.clone();
                // 同 tag 重建时先丢旧句柄 —— wasm 侧 `peers.retain(|(t, _)| *t != tag)`
                // 的对应物。缺了它，同一 tag 会同时留着新旧两条连接，而所有按 tag 的查找
                //（`send_to` / `wait_peer` / `peer_gather`）命中的都是**第一条**（可能是
                // 已经关闭的旧连接），新连接的 DC 再也不会被用到，消息静默丢进死连接。
                let stale = core2.lock().ok().and_then(|mut c| {
                    let idx = c.peers.iter().position(|(t, _)| *t == tag2)?;
                    Some(c.peers.remove(idx))
                });
                if let Some((_, old)) = stale {
                    old.close();
                }
                if let Ok(mut c) = core2.lock() {
                    c.peers.push((tag2.clone(), peer));
                }
                if inviter && make_offer(&core2, &tag2, pc).await.is_err() {
                    if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
                        eprintln!("[rtc {}] make_offer 失败", tag2);
                    }
                    bridge::queue(&core2, Event::PeerState { tag: tag2.clone(), opened: false, closed: true, failed: true });
                }
            }
            Err(e) => {
                if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
                    eprintln!("[rtc {}] build 失败: {e}", tag2);
                }
                bridge::queue(&core2, Event::PeerState { tag: tag2, opened: false, closed: true, failed: true });
            }
        }
    });
}

async fn build(core: &SharedCore, tag: &str) -> Result<RtcPeer, String> {
    let trace = std::env::var("GOPTOP_TRACE_RTC").is_ok();
    let stun = core.lock().map_err(|_| "lock".to_string())?.session.stun_urls.clone();
    if trace {
        eprintln!("[rtc {tag}] stun={stun:?}");
    }

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer { urls: stun, ..Default::default() }])
        .build();

    let gather = Arc::new(tokio::sync::Notify::new());
    let gather_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler = Arc::new(Handler {
        core: core.clone(),
        tag: tag.to_string(),
        gather: gather.clone(),
        gather_done: gather_done.clone(),
    });

    if trace { eprintln!("[rtc {tag}] builder 就绪，开始 build"); }
    let pc = Arc::new(
        PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_handler(handler)
            .with_runtime(runtime())
            .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
            .build()
            .await
            .map_err(|e| e.to_string())?,
    );

    // 双端 negotiated DC：`negotiated = Some(0)` 即「双方各自本地建、id 都是 0」，
    // 不需要 in-band 协商，也避开了 on_data_channel 事件的时序丢失。
    // （0.20 的 RTCDataChannelInit 没有独立的 `id` 字段——id 由 negotiated 决定。）
    let init = RTCDataChannelInit {
        negotiated: Some(0),
        ..Default::default()
    };
    if trace { eprintln!("[rtc {tag}] pc 建成，建 DC"); }
    let dc = pc
        .create_data_channel("goptop", Some(init))
        .await
        .map_err(|e| e.to_string())?;
    if trace { eprintln!("[rtc {tag}] DC 建成"); }
    poll_channel(core.clone(), tag.to_string(), dc.clone());

    Ok(RtcPeer {
        pc,
        dc: Arc::new(tokio::sync::Mutex::new(Some(dc))),
        closed: std::sync::atomic::AtomicBool::new(false),
        gather,
        gather_done,
    })
}

/// 主动侧：createOffer → setLocal → 等 gathering → 回 RtcReady。
async fn make_offer(core: &SharedCore, tag: &str, pc: Arc<dyn PeerConnection>) -> Result<(), String> {
    let trace = std::env::var("GOPTOP_TRACE_RTC").is_ok();
    if trace { eprintln!("[rtc {tag}] make_offer: create_offer"); }
    let offer = pc.create_offer(None).await.map_err(|e| e.to_string())?;
    if trace { eprintln!("[rtc {tag}] make_offer: set_local_description"); }
    pc.set_local_description(offer).await.map_err(|e| e.to_string())?;
    if trace { eprintln!("[rtc {tag}] make_offer: 等 gathering"); }
    if let Some((g, d)) = peer_gather(core, tag) {
        wait_gathering(&g, &d).await;
    }
    if trace { eprintln!("[rtc {tag}] make_offer: gathering 结束"); }
    let desc = pc.local_description().await.ok_or("no local description")?;
    // offer 的编码判据与 wasm 侧 CreatePeer 分支一致：服务器模式不加密（明文进信令），
    // 无服务器模式用当前局 pwd 编成 G1 密文进邀请链接。
    let key = match core.lock() {
        Ok(c) if !c.session.server_mode => c.session.pwd.clone(),
        _ => None,
    };
    let (plain, enc) = encode_payload(&desc.sdp, "offer", "player", key.as_deref());
    bridge::queue(
        core,
        Event::RtcReady {
            tag: tag.to_string(),
            offer_plain: Some(plain),
            answer_plain: None,
            offer_enc: enc,
            answer_enc: None,
        },
    );
    Ok(())
}

/// 被动侧：喂远端 offer → createAnswer → setLocal → 等 gathering → 回 RtcReady。
pub fn feed_offer(core: &SharedCore, tag: &str, offer: &str, encrypted: bool) {
    let trace = std::env::var("GOPTOP_TRACE_RTC").is_ok();
    if trace {
        eprintln!("[rtc {tag}] feed_offer 收到 offer（{} 字节）", offer.len());
    }
    // **解码与「answer 用什么钥匙」都必须在入队时刻同步取定，不能挪进 spawn**：两者都要读
    // `session.pwd` / `spec_pwd`，而 spawn 之后的 await（等 peer 建好）期间状态机可能已把
    // 它们清空——`accept_answer` 踩过同一个坑（见那里的说明）。同步解码也与 wasm 侧一致
    //（wasm 的 FeedOffer 就是在 Effect 执行时同步 `decode_sdp`）。
    let Some((sdp, typ)) = decode_payload(core, offer, encrypted) else {
        if trace {
            eprintln!("[rtc {tag}] feed_offer: 解码失败");
        }
        return;
    };
    // answer 的编码判据与 wasm 侧 FeedOffer 分支逐字对齐：**跟 offer 的 encrypted 标志走**，
    // 钥匙取 pwd 再回退 spec_pwd。不能按「pwd 是否存在」反推——无服务器观战链
    //（`accept_spec_offer_serverless`，encrypted=false）若此时残留着上一局的 pwd，
    // 明文 answer 会被编成密文，而房主按明文解（`accept_spec_receipt` 也是 encrypted=false），
    // 观战直连永远建不起来且不报错。
    let (key, role) = {
        let Ok(c) = core.lock() else { return };
        let key = if encrypted {
            c.session.pwd.clone().or_else(|| c.session.spec_pwd.clone())
        } else {
            None
        };
        // `r` 与 wasm 侧同源（观战 offer 的 answer 标 spectator）——两端载荷逐字段一致。
        let role = if c.session.role == goptop_net::session::Role::Spectator { "spectator" } else { "player" };
        (key, role)
    };
    if trace {
        eprintln!("[rtc {tag}] feed_offer: 解码出 sdp {} 字节 / type={typ}", sdp.len());
    }
    let core = core.clone();
    let tag = tag.to_string();
    tokio::spawn(async move {
        let Some(pc) = wait_peer(&core, &tag, "feed_offer").await else { return };
        let gather = peer_gather(&core, &tag);
        let r = async {
            let desc = match typ.as_str() {
                "answer" => RTCSessionDescription::answer(sdp).map_err(|e| e.to_string())?,
                _ => RTCSessionDescription::offer(sdp).map_err(|e| e.to_string())?,
            };
            if trace {
                eprintln!("[rtc {tag}] feed_offer: set_remote");
            }
            pc.set_remote_description(desc).await.map_err(|e| e.to_string())?;
            if trace {
                eprintln!("[rtc {tag}] feed_offer: create_answer");
            }
            let answer = pc.create_answer(None).await.map_err(|e| e.to_string())?;
            pc.set_local_description(answer).await.map_err(|e| e.to_string())?;
            if trace {
                eprintln!("[rtc {tag}] feed_offer: 等 gathering");
            }
            if let Some((g, d)) = &gather {
                wait_gathering(g, d).await;
            }
            if trace {
                eprintln!("[rtc {tag}] feed_offer: gathering 结束");
            }
            pc.local_description().await.ok_or_else(|| "no local description".to_string())
        }
        .await;
        if let Err(e) = &r {
            if trace {
                eprintln!("[rtc {tag}] feed_offer 失败: {e}");
            }
        }
        if let Ok(desc) = r {
            let (plain, enc) = encode_payload(&desc.sdp, "answer", role, key.as_deref());
            bridge::queue(
                &core,
                Event::RtcReady {
                    tag: tag.clone(),
                    offer_plain: None,
                    answer_plain: Some(plain),
                    offer_enc: None,
                    answer_enc: enc,
                },
            );
        }
    });
}

/// 应用远端 answer（幂等位由状态机管理；重复应用静默）。
pub fn accept_answer(core: &SharedCore, tag: &str, answer: &str, encrypted: bool, pwd: Option<String>) {
    let core = core.clone();
    let tag = tag.to_string();
    let answer = answer.to_string();
    // **解码必须同步做完，不能挪进 spawn**：解密要读 `session.pwd`，而调用方
    //（`accept_challenge_with`）在发出本 Effect 之后紧接着就把 `s.pwd` 清成 None
    //（「两人满员，钥匙失效」）。spawn 出去再解就成了异步读，拿到的是已清空的状态
    // ——实测 A 侧 `pwd=None spec_pwd=Some(6)` 解码失败，而 B 侧同一份载荷能解开。
    let Some((sdp, _typ)) = decode_payload_with(&core, &answer, encrypted, pwd.as_deref()) else {
        if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
            eprintln!("[rtc {tag}] accept_answer: 解码失败");
        }
        return;
    };
    if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
        eprintln!("[rtc {tag}] accept_answer: 解码出 {} 字节（同步完成）", sdp.len());
    }
    tokio::spawn(async move {
        let Some(pc) = wait_peer(&core, &tag, "accept_answer").await else { return };
        match RTCSessionDescription::answer(sdp) {
            Ok(desc) => {
                if let Err(e) = pc.set_remote_description(desc).await {
                    if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
                        eprintln!("[rtc {tag}] accept_answer: set_remote 失败: {e}");
                    }
                } else if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
                    eprintln!("[rtc {tag}] accept_answer: set_remote 完成，等 ICE");
                }
            }
            Err(e) => {
                if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
                    eprintln!("[rtc {tag}] accept_answer: answer 解析失败: {e}");
                }
            }
        }
    });
}

/// 等某个 tag 的 peer 登记完成（最多 10 秒）。
///
/// **必须有这个等待**：`Effect::CreatePeer` 在 native 侧是异步的（要 await
/// `PeerConnectionBuilder::build`），而 `FeedOffer`/`AcceptAnswer` 紧随其后同步到达——
/// 不等就会「找不到 peer」，表现为配对永远停在等待态。wasm 侧 `RtcPeer::new` 是
/// 同步的，所以那边没有这个竞态，两边行为在这一点上**有意不同**。
async fn wait_peer(core: &SharedCore, tag: &str, who: &str) -> Option<Arc<dyn PeerConnection>> {
    for _ in 0..200 {
        if let Some(pc) = peer_pc(core, tag) {
            return Some(pc);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
        eprintln!("[rtc {tag}] {who}: 等 peer 超时");
    }
    None
}

/// 把一条对局消息发给该 peer 的 DC（open 才发）。与 wasm 侧 `send` 对应。
pub fn send_to(core: &SharedCore, tag: &str, json: &str) {
    let dc = {
        let Ok(c) = core.lock() else { return };
        match c.peers.iter().find(|(t, _)| t == tag) {
            Some((_, p)) => p.dc.clone(),
            None => return,
        }
    };
    let json = json.to_string();
    tokio::spawn(async move {
        let guard = dc.lock().await;
        if let Some(ch) = guard.as_ref() {
            if matches!(ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
                let _ = ch.send_text(&json).await;
            }
        }
    });
}

/// 广播给全部 peer 的 DC（`Effect::Broadcast` 的 DataChannel 那一半）。
pub fn broadcast(core: &SharedCore, msg: &goptop_net::protocol::GameMsg) {
    let Ok(json) = serde_json::to_string(msg) else { return };
    let tags: Vec<String> = {
        let Ok(c) = core.lock() else { return };
        c.peers.iter().map(|(t, _)| t.clone()).collect()
    };
    for t in tags {
        send_to(core, &t, &json);
    }
}

fn peer_pc(core: &SharedCore, tag: &str) -> Option<Arc<dyn PeerConnection>> {
    let c = core.lock().ok()?;
    c.peers.iter().find(|(t, _)| t == tag).map(|(_, p)| p.pc.clone())
}

fn peer_gather(
    core: &SharedCore,
    tag: &str,
) -> Option<(Arc<tokio::sync::Notify>, Arc<std::sync::atomic::AtomicBool>)> {
    let c = core.lock().ok()?;
    c.peers
        .iter()
        .find(|(t, _)| t == tag)
        .map(|(_, p)| (p.gather.clone(), p.gather_done.clone()))
}

/// 等 ICE gathering 完成。wasm 侧是 100ms 轮询 + 8s 上限；这边有事件回调，
/// 直接等通知，超时仍留 8s 兜底（对齐 wasm 侧的量级）。
async fn wait_gathering(gather: &Arc<tokio::sync::Notify>, done: &Arc<std::sync::atomic::AtomicBool>) {
    // **先注册等待者，再复查标志**：`notify_waiters()` 只唤醒已注册的等待者、不存许可。
    // 若按「先查标志再 notified()」的顺序写，gathering 恰在这两步之间完成时通知会丢
    //（标志的 store 与 notify 的间隔里我们还没注册，之后只能干等 8s 兜底）——
    // 表现是 offer/answer 晚 8 秒才产出，对端在这期间停在等待态。
    let mut wait = std::pin::pin!(gather.notified());
    wait.as_mut().enable();
    if done.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(8), wait).await;
}

/// 把 SDP 包成载荷：`{"s":sdp,"t":type,"r":role}`；`key` 给定时再编成 G1 密文。
///
/// **与 wasm 侧 bridge 的约定逐字一致**——两端互操作全靠这个 JSON 形状与 G1 编码。
/// 少了编码这一步，无服务器模式下 `offer_enc` 会是 None，状态机据此判定
/// 「直连邀请生成失败：已生成同源链接（跨设备不可用）」，配对永远停在等待态。
///
/// 钥匙由调用点给：两端的两处调用点各自与 wasm 对应分支同规则（offer 看 `server_mode`、
/// answer 看 offer 带来的 `encrypted` 标志），不在这里用某一种判据统一反推。
fn encode_payload(sdp: &str, typ: &str, role: &str, key: Option<&str>) -> (String, Option<String>) {
    let payload = serde_json::json!({ "s": sdp, "t": typ, "r": role }).to_string();
    let enc = key.and_then(|p| goptop_net::codec::encode(&payload, p).ok());
    if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
        eprintln!("[rtc] encode_payload typ={typ} enc={:?}", enc.as_ref().map(|e| e.len()));
    }
    (payload, enc)
}

/// 还原载荷：加密的先解 G1，再取 (sdp, type)。
fn decode_payload(core: &SharedCore, raw: &str, encrypted: bool) -> Option<(String, String)> {
    decode_payload_with(core, raw, encrypted, None)
}

/// 同上，但可显式指定钥匙（`AcceptAnswer` 随 Effect 携带的那把）。
fn decode_payload_with(core: &SharedCore, raw: &str, encrypted: bool, key: Option<&str>) -> Option<(String, String)> {
    let text = if encrypted {
        let (pwd, spwd) = {
            let c = core.lock().ok()?;
            (c.session.pwd.clone(), c.session.spec_pwd.clone())
        };
        if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
            eprintln!("[rtc] decode_payload: pwd={:?} spec_pwd={:?}", pwd.as_deref().map(|p| p.len()), spwd.as_deref().map(|p| p.len()));
        }
        let t = key.map(str::to_string).or(pwd).or(spwd).unwrap_or_default();
        let r = goptop_net::codec::decode(raw, &t).ok();
        if std::env::var("GOPTOP_TRACE_RTC").is_ok() {
            eprintln!("[rtc] decode_payload: 解码{}", if r.is_some() { "成功" } else { "失败" });
        }
        r?
    } else {
        raw.to_string()
    };
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((v["s"].as_str()?.to_string(), v["t"].as_str().unwrap_or("offer").to_string()))
}
