#!/usr/bin/env bash
# 规则引擎 wasm 构建：Rust core → wasm32 release → wasm-bindgen 胶水 → frontend/src/wasm/。
# 前端构建（npm run build）直接消费已提交的胶水产物，不需要 Rust 工具链；
# 只有改了 crates/goptop-core 才需要重跑本脚本并提交更新后的 frontend/src/wasm/。
# 前置：rustup target add wasm32-unknown-unknown
#       cargo install wasm-bindgen-cli（版本必须与 Cargo.lock 锁定的 wasm-bindgen 一致）
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p goptop-core --release --target wasm32-unknown-unknown --features wasm
wasm-bindgen target/wasm32-unknown-unknown/release/goptop_core.wasm \
  --out-dir frontend/src/wasm --out-name goptop_core --target web
echo "wasm 胶水已写入 frontend/src/wasm/"
