//! 无头验证 —— **界面被完全移除**，只靠 `HeadlessHost` 提供的五个平台动作
//! （存储/剪贴板/提示/导航/推送）驱动全部业务逻辑。
//!
//! 这是「一切功能皆 Rust」的直接证明：如果任何一环逻辑还留在前端，这些测试就
//! 跑不通——因为这里根本没有前端。
//!
//! 两端跑在**同一进程**里，经进程内广播互通（对应 wasm 侧同源页面的
//! BroadcastChannel）。广播是异步的（订阅任务在 tokio 上跑），所以每步之后
//! 要 pump 并给传播留一点时间。

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use goptop_net::session::UiCommand;
use goptop_transport_native::{HeadlessHost, Host, NativeSession, SessionConfig};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

/// 进程内广播 hub 是**全局**的（真实场景一个进程只有一个 app 实例），
/// 所以并行跑的用例会互相收到对方的消息。用一把全局锁把用例串起来——
/// 这是测试隔离的需要，不是产品缺陷。
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 无 STUN 的宿主：本地候选即刻收集完，省掉 gathering 的 8s 超时兜底。
fn host() -> Arc<HeadlessHost> {
    let h = Arc::new(HeadlessHost::default());
    h.storage_set("goptop:stun", Some("[]"));
    h
}

fn cfg(name: &str) -> SessionConfig {
    SessionConfig {
        name: name.to_string(),
        // 无服务器模式：不连 WS，纯走同源广播 + 直连，最适合无头验证
        server_mode: false,
        share_origin: "https://goptop.pages.dev".to_string(),
        kind: "gomoku".to_string(),
        size: 15,
    }
}

