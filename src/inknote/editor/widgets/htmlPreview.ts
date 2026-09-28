import { EditorView, WidgetType } from "@codemirror/view";
import { bindBlockBoundaryCursor, stampBlockRange } from "./blockRange";
import {
  attachSourceEditing,
  beginSourceEditing,
  clickedOnBlockPadding,
  makePlainTextEditable,
} from "./editableSource";
import { getLocale, t } from "../../lib/i18n";

/**
 * html / svg 代码块的「预览 ↔ 源码」。
 *
 * 复用应用里块级组件的既有手势：默认显示渲染结果，点击预览即进入同位置源码编辑，
 * 焦点离开自动回到预览；右上角另给一个显式的开关按钮。状态就是既有的
 * `md-block--editing` 类，不额外维护任何状态。
 *
 * 两种预览各自选最轻的安全做法：
 * - html：沙箱 iframe。与应用的样式/脚本完全隔离，用户写的 `<style>` 能生效，
 *   而脚本被 iframe 的 sandbox 与应用的 CSP 双重挡住。
 * - svg：`<img src="data:image/svg+xml,...">`。SVG 作为图片加载时脚本不会执行，
 *   同时保留内联样式与动画，并且按原始尺寸显示（不会被放大到占满整行）。
 */

/** 预览画布跟随编辑器纸色，用户自己写的样式优先级更高 */
function canvasStyle(): { background: string; scheme: "light" | "dark" } {
  const root = document.documentElement;
  const dark = root.getAttribute("data-theme") === "dark";
  const paper = getComputedStyle(root).getPropertyValue("--bg-editor").trim();
  return {
    background: paper || (dark ? "#1e1e1e" : "#ffffff"),
    scheme: dark ? "dark" : "light",
  };
}

export function previewDocument(code: string): string {
  const { background, scheme } = canvasStyle();
  return (
    "<!doctype html><html><head><meta charset=\"utf-8\">" +
    `<style>html{background:${background};color-scheme:${scheme}}</style>` +
    `</head><body>${code}</body></html>`
  );
}

export function svgDataUrl(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

function previewButton(label: string, className: string, action: () => void) {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = label;
  element.title = label;
  element.className = className;
  element.addEventListener("mousedown", (event) => event.stopPropagation());
  element.addEventListener("click", (event) => {
    event.stopPropagation();
    action();
  });
  return element;
}

/** 高度变化（用户拖动预览框、窗口缩放）后要让 CodeMirror 重新测量，否则点击定位会偏 */
const observers = new WeakMap<HTMLElement, ResizeObserver>();

function measureLater(host: HTMLElement) {
  observers.get(host)?.disconnect();
  const observer = new ResizeObserver(() => EditorView.findFromDOM(host)?.requestMeasure());
  observer.observe(host);
  observers.set(host, observer);
}

export class HtmlPreviewWidget extends WidgetType {
  constructor(
    readonly from: number,
    readonly to: number,
    readonly code: string,
    readonly lang: string,
  ) {
    super();
  }

  /** 只比较内容与长度：位置变化不应触发 iframe 重载 */
  eq(other: HtmlPreviewWidget) {
    return (
      other.to - other.from === this.to - this.from &&
      other.code === this.code &&
      other.lang === this.lang
    );
  }

  private paint(stage: HTMLElement, code: string) {
    if (this.lang === "svg") {
      let image = stage.querySelector<HTMLImageElement>(".md-preview-image");
      if (!image) {
        image = document.createElement("img");
        image.className = "md-preview-image";
        image.alt = "";
        stage.replaceChildren(image);
      }
      if (image.dataset.renderedCode !== code) {
        image.dataset.renderedCode = code;
        image.src = svgDataUrl(code);
      }
      return;
    }

    let frame = stage.querySelector<HTMLIFrameElement>(".md-preview-frame");
    if (!frame) {
      frame = document.createElement("iframe");
      frame.className = "md-preview-frame";
      // sandbox=""：无脚本、无同源、无表单与弹窗；srcdoc 还会继承应用的 CSP
      frame.setAttribute("sandbox", "");
      frame.setAttribute("referrerpolicy", "no-referrer");
      frame.title = t(getLocale(), "editor.htmlPreview.frame");
      stage.replaceChildren(frame);
    }
    if (frame.dataset.renderedCode !== code) {
      frame.dataset.renderedCode = code;
      frame.srcdoc = previewDocument(code);
    }
  }

  toDOM() {
    const wrap = document.createElement("div");
    wrap.className = "md-preview-widget";
    stampBlockRange(wrap, this.from, this.to);

    const source = document.createElement("div");
    source.className = "md-block-source";
    makePlainTextEditable(source);
    source.textContent = this.code;

    const stage = document.createElement("div");
    stage.className = "md-html-preview md-preview-stage";
    this.paint(stage, this.code);

    const actions = document.createElement("div");
    actions.className = "md-preview-actions";
    actions.contentEditable = "false";

    const preview = () => {
      // 先按当前源码刷新预览，再交还焦点（focusout 会自动摘掉编辑态）
      this.paint(stage, source.textContent ?? "");
      wrap.classList.remove("md-block--editing");
      source.blur();
      EditorView.findFromDOM(wrap)?.requestMeasure();
    };
    actions.append(
      previewButton(t(getLocale(), "editor.htmlPreview.preview"), "md-preview-to-preview", preview),
      previewButton(
        t(getLocale(), "editor.htmlPreview.source"),
        "md-preview-to-source",
        () => beginSourceEditing(wrap, source, false),
      ),
    );

    wrap.append(actions, source, stage);
    bindBlockBoundaryCursor(wrap, stage);

    let debounce = 0;
    attachSourceEditing(wrap, {
      source: () => source,
      toMarkdown: (text) => `\`\`\`${this.lang}\n${text.replace(/\n+$/, "")}\n\`\`\``,
      onInput: (text) => {
        window.clearTimeout(debounce);
        // 编辑期间预览是隐藏的，防抖刷新即可，避免每次按键都重载
        debounce = window.setTimeout(() => this.paint(stage, text), 400);
      },
      indentOnTab: true,
    });

    // 点预览区域＝看源码，与 mermaid / 表格的就地编辑手势一致
    stage.addEventListener("mousedown", (event) => {
      if (event.button !== 0 || EditorView.findFromDOM(wrap)?.state.readOnly) return;
      event.preventDefault();
      beginSourceEditing(wrap, source, false);
    });
    stage.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        beginSourceEditing(wrap, source, false);
      }
    });
    stage.tabIndex = 0;
    stage.setAttribute("role", "group");

    measureLater(stage);
    return wrap;
  }

  updateDOM(dom: HTMLElement) {
    const source = dom.querySelector<HTMLElement>(".md-block-source");
    const stage = dom.querySelector<HTMLElement>(".md-preview-stage");
    if (!source || !stage) return false;
    stampBlockRange(dom, this.from, this.to);
    if (document.activeElement !== source) {
      if ((source.textContent ?? "") !== this.code) source.textContent = this.code;
      this.paint(stage, this.code);
    }
    return true;
  }

  destroy(dom: HTMLElement) {
    const stage = dom.querySelector<HTMLElement>(".md-preview-stage");
    if (stage) observers.get(stage)?.disconnect();
  }

  ignoreEvent(event: Event) {
    return !clickedOnBlockPadding(event, "md-preview-widget");
  }
}
