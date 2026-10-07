import { defineConfig } from "vitest/config";

// 两类单元测试：
// 1. 编辑器（移植自 InkNote）的回归网：预览渲染、公式扫描、路径处理、frontmatter；
// 2. 应用自己的纯逻辑（对话框底部的缓存命中率统计、图标/格式化这类小工具）。
export default defineConfig({
  test: {
    environment: "happy-dom",
    // 移植过来的集成测试较重（整篇文档 + 预览渲染），默认 5s 在这台机器上不够
    testTimeout: 30000,
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
