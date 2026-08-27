import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 开发时由 Tauri 注入 dev host，纯 Web 开发时为空。
// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // 以下为 Tauri 官方模板的 Vite 配置，兼容纯 Web 与 Tauri 双模式：
  // - clearScreen: false 便于查看 Rust 侧错误
  // - 固定 1420 端口供 Tauri 期望，strictPort 保证端口冲突时直接失败而非静默换端口
  // - 忽略 src-tauri 避免无谓监听
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
