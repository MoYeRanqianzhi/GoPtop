#!/usr/bin/env bash
# 传输层 wasm 构建：goptop-transport → wasm32 release → wasm-bindgen 胶水 → frontend/src/wasm/transport/。
# 前端构建直接消费已提交的胶水产物；只有改了 goptop-net/goptop-transport 才需要重跑。
# 前置：rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli
#
# Agent 出口（阶段⑤）**恒开**（契约 §5.1「默认带 agent」的单产物策略）：web 只有一份
# transport wasm，agent_* 导出多出的体积换零加载分叉（体积预算见契约 R-w4）。
# 不带 agent 的产物会让 webAgentAvailable() 判 false、/agent 整页退回降级横幅——
# 集成期「默认不开、GOPTOP_TRANSPORT_AGENT=1 才开」的开关已随集成完成移除，
# 不存在合法的无 agent 构建。
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p goptop-transport --release --target wasm32-unknown-unknown --features agent
wasm-bindgen target/wasm32-unknown-unknown/release/goptop_transport.wasm \
  --out-dir frontend/src/wasm/transport --out-name goptop_transport --target web
echo "transport wasm 胶水已写入 frontend/src/wasm/transport/（agent 出口：开）"
