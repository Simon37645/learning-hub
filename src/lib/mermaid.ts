// Mermaid 的统一入口。
//
// 实际实现复用移植自 InkNote 的那份（`inknote/lib/mermaid.ts`，负责懒加载与
// initialize 的幂等），这里只做两件事：
// 1. 给「非编辑器」的渲染场景（聊天、内置浏览器）提供同一个入口
// 2. 按**应用当前主题**决定用哪套配色
//
// 主题必须与内置编辑器取同一个来源（html[data-theme]）：
// Mermaid 是单例，`initialize` 只在主题变化时被调用；如果两个入口各自判断主题
// （一个看系统、一个看 data-theme），在「系统深色 + 应用强制明亮」这类情况下
// 就会互相把对方 initialize 掉，渲染到一半的图会抛出
// 「Syntax error in text」——而且 Mermaid 会把这个错误框塞进 DOM，
// 一直留在界面上（曾经被截进 README 的图里）。

export type MermaidTheme = "dark" | "default" | "neutral";

export { configuredMermaid, renderDiagram } from "../inknote/lib/mermaid";

/**
 * 当前该用哪套 Mermaid 配色。
 *
 * 优先读 `html[data-theme]`（应用主题的真源），拿不到时退回系统偏好。
 * `data-theme` 由 store 的 applyTheme 统一维护，三种模式（跟随系统/明亮/深色）都会写进去。
 */
export function currentMermaidTheme(): MermaidTheme {
  const attr = document.documentElement.dataset.theme;
  if (attr === "dark") return "dark";
  if (attr === "light") return "default";
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "default";
  } catch {
    return "default";
  }
}
