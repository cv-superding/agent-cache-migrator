import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 要求固定端口；占用时直接失败，而不是悄悄换端口（否则窗口连不上 devserver）
// @ts-expect-error process 是 node 全局变量
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // 1. 别让 vite 把 rust 的报错刷掉
  clearScreen: false,
  server: {
    port: 1422,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1423 } : undefined,
    watch: {
      // 别监视 Rust 产物 —— 那是 cargo 的事，监视了会疯狂重载
      ignored: ["**/src-tauri/**"],
    },
  },
});
