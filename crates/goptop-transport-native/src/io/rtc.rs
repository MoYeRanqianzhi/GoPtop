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
}

impl RtcPeer {
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
}

#[async_trait]
impl PeerConnectionEventHandler for Handler {
    /// 连接状态 → PeerState（与 wasm 侧同一套 open/closed/failed 判定）。
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
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
        if state == RTCIceGatheringState::Complete {
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
                let dc_slot = peer.dc.clone();
                let pc = peer.pc.clone();
                if let Ok(mut c) = core2.lock() {
                    c.peers.push((tag2.clone(), peer));
                }
                if inviter && make_offer(&core2, &tag2, pc).await.is_err() {
                    bridge::queue(&core2, Event::PeerState { tag: tag2.clone(), opened: false, closed: true, failed: true });
                }
                let _ = dc_slot;
            }
            Err(_) => {
                bridge::queue(&core2, Event::PeerState { tag: tag2, opened: false, closed: true, failed: true });
            }
        }
    });
}

async fn build(core: &SharedCore, tag: &str) -> Result<RtcPeer, String> {
    let stun = core.lock().map_err(|_| "lock".to_string())?.session.stun_urls.clone();

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer { urls: stun, ..Default::default() }])
        .build();

    let gather = Arc::new(tokio::sync::Notify::new());
    let handler = Arc::new(Handler {
        core: core.clone(),
        tag: tag.to_string(),
        gather: gather.clone(),
    });

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
    let dc = pc
        .create_data_channel("goptop", Some(init))
        .await
        .map_err(|e| e.to_string())?;
    poll_channel(core.clone(), tag.to_string(), dc.clone());

    Ok(RtcPeer {
        pc,
        dc: Arc::new(tokio::sync::Mutex::new(Some(dc))),
        closed: std::sync::atomic::AtomicBool::new(false),
        gather,
    })
}

/// 主动侧：createOffer → setLocal → 等 gathering → 回 RtcReady。
async fn make_offer(core: &SharedCore, tag: &str, pc: Arc<dyn PeerConnection>) -> Result<(), String> {
    let offer = pc.create_offer(None).await.map_err(|e| e.to_string())?;
    pc.set_local_description(offer).await.map_err(|e| e.to_string())?;
    if let Some(g) = peer_gather(core, tag) {
        wait_gathering(&g).await;
    }
    let desc = pc.local_description().await.ok_or("no local description")?;
    bridge::queue(
        core,
        Event::RtcReady {
            tag: tag.to_string(),
            offer_plain: Some(desc.sdp.clone()),
            answer_plain: None,
            offer_enc: None,
            answer_enc: None,
        },
    );
    Ok(())
}

/// 被动侧：喂远端 offer → createAnswer → setLocal → 等 gathering → 回 RtcReady。
pub fn feed_offer(core: &SharedCore, tag: &str, offer: &str, _encrypted: bool) {
    let core = core.clone();
    let tag = tag.to_string();
    let offer = offer.to_string();
    tokio::spawn(async move {
        let Some(pc) = peer_pc(&core, &tag) else { return };
        let gather = peer_gather(&core, &tag);
        let r = async {
            let desc = RTCSessionDescription::offer(offer).map_err(|e| e.to_string())?;
            pc.set_remote_description(desc).await.map_err(|e| e.to_string())?;
            let answer = pc.create_answer(None).await.map_err(|e| e.to_string())?;
            pc.set_local_description(answer).await.map_err(|e| e.to_string())?;
            if let Some(g) = &gather {
                wait_gathering(g).await;
            }
            pc.local_description().await.ok_or_else(|| "no local description".to_string())
        }
        .await;
        if let Ok(desc) = r {
            bridge::queue(
                &core,
                Event::RtcReady {
                    tag: tag.clone(),
                    offer_plain: None,
                    answer_plain: Some(desc.sdp.clone()),
                    offer_enc: None,
                    answer_enc: None,
                },
            );
        }
    });
}

/// 应用远端 answer（幂等位由状态机管理；重复应用静默）。
pub fn accept_answer(core: &SharedCore, tag: &str, answer: &str, _encrypted: bool) {
    let Some(pc) = peer_pc(core, tag) else { return };
    let answer = answer.to_string();
    tokio::spawn(async move {
        if let Ok(desc) = RTCSessionDescription::answer(answer) {
            let _ = pc.set_remote_description(desc).await;
        }
    });
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

fn peer_gather(core: &SharedCore, tag: &str) -> Option<Arc<tokio::sync::Notify>> {
    let c = core.lock().ok()?;
    c.peers.iter().find(|(t, _)| t == tag).map(|(_, p)| p.gather.clone())
}

/// 等 ICE gathering 完成。wasm 侧是 100ms 轮询 + 8s 上限；这边有事件回调，
/// 直接等通知，超时仍留 8s 兜底（对齐 wasm 侧的量级）。
async fn wait_gathering(gather: &Arc<tokio::sync::Notify>) {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(8), gather.notified()).await;
}
