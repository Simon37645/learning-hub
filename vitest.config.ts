import { defineConfig } from "vitest/config";

// 两类单元测试：
// 1. 编辑器（移植自 InkNote）的回归网：预览渲染、公式扫描、路径处理、frontmatter；
// 2. 应用自己的纯逻辑（对话框底部的缓存命中率统计、图标/格式化这类小工具）。
export default defineConfig({
  test: {
    environment: "happy-dom",
    // 移植过来的集成测试较重（整篇文档 + 预览渲染），默认 5s 在这台机器上不够
    testTimeout: 30000,
    // happy-dom 环境极重（每个测试文件起一个完整 DOM），默认按 CPU 数并行会让
    // worker 互相饿死：最重的编辑器集成测试里那个「等补全激活」的异步断言，
    // 计时器被挤到晚点，waitFor 的等待预算和 testTimeout 一起按墙钟爆掉
    // （实测全量并行时该用例能挂 46-64 秒，单跑只要 0.1 秒）。把文件级并行度
    // 压到 4 换稳定——整套测试本来就十几秒，串行化也慢不到哪里去。
    fileParallelism: 4,
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
