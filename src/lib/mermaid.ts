// Mermaid 的统一入口。
//
// 实际实现复用移植自 InkNote 的那份（`inknote/lib/mermaid.ts`，负责懒加载与
// initialize 的幂等），这里只做两件事：
// 1. 给「非编辑器」的渲染场景（聊天、内置浏览器）提供同一个入口
// 2. 按当前系统主题决定用哪套配色

export type MermaidTheme = "dark" | "default" | "neutral";

export { configuredMermaid } from "../inknote/lib/mermaid";

/** 跟随系统浅色/深色；Mermaid 的 default 就是浅色主题 */
export function currentMermaidTheme(): MermaidTheme {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "default";
  } catch {
    return "default";
  }
}