fn snap(s: &NativeSession) -> Value {
    serde_json::from_str(&s.snapshot()).expect("snapshot 必须是合法 JSON")
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

/// 泵两端并给广播传播留时间。
async fn settle(a: &NativeSession, b: &NativeSession) {
    for _ in 0..60 {
        a.pump();
        b.pump();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    a.pump();
    b.pump();
}

/// 泵到谓词成立（或超时）。**用条件等待而非固定轮数**——native 的 ICE gathering
/// 耗时随环境波动（无 STUN 时也可能走到超时兜底），死等固定时长必然 flaky。
async fn pump_until(
    a: &NativeSession,
    b: &NativeSession,
    pred: impl Fn() -> bool,
    secs: u64,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        a.pump();
        b.pump();
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    a.pump();
    b.pump();
    pred()
}

/// 建一对已配对的会话：a 邀请（等 offer 就绪），b **打开邀请链接**入局。
///
/// b 走 Boot 路径（真实用户就是点链接进来的）。不用 AcceptInvite 粘贴路径：
/// 它在「无服务器 + 链接带 rtc」时不发 `CreatePeer`，那条路径浏览器端同样走不通，
/// 是既有实现的空档（lobby.rs 的注释也写明 Boot 路径因 process_intent 提前分流
/// 才没暴露它），不在本次范围。
async fn paired() -> (NativeSession, NativeSession, Arc<HeadlessHost>, Arc<HeadlessHost>) {
    let ha = host();
    let hb = host();
    let a = NativeSession::new(cfg("甲"), ha.clone(), "http://localhost/p2p");

    a.cmd(UiCommand::CreateInvite);
    // 等 offer 编进链接：出现 rtc= 参数即就绪（同源链接一开始就有，但那时不含 rtc）
    let ok = pump_until(&a, &a, || {
        snap(&a)["inviteUrl"].as_str().is_some_and(|u| u.contains("rtc="))
    }, 40)
    .await;
    assert!(ok, "40 秒内未生成含 rtc 的邀请链接");
    let sa = snap(&a);
    assert_eq!(s(&sa, "phase"), "waiting", "邀请后应进等待态");

    // b 打开链接入局
    let link = s(&sa, "inviteUrl");
    let b = NativeSession::new(cfg("乙"), hb.clone(), &link);

    let ok = pump_until(
        &a,
        &b,
        || s(&snap(&a), "phase") == "playing" && s(&snap(&b), "phase") == "playing",
        40,
    )
    .await;
    if !ok {
        println!("[B notices] {:?}", hb.notices.lock().unwrap());
        println!("[B navs] {:?}", hb.navs.lock().unwrap());
        println!("[B notices] {:?}", hb.notices.lock().unwrap());
        println!("[A notices] {:?}", ha.notices.lock().unwrap());
        println!("[B role={} myColor={} peerConnected={}]", s(&snap(&b), "role"), s(&snap(&b), "myColor"), snap(&b)["peerConnected"]);
    }
    assert!(ok, "40 秒内未双双进入对局态：A={} B={}", s(&snap(&a), "phase"), s(&snap(&b), "phase"));
    (a, b, ha, hb)
}

/// 双人对局全流程：邀请 → 配对 → 落子同步 → 五连终局。
#[tokio::test]
async fn 双人对局全流程() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (a, b, ha, hb) = paired().await;

    let sa = snap(&a);
    let sb = snap(&b);
    assert_eq!(s(&sa, "phase"), "playing", "邀请者应进对局态");
    assert_eq!(s(&sb, "phase"), "playing", "受邀者应进对局态");
    assert_eq!(s(&sa, "role"), "inviter");
    assert_eq!(s(&sb, "role"), "invitee");
    // 双方执色必须对立
    let (ca, cb) = (s(&sa, "myColor"), s(&sb, "myColor"));
    assert_ne!(ca, cb, "双方执色不能相同");
    assert!(ca == "black" || ca == "white", "执色必须是 black/white，实际 {ca}");

    // 等数据通道打开（peerConnected）——在此之前落子会被 can_place 静默拒绝
    pump_until(&a, &b, || snap(&a)["peerConnected"] == Value::Bool(true) && snap(&b)["peerConnected"] == Value::Bool(true), 40).await;

    // 黑方连下五手，白方垫手 —— 全程由 Rust 规则引擎判定
    let (black, white) = if ca == "black" { (&a, &b) } else { (&b, &a) };
    for x in 0..4u8 {
        black.cmd(UiCommand::Place { x: x as u16, y: 0 });
        settle(&a, &b).await;
        white.cmd(UiCommand::Place { x: x as u16, y: 7 });
        settle(&a, &b).await;
    }
    black.cmd(UiCommand::Place { x: 4, y: 0 });
    settle(&a, &b).await;

    let sa = snap(&a);
    let sb = snap(&b);
    println!("[落子后] A.mc={:?} B.mc={:?} A.phase={} B.phase={} A.role={} B.role={} B.uid={} A.uid={}", sa["moveCount"], sb["moveCount"], sa["phase"], sb["phase"], sa["role"], sb["role"], sb["userId"], sa["userId"]);
    assert_eq!(sa["moveCount"].as_u64(), Some(9), "手数应为 9");
    assert_ne!(sa["winner"], Value::Null, "五连后应有胜者");
    // 胜负必须两端一致（观战者判错方向是历史 bug）
    assert_eq!(sa["winner"], sb["winner"], "两端胜者判定必须一致");
    assert_eq!(sa["board"], sb["board"], "两端棋盘必须一致");
    assert!(ha.emits.load(std::sync::atomic::Ordering::Relaxed) > 0, "应有状态推送给 UI");
    let _ = hb;
}

/// 围棋：配对后的落子同步与提子（本地对局的规则判定由 goptop-core 的测试覆盖，
/// 这里验的是**经 transport 同步**这一层）。
#[tokio::test]
async fn 围棋落子同步() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let ha = host();
    let hb = host();
    let mut ca = cfg("甲");
    ca.kind = "go".into();
    ca.size = 9;
    let mut cb = cfg("乙");
    cb.kind = "go".into();
    cb.size = 9;
    let a = NativeSession::new(ca, ha.clone(), "http://localhost/p2p");

    a.cmd(UiCommand::CreateInvite);
    pump_until(&a, &a, || snap(&a)["inviteUrl"].as_str().is_some_and(|u| u.contains("rtc=")), 40).await;
    let sa = snap(&a);
    // 走 Boot 路径（打开邀请链接）——与双人对局一致，也是真实用户路径
    let link = s(&sa, "inviteUrl");
    let b = NativeSession::new(cb, hb.clone(), &link);
    let ok = pump_until(
        &a,
        &b,
        || s(&snap(&a), "phase") == "playing" && s(&snap(&b), "phase") == "playing",
        40,
    )
    .await;
    assert!(ok, "围棋局未进入对局态");
    pump_until(&a, &b, || snap(&a)["peerConnected"] == Value::Bool(true) && snap(&b)["peerConnected"] == Value::Bool(true), 40).await;

    let sa = snap(&a);
    let sb = snap(&b);
    assert_eq!(sa["kind"], "go");
    assert_eq!(sa["size"], 9);
    assert_ne!(s(&sa, "myColor"), s(&sb, "myColor"), "双方执色必须对立");

    // 黑下 (1,0)，看是否同步到对端（围棋的提子/轮转都由 Rust 规则引擎判定）
    let (black, white) = if s(&sa, "myColor") == "black" { (&a, &b) } else { (&b, &a) };
    black.cmd(UiCommand::Place { x: 1, y: 0 });
    pump_until(&a, &b, || snap(&a)["moveCount"].as_u64() == Some(1) && snap(&b)["moveCount"].as_u64() == Some(1), 15).await;
    let sa = snap(&a);
    let sb = snap(&b);
    assert_eq!(sa["board"], sb["board"], "落子必须同步到对端");
    assert_eq!(sa["moveCount"].as_u64(), Some(1));
    let _ = white;
}

