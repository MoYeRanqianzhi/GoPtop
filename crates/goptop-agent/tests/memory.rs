//! /memory 的工具面测试 —— 计划「测试计划」第 1 条的「记忆 round-trip 与 edit
//! 不唯一报错」，从 `execute()` 一口进（B 路的 store.rs 单测已钉住存储层语义：
//! round-trip/配额/usage 账目/路径规范化；本文件钉的是**工具层**的回执形状、
//! 错误文案与配额前置拦截——两层之间正是集成最容易出缝的地方）。

use std::sync::Arc;

use goptop_agent::player::{EventQueue, HookHost};
use goptop_agent::registry::{ToolCtx, ToolError, execute};
use goptop_agent::store::NativeStore;
use goptop_agent::vfs::Staging;
use goptop_transport_native::{HeadlessHost, Host, NativeSession};
use serde_json::json;

/// 每用例独享的记忆库文件（并行用例同进程，文件名撞车会互读对方的表）。
fn temp_db() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("goptop-agent-mem-{}-{n}.db", std::process::id()))
}

/// 搭一个工具面 ctx：会话停在 home 相位（记忆语义不依赖对局），免配对成本。
fn ctx() -> ToolCtx {
    let h: Arc<dyn Host> = Arc::new(HeadlessHost::default());
    let (hook, watch) = HookHost::wrap(h);
    let _ = hook; // watch 的 sender 持有者；活到 ctx 结束即可
    ToolCtx {
        player: Arc::new(goptop_agent::player::NativePlayer::new(Arc::new(NativeSession::new(
            goptop_transport_native::SessionConfig {
                name: "测试".into(),
                server_mode: false,
                share_origin: "http://localhost".into(),
                kind: "gomoku".into(),
                size: 15,
            },
            Arc::new(HeadlessHost::default()),
            "http://localhost/p2p",
        )))),
        watch,
        events: Arc::new(EventQueue::new()),
        staging: Arc::new(Staging::new()),
        memory: Arc::new(NativeStore::open(&temp_db()).unwrap()),
        memory_ns: "builtin",
        driver: goptop_agent::Driver::Builtin,
        subagent_enabled: false,
        subagent: None,
    }
}

async fn act(ctx: &ToolCtx, name: &str, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    execute(name, &args, ctx).await
}

/// 工具面 round-trip：write 回执（bytes/usage）、read 回显、覆盖写按净增量计费。
#[tokio::test]
async fn write_read_round_trip() {
    let c = ctx();
    let out = act(&c, "write", json!({ "path": "/memory/notes/style.md", "content": "黑喜占角" }))
        .await
        .expect("write 应成功");
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["path"], json!("/memory/notes/style.md"));
    assert_eq!(out["bytes"], json!("黑喜占角".len()));
    assert_eq!(out["usage"], json!("黑喜占角".len()));

    let out = act(&c, "read", json!({ "path": "/memory/notes/style.md" })).await.expect("read 应成功");
    assert_eq!(out["content"], json!("黑喜占角"));
    assert_eq!(out["total_lines"], json!(1));

    // 覆盖写：usage 按净增量（不是字面相加）。
    let out = act(&c, "write", json!({ "path": "/memory/notes/style.md", "content": "黑喜占角；白好战" }))
        .await
        .expect("覆盖写应成功");
    assert_eq!(out["usage"], json!("黑喜占角；白好战".len()), "净增量计费");

    // 读不存在的文件：回人话错误 + 指路 grep。
    let err = act(&c, "read", json!({ "path": "/memory/nope.md" })).await.unwrap_err();
    assert!(err.message().contains("file not found: /memory/nope.md"), "{err:?}");
    assert!(err.message().contains("grep"), "错误要指路 grep");
}

