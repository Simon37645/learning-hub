// Markdown 渲染：marked + KaTeX + highlight.js + DOMPurify。
//
// 三道处理顺序很重要：渲染 → 净化 → 高亮。
// 净化放在高亮之前，保证只有白名单内的标签进入 DOM。

import { Marked } from "marked";
import markedKatex from "marked-katex-extension";
import DOMPurify from "dompurify";

// highlight.js 默认会把 190 多种语言全打进包里（约 1MB）。
// 这里只注册学习场景真正会遇到的，包体小一个数量级。
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import julia from "highlight.js/lib/languages/julia";
import latex from "highlight.js/lib/languages/latex";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import matlab from "highlight.js/lib/languages/matlab";
import powershell from "highlight.js/lib/languages/powershell";
import python from "highlight.js/lib/languages/python";
import r from "highlight.js/lib/languages/r";
import rust from "highlight.js/lib/languages/rust";
import scala from "highlight.js/lib/languages/scala";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const LANGUAGES: [string, Parameters<typeof hljs.registerLanguage>[1]][] = [
  ["python", python],
  ["r", r],
  ["julia", julia],
  ["matlab", matlab],
  ["javascript", javascript],
  ["typescript", typescript],
  ["rust", rust],
  ["c", c],
  ["cpp", cpp],
  ["csharp", csharp],
  ["java", java],
  ["go", go],
  ["scala", scala],
  ["bash", bash],
  ["powershell", powershell],
  ["sql", sql],
  ["json", json],
  ["yaml", yaml],
  ["ini", ini],
  ["xml", xml],
  ["css", css],
  ["markdown", markdown],
  ["latex", latex],
  ["diff", diff],
  ["dockerfile", dockerfile],
  ["makefile", makefile],
];
for (const [name, lang] of LANGUAGES) hljs.registerLanguage(name, lang);

/** 常见的别名 / 非标准标注，映射到已注册的语言 */
const ALIASES: Record<string, string> = {
  py: "python",
  ipynb: "python",
  js: "javascript",
  jsx: "javascript",
  node: "javascript",
  ts: "typescript",
  tsx: "typescript",
  rs: "rust",
  "c++": "cpp",
  cxx: "cpp",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  text: "plaintext",
  txt: "plaintext",
  tex: "latex",
  md: "markdown",
  yml: "yaml",
  htm: "xml",
  html: "xml",
  svg: "xml",
  mat: "matlab",
  octave: "matlab",
  "c#": "csharp",
};

const marked = new Marked(
  {
    gfm: true,
    breaks: true,
    pedantic: false,
  },
  markedKatex({
    throwOnError: false,
    nonStandard: true,
    output: "htmlAndMathml",
  }),
);

const PURIFY_OPTIONS = {
  USE_PROFILES: { html: true, svg: true, mathMl: true },
  ADD_ATTR: ["target", "rel", "class", "id", "colspan", "rowspan", "align", "start", "data-page"],
  ADD_TAGS: ["annotation", "semantics", "mrow", "mi", "mo", "mn", "msup", "msub", "mfrac"],
  FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form", "input"],
  FORBID_ATTR: ["onerror", "onload", "onclick"],
};

/** Markdown → 安全 HTML 字符串 */
export function renderMarkdown(source: string): string {
  if (!source) return "";
  let html = "";
  try {
    html = marked.parse(source, { async: false }) as string;
  } catch (e) {
    return `<pre class="md-error">Markdown 渲染失败：${escapeHtml(String(e))}</pre>`;
  }
  return DOMPurify.sanitize(html, PURIFY_OPTIONS) as unknown as string;
}

