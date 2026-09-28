import { defineConfig } from "vitest/config";

// 只跑编辑器（移植自 InkNote）的单元测试。
// 它们是移植后行为一致性的回归网：预览渲染、公式扫描、路径处理、frontmatter 都在里面。
export default defineConfig({
  test: {
    environment: "happy-dom",
    // 移植过来的集成测试较重（整篇文档 + 预览渲染），默认 5s 在这台机器上不够
    testTimeout: 30000,
    include: ["src/inknote/**/*.test.ts", "src/inknote/**/*.test.tsx"],
  },
});