/// edit 的 Claude Code 语义：0 次拒、不唯一拒、唯一处替换、replace_all 全替换。
#[tokio::test]
async fn edit_唯一性与替换() {
    let c = ctx();
    act(&c, "write", json!({ "path": "/memory/a.md", "content": "甲乙甲" })).await.expect("write");

    // 0 次：拒。
    let err = act(&c, "edit", json!({ "path": "/memory/a.md", "old_string": "丙", "new_string": "丁" }))
        .await
        .unwrap_err();
    assert!(err.message().contains("not found"), "{err:?}");
    // >1 次且未 replace_all：不唯一拒。
    let err = act(&c, "edit", json!({ "path": "/memory/a.md", "old_string": "甲", "new_string": "丁" }))
        .await
        .unwrap_err();
    assert!(err.message().contains("2 times"), "{err:?}");
    // edit 不隐式创建：对不存在的文件拒。
    let err = act(&c, "edit", json!({ "path": "/memory/new.md", "old_string": "x", "new_string": "y" }))
        .await
        .unwrap_err();
    assert!(err.message().contains("never creates files"), "{err:?}");

    // replace_all=true：全替换并报实数。
    let out = act(
        &c,
        "edit",
        json!({ "path": "/memory/a.md", "old_string": "甲", "new_string": "丁", "replace_all": true }),
    )
    .await
    .expect("replace_all 应成功");
    assert_eq!(out["occurrences"], json!(2));
    let out = act(&c, "read", json!({ "path": "/memory/a.md" })).await.expect("read");
    assert_eq!(out["content"], json!("丁乙丁"));

    // 唯一处替换。
    act(&c, "write", json!({ "path": "/memory/b.md", "content": "only one here" })).await.expect("write");
    let out = act(&c, "edit", json!({ "path": "/memory/b.md", "old_string": "one", "new_string": "two" }))
        .await
        .expect("唯一替换");
    assert_eq!(out["occurrences"], json!(1));
    assert_eq!(out["size"], json!("only two here".len()));

    // edit 不越过 /memory：in/ 槽与 /game 只读文件各有归属。
    let err = act(&c, "edit", json!({ "path": "/game/in/move", "old_string": "a", "new_string": "b" }))
        .await
        .unwrap_err();
    assert!(err.message().contains("staging slot"), "{err:?}");
    let err = act(&c, "edit", json!({ "path": "/game/board", "old_string": "a", "new_string": "b" }))
        .await
        .unwrap_err();
    assert!(err.message().contains("read-only"), "{err:?}");
}

/// 配额与路径规范在工具层的前置拦截（RespondToModel 级，存储不落半截）。
#[tokio::test]
async fn 配额与路径规范拦截() {
    let c = ctx();
    // 单文件超限：write 即拒（工具层前置查，回人话）。
    let big = "x".repeat(256 * 1024 + 1);
    let err = act(&c, "write", json!({ "path": "/memory/big.md", "content": big })).await.unwrap_err();
    assert!(err.message().contains("limited to 262144"), "{err:?}");
    let out = act(&c, "read", json!({ "path": "/memory/big.md" })).await;
    assert!(out.is_err(), "超限内容绝不落库");

    // 恰好 256KB 合法。
    let ok = "x".repeat(256 * 1024);
    let out = act(&c, "write", json!({ "path": "/memory/big.md", "content": ok })).await.expect("256KB 合法");
    assert_eq!(out["bytes"], json!(256 * 1024));

    // 路径规范化：.. / 反斜杠 / 空段折叠，在工具层回人话错误。
    for bad in ["a/../b", "a\\b"] {
        let err = act(&c, "write", json!({ "path": format!("/memory/{bad}"), "content": "x" }))
            .await
            .unwrap_err();
        assert!(err.message().contains("invalid path"), "非法路径 {bad}：{err:?}");
    }
    // 空段折叠合法：///a///b == a/b。
    let out = act(&c, "write", json!({ "path": "/memory///fold///x.md", "content": "v" }))
        .await
        .expect("折叠路径合法");
    assert_eq!(out["path"], json!("/memory/fold/x.md"), "存表键形态回显");

    // grep /memory 命中已写文件（工具层的记忆遍历）。
    act(&c, "write", json!({ "path": "/memory/notes/rival.md", "content": "对手爱下三三" })).await.expect("write");
    let hits = act(&c, "grep", json!({ "pattern": "三三", "path": "/memory" })).await.expect("grep");
    assert_eq!(hits["total"], json!(1), "{hits}");
    assert_eq!(hits["matches"][0]["path"], json!("/memory/notes/rival.md"));
}