/** 对容器里的代码块做语法高亮（渲染完 HTML 之后调用） */
export function highlightWithin(root: HTMLElement | null): void {
  if (!root) return;
  root.querySelectorAll<HTMLElement>("pre code").forEach((block) => {
    if (block.dataset.hl === "1") return;
    try {
      // marked 输出的是 class="language-xxx"，先归一到我们注册过的语言名
      const raw = Array.from(block.classList)
        .find((c) => c.startsWith("language-"))
        ?.slice("language-".length)
        .toLowerCase();
      const name = raw ? ALIASES[raw] ?? raw : undefined;
      if (name && hljs.getLanguage(name)) {
        const html = hljs.highlight(block.textContent ?? "", { language: name }).value;
        block.innerHTML = html;
      } else if (!raw) {
        // 没标语言就让它自动猜（猜不准也不报错）
        const auto = hljs.highlightAuto(block.textContent ?? "");
        if (auto.relevance > 6) block.innerHTML = auto.value;
      }
      block.dataset.hl = "1";
    } catch {
      // 高亮失败就当普通代码块，保持原样
    }
  });
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** 从 Markdown 里抽出所有标题，用于生成大纲 */
export function outlineOf(source: string): { level: number; text: string; line: number }[] {
  const out: { level: number; text: string; line: number }[] = [];
  let inCode = false;
  source.split("\n").forEach((line, i) => {
    if (line.trim().startsWith("```")) {
      inCode = !inCode;
      return;
    }
    if (inCode) return;
    const m = /^(#{1,6})\s+(.*)$/.exec(line);
    if (m) {
      out.push({ level: m[1].length, text: m[2].trim(), line: i });
    }
  });
  return out;
}

/**
 * 把 ```mermaid 代码块渲染成图（思维导图、流程图、时序图、时间线…）。
 *
 * 为什么在渲染后处理而不是做成 marked 插件：mermaid 是异步且要操作真实 DOM 的，
 * 放在标记解析阶段会把渲染流程拖成异步；这里按「先出代码块，再原地替换」处理，
 * 即使 mermaid 还没加载完，用户看到的也只是代码块而不是空白。
 *
 * 安全：mermaid 以 `securityLevel: "strict"` 初始化（见 lib/mermaid.ts），
 * 标签里的 HTML 会被转义，所以直接把生成的 SVG 插进 DOM。
 */
let mermaidSeq = 0;

export async function renderMermaidIn(root: HTMLElement | null): Promise<number> {
  if (!root) return 0;
  const blocks = Array.from(root.querySelectorAll<HTMLElement>("pre > code.language-mermaid"));
  if (blocks.length === 0) return 0;

  const { renderDiagram, currentMermaidTheme } = await import("./mermaid");
  let rendered = 0;

  for (const code of blocks) {
    const pre = code.parentElement;
    if (!pre || pre.dataset.mermaid === "done") continue;
    const source = code.textContent ?? "";
    if (!source.trim()) continue;

    // id 只是给失败时清理用；真正的渲染 id 由共享模块统一分配（避免撞 id）
    const id = `mmd-${++mermaidSeq}`;
    try {
      const svg = await renderDiagram(source, currentMermaidTheme());

      const holder = document.createElement("div");
      holder.className = "mermaid-block";
      holder.dataset.mermaid = "done";
      holder.innerHTML = `
        <div class="mermaid-tools">
          <button type="button" data-zoom="fit">适应宽度</button>
          <button type="button" data-zoom="1">100%</button>
          <button type="button" data-zoom="1.6">放大</button>
        </div>
        <div class="mermaid-canvas">${svg}</div>`;
      applyZoom(holder, "fit");
      holder.querySelectorAll<HTMLButtonElement>("[data-zoom]").forEach((btn) => {
        btn.addEventListener("click", (e) => {
          e.preventDefault();
          e.stopPropagation();
          applyZoom(holder, btn.dataset.zoom ?? "fit");
        });
      });

      pre.replaceWith(holder);
      rendered++;
    } catch (e) {
      // 图有语法错误时保留代码块，并在旁边说明——总比整段消失好
      pre.dataset.mermaid = "failed";
      // 防御：万一 Mermaid 仍然往 DOM 里插了错误节点（它有过这种行为），
      // 这里把它清掉，免得留在界面上还被拍进截图
      cleanupMermaidErrorNode(id);
      const note = document.createElement("div");
      note.className = "mermaid-error";
      note.textContent = `这张图渲染失败（${String(e).slice(0, 120)}），下面是原始定义`;
      pre.parentElement?.insertBefore(note, pre);
    }
  }
  return rendered;
}

/**
 * 清掉 Mermaid 在渲染失败时可能注入到 DOM 里的错误节点。
 *
 * Mermaid 会在 `document.body`（或容器内）插入一个 id 形如 `dmermaid-<id>` 的元素，
 * 里面是「Syntax error in text」的提示。它是全局副作用，不会随 React 卸载消失，
 * 所以这里主动收尾。已开启 `suppressErrorRendering`，这里是双保险。
 */
function cleanupMermaidErrorNode(id: string): void {
  for (const node of Array.from(document.querySelectorAll(`#d${CSS.escape(id)}, [id^="dmermaid"]`))) {
    node.remove();
  }
}

/** 缩放：适应宽度用 max-width 限制，固定比例用宽度百分比 */function applyZoom(holder: HTMLElement, zoom: string) {
  const canvas = holder.querySelector<HTMLElement>(".mermaid-canvas");
  const svg = canvas?.querySelector<SVGSVGElement>("svg");
  if (!canvas || !svg) return;
  holder.querySelectorAll("[data-zoom]").forEach((b) => b.classList.remove("on"));
  holder.querySelector(`[data-zoom="${zoom}"]`)?.classList.add("on");

  if (zoom === "fit") {
    svg.style.removeProperty("width");
    svg.style.removeProperty("height");
    svg.style.maxWidth = "100%";
    canvas.style.width = "100%";
  } else {
    const k = Number(zoom) || 1;
    svg.style.removeProperty("max-width");
    canvas.style.width = `${Math.round(k * 100)}%`;
  }
}

/**
 * 把 ```html / ```svg 代码块渲染成**演示卡片**（agent 用它讲难懂的东西）。
 *
 * 为什么这么做：动态过程、空间关系、参数怎么影响结果这类内容，文字讲十句不如看一眼。
 * 约定就是「在回复里放一个 ```html 代码块」——模型最容易写对，用户也一眼看得见。
 *
 * 与 mermaid 同一套路：先让代码块照常渲染出来，再原地替换成卡片。
 * 这样即使这一步没跑到、或 HTML 写坏了，用户看到的也只是代码块，不会是空白。
 *
 * 安全：iframe 是 `sandbox=""`（**不给 allow-scripts**），所以演示里的 <script> 不会执行，
 * 也碰不到应用的 DOM 与 IPC。动画只能靠 CSS（animation / transition）或 SVG 的
 * <animate>，纯 CSS 的交互（:hover / :checked / <details>）也照常能用。
 * 提示词里就是这么要求模型的；卡片上会明说这一点，免得用户以为是应用坏了。
 */
const DEMO_LANGS = new Set(["html", "htm", "svg"]);

/** 组装 iframe 里真正要渲染的那份文档（`svg` 代码块补一层 HTML 壳）。 */
export function demoDocument(lang: string, source: string): string {
  if (lang !== "svg") return source;
  return (
    "<!doctype html><html><head><meta charset=\"utf-8\"><style>" +
    "html,body{margin:0;height:100%;display:grid;place-items:center;overflow:hidden}" +
    "svg{max-width:100%;max-height:100%}" +
    `</style></head><body>${source}</body></html>`
  );
}

export function renderDemoBlocksIn(root: HTMLElement | null): number {
  if (!root) return 0;
  let rendered = 0;

  for (const code of Array.from(root.querySelectorAll<HTMLElement>("pre > code"))) {
    const pre = code.parentElement as HTMLElement | null;
    if (!pre || pre.dataset.demo === "done") continue;
    const lang = Array.from(code.classList)
      .find((c) => c.startsWith("language-"))
      ?.slice("language-".length)
      .toLowerCase();
    if (!lang || !DEMO_LANGS.has(lang)) continue;
    const source = code.textContent ?? "";
    if (!source.trim()) continue;

    pre.dataset.demo = "done";
    const card = buildDemoCard(lang, source);
    pre.replaceWith(card);
    // 原始代码块留在卡片里（默认收起）：用户想抄走或改的时候用得上
    pre.hidden = true;
    card.appendChild(pre);
    rendered++;
  }
  return rendered;
}

function buildDemoCard(lang: string, source: string): HTMLElement {
  const card = document.createElement("div");
  card.className = "demo-card";
  card.dataset.lang = lang;

  const head = document.createElement("div");
  head.className = "demo-head";
  const label = document.createElement("span");
  label.className = "demo-label";
  label.textContent = lang === "svg" ? "演示 · SVG" : "演示";
  head.appendChild(label);

  // 写了脚本但不会跑——说清楚，否则用户会以为「动画坏了」
  if (/<script[\s>]/i.test(source)) {
    const warn = document.createElement("span");
    warn.className = "demo-warn";
    warn.textContent = "脚本已禁用（沙箱），只显示静态效果";
    warn.title = "演示卡片在沙箱里渲染，不允许执行 JavaScript。动画请让 agent 用 CSS 或 SVG 的 <animate>。";
    head.appendChild(warn);
  }

  const spacer = document.createElement("div");
  spacer.className = "spacer";
  head.appendChild(spacer);

  const srcBtn = document.createElement("button");
  srcBtn.type = "button";
  srcBtn.className = "demo-btn";
  srcBtn.textContent = "源码";
  srcBtn.addEventListener("click", (e) => {
    e.preventDefault();
    const pre = card.querySelector<HTMLElement>("pre");
    if (!pre) return;
    pre.hidden = !pre.hidden;
    srcBtn.classList.toggle("on", !pre.hidden);
  });
  head.appendChild(srcBtn);

  const fsBtn = document.createElement("button");
  fsBtn.type = "button";
  fsBtn.className = "demo-btn";
  fsBtn.textContent = "全屏";
  fsBtn.addEventListener("click", (e) => {
    e.preventDefault();
    // 全屏 API 在 WebView2 里可用；失败（被策略拦下）时什么都不做，
    // 不能让它抛出去把点击事件整条打断
    try {
      if (document.fullscreenElement === card) void document.exitFullscreen();
      else void card.requestFullscreen();
    } catch {
      /* 不支持就算了：卡片本身也能看 */
    }
  });
  head.appendChild(fsBtn);

  const body = document.createElement("div");
  body.className = "demo-body";
  const frame = document.createElement("iframe");
  frame.className = "demo-frame";
  // 空 sandbox：不给脚本、不给表单、不给同源——模型产出的 HTML 只被当成画面
  frame.setAttribute("sandbox", "");
  frame.setAttribute("title", "演示");
  frame.setAttribute("loading", "lazy");
  frame.srcdoc = demoDocument(lang, source);
  body.appendChild(frame);

  card.appendChild(head);
  card.appendChild(body);
  return card;
}

/** 判断一段文本是否几乎全是代码（用于选择更窄的排版） */export function looksLikeCode(text: string): boolean {
  const lines = text.split("\n").filter((l) => l.trim().length > 0);
  if (lines.length < 3) return false;
  const codeish = lines.filter((l) =>
    /^\s*(const|let|var|function|class|def|import|from|#include|public|private|fn|pub|impl|return|if|for|while|\}|\{|\)|;)/.test(
      l,
    ),
  ).length;
  return codeish / lines.length > 0.6;
}

/**
 * 把 agent 写的来源标注变成可点击的跳转。
 *
 * 约定格式：`【来源：materials/ch1.pdf 第 12 页】` 或 `【来源：notes/特征值.md】`。
 * 为什么用这种「人类可读」的格式而不是链接：模型写这种标记几乎不会出错，
 * 而自定义 URL scheme 经常被它写坏或干脆忘掉。
 */
export interface CitationRef {
  path: string;
  page: number | null;
}

const CITATION_RE = /【来源：([^】]+?)】/g;

export function parseCitation(raw: string): CitationRef | null {
  const inner = raw.trim();
  // 尾巴上的「第 N 页」是页码，不是文件名的一部分。
  // 其它定位词（「第四节」这类）在这里一并丢掉——定位只认页码，
  // 页码由 kb_search 给出（见 kb.rs 的分页缓存），模型照抄即可。
  const m = /^(.*?)(?:\s*第\s*([0-9]+|[零一二三四五六七八九十百两]+)\s*(页|章|节|讲|篇|段|课|部分))?$/.exec(inner);
  if (!m) return null;
  const path = m[1].trim().replace(/[，,。；;]$/, "");
  if (!path) return null;
  const page = m[2] && m[3] === "页" && /^[0-9]+$/.test(m[2]) ? Number(m[2]) : null;
  return { path, page };
}

/** 在已渲染的 DOM 里把来源标注替换成按钮（返回替换了几处） */
export function linkifyCitations(root: HTMLElement | null, onOpen: (ref: CitationRef) => void): number {
  if (!root) return 0;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const value = node.nodeValue ?? "";
      if (!value.includes("【来源：")) return NodeFilter.FILTER_REJECT;
      const parent = (node as Text).parentElement;
      if (!parent) return NodeFilter.FILTER_REJECT;
      const tag = parent.tagName.toLowerCase();
      if (tag === "script" || tag === "style" || tag === "button" || tag === "code") {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });

  const targets: Text[] = [];
  let n: Node | null;
  while ((n = walker.nextNode())) targets.push(n as Text);

  let count = 0;
  for (const node of targets) {
    const value = node.nodeValue ?? "";
    const frag = document.createDocumentFragment();
    let last = 0;
    CITATION_RE.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = CITATION_RE.exec(value))) {
      const ref = parseCitation(m[1]);
      if (!ref) continue;
      if (m.index > last) frag.appendChild(document.createTextNode(value.slice(last, m.index)));
      const btn = document.createElement("button");
      btn.className = "cite-chip";
      btn.type = "button";
      btn.title = `打开 ${ref.path}${ref.page ? ` 第 ${ref.page} 页` : ""}`;
      btn.textContent = ref.page ? `${ref.path} 第${ref.page}页` : ref.path;
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        onOpen(ref);
      });
      frag.appendChild(btn);
      last = m.index + m[0].length;
      count++;
    }
    if (last < value.length) frag.appendChild(document.createTextNode(value.slice(last)));
    if (count > 0) node.parentNode?.replaceChild(frag, node);
  }
  return count;
}
