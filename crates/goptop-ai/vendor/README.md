# vendor/ — 第三方源码副本

## figrid-board 0.8.8

五子棋 α-β + NNUE 引擎，Gomocup 参赛引擎（[crates.io](https://crates.io/crates/figrid-board) ·
[上游仓库](https://github.com/nicotina04/figrid-board)）。许可 **MIT OR Apache-2.0**，
与本项目一致。注意：crates.io 分发包内**未附许可文本**，以 crate 元数据与上游仓库的
LICENSE 为准。

### 为什么 vendor 而不是直接用

上游的搜索全程用 `std::time::Instant` 做时间控制（`src/search.rs`、`src/vct.rs`、
`src/tss.rs`、`src/vct/dfpn.rs` 共 20+ 处），而 `wasm32-unknown-unknown` 上 std 没有
时钟，`Instant::now()` 会 panic（"time not implemented on this platform"）并被编译为
wasm 的 `unreachable` 陷阱。**这个问题编译期不报错**——依赖能编过、能链接，只有真正
调用搜索时才炸。

### 相对 0.8.8 的改动（仅此一处，勿再扩大）

`src/` 下 4 个文件的 `use std::time::` 改为 `use web_time::`（`web_time::Instant` 在
wasm 上走 `performance.now()`，在原生上就是 std 的再导出），并在 `Cargo.toml` 增加
`web-time = "1"` 依赖。除此之外未改动任何逻辑。

### 同步上游时

```bash
cargo add figrid-board@<新版本>   # 先取下新版本源码
# 重新应用上述时间源替换，再覆盖本目录
```

改动面只有 4 行 import，同步成本可控。若上游某天改用 `web-time` 或提供时间源注入，
应撤回本 vendor 目录，改回 crates.io 直连。

### models/

`gomoku_v52_5stone_conv_93k.bin.gz` 是 v52 NNUE 权重（gzip 1.7MB，解压约 14.3MB），
随上游 crate 分发。我们只用权重文件，**不编译**上游的 `pbrain-figrid` 可执行目标
（它需要 `embed-weights`/`codebook-eval` feature，我们不启用）。
