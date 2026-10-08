#!/usr/bin/env bash
# 传输层 wasm 构建：goptop-transport → wasm32 release → wasm-bindgen 胶水 → frontend/src/wasm/transport/。
# 前端构建直接消费已提交的胶水产物；只有改了 goptop-net/goptop-transport 才需要重跑。
# 前置：rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli
#
# Agent 出口（阶段⑤）：开关 `GOPTOP_TRANSPORT_AGENT=1` 追加 `--features agent`
# （Web 内置模式的 agent_* 导出面随单产物上线——web 只有一份 transport wasm，
# agent 导出多出的体积换零加载分叉）。**默认不开**：不带开关的产物与阶段④完全
# 同体积（goptop-agent 不进树），集成阶段切换前端构建前产物零回归。
set -euo pipefail
cd "$(dirname "$0")/.."
FEATURES=()
if [[ "${GOPTOP_TRANSPORT_AGENT:-0}" == "1" ]]; then
  FEATURES+=(--features agent)
fi
cargo build -p goptop-transport --release --target wasm32-unknown-unknown "${FEATURES[@]}"
wasm-bindgen target/wasm32-unknown-unknown/release/goptop_transport.wasm \
  --out-dir frontend/src/wasm/transport --out-name goptop_transport --target web
echo "transport wasm 胶水已写入 frontend/src/wasm/transport/（agent 出口：${GOPTOP_TRANSPORT_AGENT:-0}）"