/// 聊天与协商（悔棋/重开）经同源通道送达对端。
#[tokio::test]
async fn 聊天与协商() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (a, b, _ha, _hb) = paired().await;

    a.cmd(UiCommand::SendChat("你好".into()));
    settle(&a, &b).await;
    let sb = snap(&b);
    let log = sb["chatLog"].as_array().cloned().unwrap_or_default();
    assert!(
        log.iter().any(|m| m["text"].as_str() == Some("你好")),
        "聊天应送达对端，实际 chatLog={log:?}"
    );

    // 协商：a 请求悔棋 → b 端应出现待确认
    a.cmd(UiCommand::Place { x: 7, y: 7 });
    settle(&a, &b).await;
    a.cmd(UiCommand::RequestUndo);
    settle(&a, &b).await;
    let sb = snap(&b);
    assert_ne!(sb["confirmReq"], Value::Null, "对端应收到悔棋确认请求");
    b.cmd(UiCommand::ConfirmApprove);
    settle(&a, &b).await;
    let sa = snap(&a);
    assert_eq!(sa["moveCount"].as_u64(), Some(0), "同意悔棋后手数应归零");
}

/// 设置类键值经宿主存储落盘（无界面下就是 `HeadlessHost` 的内存表）。
#[tokio::test]
async fn 设置落盘走宿主存储() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let ha = host();
    let a = NativeSession::new(cfg("甲"), ha.clone(), "http://localhost/settings");
    a.cmd(UiCommand::SetName("新名字".into()));
    settle(&a, &a).await;

    let stored = ha
        .storage
        .lock()
        .unwrap()
        .get("goptop:name")
        .cloned()
        .unwrap_or_default();
    assert_eq!(stored, "新名字", "改名应落宿主存储");
    // 身份也应持久化（native 是每设备一身份）
    assert!(
        ha.storage.lock().unwrap().contains_key("goptop:userId"),
        "userId 应落盘"
    );
    assert_eq!(s(&snap(&a), "name"), "新名字");
}

