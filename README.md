# GoPtop — P2P 围棋 · 五子棋

> 硬核新野兽派 · Tauri + React + TypeScript · Rust 唯一真源 · 官方 relay 真 P2P（无自建服务器）

## 快速开始

```bash
# 纯 Web 验证（无需 Tauri）
npm --prefix frontend install
npm --prefix frontend run dev   # http://localhost:1420

# Tauri 桌面（需 Rust）
npm --prefix frontend install
npm --prefix frontend run tauri dev
```

## 结构

- `crates/goptop-core` — 统一棋盘/规则/协议（可编译为 WASM）
- `crates/goptop-transport` — `Transport` 抽象 + `memory`（测试）+ `iroh`（官方 relay 真 P2P）
- `src-tauri` — Tauri 2 后端（窗口 + iroh Endpoint）
- `frontend` — Vite + React + TS（brutal.css 新野兽派）
