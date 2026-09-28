import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 需要固定端口 + 固定 host，失败即退出，避免 devUrl 对不上。
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "chrome110",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    // PDF 渲染与 Markdown 依赖体积大，拆出去让首屏更快
    rollupOptions: {
      output: {
        manualChunks: {
          pdf: ["pdfjs-dist"],
          markdown: ["marked", "marked-katex-extension", "katex", "highlight.js", "dompurify"],
        },
      },
    },
    chunkSizeWarningLimit: 900,
  },
  optimizeDeps: {
    // pdf.js 的 worker 是独立 chunk，预构建会破坏路径解析。
    exclude: ["pdfjs-dist"],
  },
});