/// 链接解析与回执解析（粘贴弹窗背后的逻辑）也全在 Rust。
#[tokio::test]
async fn 链接解析在_Rust() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let ha = host();
    let a = NativeSession::new(cfg("甲"), ha, "http://localhost/");

    let ok: Value = serde_json::from_str(&a.parse_link("https://goptop.pages.dev/u-abc123?pwd=key&kind=go&size=9"))
        .expect("解析结果应为 JSON");
    assert_eq!(ok["ok"], Value::Bool(true), "合法邀请链接应被识别");
    assert_eq!(s(&ok["intent"], "mode"), "user");
    assert_eq!(s(&ok["intent"], "userId"), "u-abc123");
    assert_eq!(ok["intent"]["kind"], "go");
    assert_eq!(ok["intent"]["size"], 9);

    let bad: Value = serde_json::from_str(&a.parse_link("这不是链接")).expect("应为 JSON");
    assert_eq!(bad["ok"], Value::Bool(false), "非链接文本应被拒绝");
}

/// 服务器模式生命周期：hello 上行、welcome/peers 下行消化，以及 **drop 后不再重连**。
///
/// 无头用例此前全是 server_mode:false——`ws::run` 的重连循环与停机判定没有任何
/// Rust 级覆盖。旧退出条件 `rx.is_closed()` 在 server_mode 下**永远为假**（tx 就在
/// 被任务自己抱着的 Core 里），会话释放后它会每 8s 重新敲一次门。回归点：桩端
/// 绝不能在 drop 之后收到第二次连接（RECONNECT_MS=8s，盯 11s 够它现形）。
#[tokio::test]
async fn 服务器模式_drop后停止重连() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    // 本地 WS 桩：接受连接 → 收到 hello 就回 welcome + peers → 1s 后主动断开
    //（触发客户端的 8s 重连节奏），并**持续接受**后续连接以捕捉不该发生的重连。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑本地端口");
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let hellos: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let accepted = accepted.clone();
        let hellos = hellos.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accepted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let hellos = hellos.clone();
                tokio::spawn(async move {
                    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else { return };
                    let (mut sink, mut stream) = ws.split();
                    // 客户端连上后第一条 Text 必是 hello；答完下行、留 1s 消化，
                    // 就断开去触发它的重连路径
                    while let Some(Ok(msg)) = stream.next().await {
                        let Message::Text(txt) = msg else { continue };
                        hellos.lock().unwrap().push(txt.to_string());
                        let _ = sink.send(Message::Text(r#"{"t":"welcome"}"#.into())).await;
                        let _ = sink.send(Message::Text(
                            r#"{"t":"peers","users":[{"id":"u-stub","name":"桩","status":"idle","gameId":null}]}"#.into(),
                        ))
                        .await;
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        break; // drop sink/stream = 断开
                    }
                });
            }
        });
    }

    // 服务器模式会话，经设置存储指向桩（`NativeSession::new` 只认宿主存储里的地址）
    let h = host();
    h.storage_set("goptop:server-sel", Some("stub"));
    let servers = format!(r#"[{{"id":"stub","label":"桩","url":"ws://127.0.0.1:{port}/ws"}}]"#);
    h.storage_set("goptop:servers", Some(servers.as_str()));
    let mut c = cfg("甲");
    c.server_mode = true;
    let srv = NativeSession::new(c, h.clone(), "http://localhost/");

    // hello 上行 + welcome/peers 下行
    let ok = pump_until(&srv, &srv, || s(&snap(&srv), "serverState") == "ready", 10).await;
    assert!(ok, "10 秒内未连上桩并进入 ready：serverState={}", s(&snap(&srv), "serverState"));
    assert!(
        snap(&srv)["peers"].as_array().is_some_and(|p| p.len() == 1),
        "peers 下行应进名册，实际 {:?}",
        snap(&srv)["peers"]
    );
    let hello = hellos.lock().unwrap().first().cloned().unwrap_or_default();
    assert!(hello.contains(r#""t":"hello""#) && hello.contains("甲"), "桩应先收到 hello：{hello}");

    // 释放会话，然后盯 11s（> RECONNECT_MS）：绝不允许第二次连接
    drop(srv);
    let deadline = std::time::Instant::now() + Duration::from_secs(11);
    while std::time::Instant::now() < deadline {
        assert_eq!(
            accepted.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "会话释放后不得再连：重连循环必须随停机标志退出"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
