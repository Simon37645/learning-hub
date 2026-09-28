/**
 * Typora「LaTeX」主题的配色常量。
 *
 * 取值来自 Typora 正在使用的主题文件
 * `%APPDATA%\Typora\themes\latex.css`（浅色，`isDarkMode: false`），
 * 终端与 Markdown 主题共用，保证两处观感一致。
 */
export interface TyporaLatexPalette {
  /** 纸张底色（#write 的 background-color） */
  paper: string;
  /** 正文墨色 */
  ink: string;
  /** 次级文字 */
  muted: string;
  /** 超链接（--link-color-light / --link-color-dark） */
  link: string;
  /** 行内代码文字 rgb(60, 112, 198) */
  codeInk: string;
  /** 行内代码底色 #fefefe */
  codePaper: string;
  /** 行内代码描边（原主题用 box-shadow 0 0 1px 1px #c8d3df 模拟） */
  codeEdge: string;
  /** 水平分割线 #ddd */
  rule: string;
  /** 引用块左竖线 hsl(0, 0%, 70%) */
  quoteRule: string;
  /** 次要面板底色（引用块、代码块底） */
  panel: string;
}

export const TYPORA_LATEX_LIGHT: TyporaLatexPalette = {
  // 纸面用泛黄纸（浅色下的实际取值见 App.css 的 latex 主题块）
  paper: "#f6f1e4",
  ink: "#2f2a22",
  muted: "#6a6152",
  link: "#2e67d3",
  codeInk: "#2e5fb8",
  codePaper: "#fffdf4",
  codeEdge: "#d3cbb6",
  rule: "#ddd6c4",
  quoteRule: "#c4b89f",
  panel: "#efe8d7",
};

export const TYPORA_LATEX_DARK: TyporaLatexPalette = {
  paper: "#1e1e1e",
  ink: "#e4e4e4",
  muted: "#a0a0a0",
  link: "#8bb1f9",
  codeInk: "#9db8f0",
  codePaper: "#252526",
  codeEdge: "#3c3c3c",
  rule: "#3c3c3c",
  quoteRule: "#6b6b6b",
  panel: "#252526",
};

/** xterm 的 16 色 ANSI 调色板，与上面的纸张/墨色保持同一套色感。 */
export interface TerminalAnsiPalette {
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

const ANSI_LIGHT: TerminalAnsiPalette = {
  black: "#1f1f1f",
  red: "#b31d28",
  green: "#2c7a4b",
  yellow: "#9a6700",
  blue: "#2e67d3",
  magenta: "#6f42c1",
  cyan: "#1b7c83",
  white: "#b3b3b3",
  brightBlack: "#6b6b6b",
  brightRed: "#d1242f",
  brightGreen: "#3e9b63",
  brightYellow: "#bf8700",
  brightBlue: "#4a82e8",
  brightMagenta: "#8250df",
  brightCyan: "#3192a0",
  brightWhite: "#ffffff",
};

const ANSI_DARK: TerminalAnsiPalette = {
  black: "#3c3c3c",
  red: "#f47067",
  green: "#7ec699",
  yellow: "#e3b341",
  blue: "#8bb1f9",
  magenta: "#d2a8ff",
  cyan: "#56d4dd",
  white: "#d0d0d0",
  brightBlack: "#8b949e",
  brightRed: "#ff9492",
  brightGreen: "#a5d6a7",
  brightYellow: "#f0c674",
  brightBlue: "#a8c7fa",
  brightMagenta: "#e2c5ff",
  brightCyan: "#8be9f0",
  brightWhite: "#ffffff",
};

export function typoraLatexPalette(mode: "light" | "dark"): TyporaLatexPalette {
  return mode === "dark" ? TYPORA_LATEX_DARK : TYPORA_LATEX_LIGHT;
}

export function typoraTerminalAnsi(mode: "light" | "dark"): TerminalAnsiPalette {
  return mode === "dark" ? ANSI_DARK : ANSI_LIGHT;
}

export function resolveVisualMode(): "light" | "dark" {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}

/** 组装 xterm 的 theme 对象。 */
export function buildTerminalTheme(mode: "light" | "dark") {
  const palette = typoraLatexPalette(mode);
  const ansi = typoraTerminalAnsi(mode);
  return {
    background: palette.paper,
    foreground: palette.ink,
    cursor: palette.link,
    cursorAccent: palette.paper,
    selectionBackground: mode === "dark" ? "rgba(139, 177, 249, 0.3)" : "rgba(46, 103, 211, 0.18)",
    ...ansi,
  };
}
