import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// 样式加载顺序：设计 token → 组件样式 → 第三方（KaTeX 字体与布局）
// 代码高亮的 token 配色写在 app.css 里，随浅色/深色自动切换。
import "./styles/app.css";
import "katex/dist/katex.min.css";
// 内置笔记编辑器（移植自 InkNote）：令牌映射在前，组件样式在后
import "./inknote/editor-tokens.css";
import "./inknote/editor.css";

// 阻止 WebView 默认的右键菜单与文件拖放跳转（拖放由 Tauri 事件接管）
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
