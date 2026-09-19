#!/usr/bin/env bash
# AI 引擎 wasm 构建：goptop-ai → wasm32 release → wasm-bindgen 胶水 → frontend/src/wasm-ai/。
# 前端构建（npm run build）直接消费已提交的胶水产物，不需要 Rust 工具链；
# 只有改了 crates/goptop-ai 才需要重跑本脚本并提交更新后的 frontend/src/wasm-ai/。
#
# 与 build-wasm.sh（规则引擎）分开是**故意**的：AI 引擎含 1.7MB 内嵌权重、体积是规则引擎的
# 十倍，且只在人机对战与胜率显示时需要——不该压在每次都要加载的规则引擎上。
# 前置：rustup target add wasm32-unknown-unknown
#       cargo install wasm-bindgen-cli（版本必须与 Cargo.lock 锁定的 wasm-bindgen 一致）
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p goptop-ai --release --target wasm32-unknown-unknown --features wasm
wasm-bindgen target/wasm32-unknown-unknown/release/goptop_ai.wasm \
  --out-dir frontend/src/wasm-ai --out-name goptop_ai --target web
echo "AI wasm 胶水已写入 frontend/src/wasm-ai/"
