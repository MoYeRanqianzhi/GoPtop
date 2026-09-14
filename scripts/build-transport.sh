#!/usr/bin/env bash
# 传输层 wasm 构建：goptop-transport → wasm32 release → wasm-bindgen 胶水 → frontend/src/wasm/transport/。
# 前端构建直接消费已提交的胶水产物；只有改了 goptop-net/goptop-transport 才需要重跑。
# 前置：rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p goptop-transport --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/goptop_transport.wasm \
  --out-dir frontend/src/wasm/transport --out-name goptop_transport --target web
echo "transport wasm 胶水已写入 frontend/src/wasm/transport/"
