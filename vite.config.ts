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
    // workspace/ 是应用的数据目录，不该被前端 HMR 监视：
    // 在 Windows 上 chokidar 会对被监视的目录持有句柄，导致开发模式下
    // 「删除主题」这类目录改名操作直接失败（EPERM 拒绝访问）。
    // src-tauri 同理（Rust 由 tauri dev 自己监视）。
    watch: { ignored: ["**/src-tauri/**", "**/workspace/**"] },
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
