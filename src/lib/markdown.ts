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

  const { configuredMermaid, currentMermaidTheme } = await import("./mermaid");
  let rendered = 0;

  for (const code of blocks) {
    const pre = code.parentElement;
    if (!pre || pre.dataset.mermaid === "done") continue;
    const source = code.textContent ?? "";
    if (!source.trim()) continue;

    try {
      const mermaid = await configuredMermaid(currentMermaidTheme());
      const id = `mmd-${++mermaidSeq}`;
      const { svg } = await mermaid.render(id, source);

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
      const note = document.createElement("div");
      note.className = "mermaid-error";
      note.textContent = `这张图渲染失败（${String(e).slice(0, 120)}），下面是原始定义`;
      pre.parentElement?.insertBefore(note, pre);
    }
  }
  return rendered;
}

/** 缩放：适应宽度用 max-width 限制，固定比例用宽度百分比 */
function applyZoom(holder: HTMLElement, zoom: string) {
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

/** 判断一段文本是否几乎全是代码（用于选择更窄的排版） */
export function looksLikeCode(text: string): boolean {
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
  const m = /^(.*?)(?:\s*第\s*(\d+)\s*页)?$/.exec(inner);
  if (!m) return null;
  const path = m[1].trim().replace(/[，,。；;]$/, "");
  if (!path) return null;
  return { path, page: m[2] ? Number(m[2]) : null };
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
