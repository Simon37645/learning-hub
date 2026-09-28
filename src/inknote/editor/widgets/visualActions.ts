import { Image as NativeImage } from "@tauri-apps/api/image";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { writeImage, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getLocale, t, type Locale, type MessageKey } from "../../lib/i18n";
import { notifyToast } from "../../lib/useToast";

const tr = (key: MessageKey) => t(getLocale(), key);
let closeViewer: (() => void) | undefined;

function button(label: string, action: () => void) {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = label;
  element.title = label;
  element.addEventListener("click", (event) => { event.stopPropagation(); action(); });
  return element;
}

function snapshot(target: HTMLElement) {
  if (target.dataset.visualReady === "false" || !target.querySelector("svg, .katex")) {
    throw new Error(tr("visual.notReady"));
  }
  const clone = target.cloneNode(true) as HTMLElement;
  const svg = target.querySelector<SVGSVGElement>(":scope > svg");
  const box = target.getBoundingClientRect();
  const viewBox = svg?.viewBox?.baseVal;
  const width = Math.max(1, viewBox?.width || target.scrollWidth || box.width);
  const height = Math.max(1, viewBox?.height || target.scrollHeight || box.height);
  const copySvg = clone.querySelector<SVGSVGElement>(":scope > svg");
  if (copySvg && viewBox?.width) {
    copySvg.style.width = `${width}px`;
    copySvg.style.height = `${height}px`;
    copySvg.style.maxWidth = "none";
  }
  const computed = getComputedStyle(target);
  Object.assign(clone.style, {
    width: `${width}px`, height: `${height}px`, maxWidth: "none", margin: "0",
    font: computed.font, color: computed.color, overflow: "visible",
  });
  return { clone, width, height };
}

export async function copyVisualImage(target: HTMLElement) {
  await document.fonts?.ready;
  const { clone, width, height } = snapshot(target);
  const holder = document.createElement("div");
  holder.className = "visual-export";
  Object.assign(holder.style, { position: "fixed", left: "-100000px", top: "0", padding: "16px", width: `${width}px` });
  holder.appendChild(clone);
  document.body.appendChild(holder);
  try {
    const { toCanvas } = await import("html-to-image");
    // Bound canvas memory for very large diagrams while keeping the entire diagram.
    const ratio = Math.min(2, 8192 / (width + 32), 8192 / (height + 32), Math.sqrt(16_000_000 / ((width + 32) * (height + 32))));
    const canvas = await toCanvas(holder, {
      width: width + 32, height: height + 32, pixelRatio: ratio,
      style: { position: "static", left: "auto", top: "auto", boxSizing: "border-box" },
      backgroundColor: getComputedStyle(document.documentElement).getPropertyValue("--bg").trim() || "#ffffff",
      preferredFontFormat: "woff2",
    });
    const context = canvas.getContext("2d");
    if (!context) throw new Error("canvas unavailable");
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
    const image = await NativeImage.new(new Uint8Array(pixels.data), canvas.width, canvas.height);
    try { await writeImage(image); } finally { await image.close(); }
  } finally {
    holder.remove();
  }
}

interface NativeViewerControls {
  locale: Locale;
  close: () => Promise<void>;
  fullscreen: () => Promise<void>;
  escape: () => Promise<void>;
}

export async function openVisualPreview(target: HTMLElement, kind: "mermaid" | "math", source: string) {
  if (!isTauri()) { openVisualViewer(target); return; }
  snapshot(target);
  await invoke("open_visual_preview", { data: {
    kind, source, locale: getLocale(), theme: document.documentElement.dataset.theme === "dark" ? "dark" : "light",
  } });
}

export function openVisualViewer(target: HTMLElement, native?: NativeViewerControls) {
  const tr = (key: MessageKey) => t(native?.locale ?? getLocale(), key);
  const { clone, width, height } = snapshot(target);
  closeViewer?.();
  const previousFocus = document.activeElement as HTMLElement | null;
  const dialog = document.createElement("dialog");
  dialog.className = "visual-viewer";
  if (native) dialog.classList.add("visual-viewer--native");
  dialog.setAttribute("aria-label", tr("visual.viewer"));
  const header = document.createElement("div");
  header.className = "visual-viewer-header";
  const title = document.createElement("strong");
  title.textContent = tr("visual.viewer");
  const controls = document.createElement("div");
  controls.className = "visual-viewer-controls";
  const percent = document.createElement("output");
  const viewport = document.createElement("div");
  viewport.className = "visual-viewer-viewport";
  viewport.addEventListener("click", (event) => event.preventDefault());
  const centering = document.createElement("div");
  centering.className = "visual-viewer-centering";
  const plane = document.createElement("div");
  plane.className = "visual-viewer-plane";
  clone.style.transformOrigin = "0 0";
  clone.style.position = "absolute";
  plane.appendChild(clone);
  centering.appendChild(plane);
  viewport.appendChild(centering);
  let scale = 1;
  const zoom = (next: number) => {
    scale = Math.min(8, Math.max(0.01, next));
    plane.style.width = `${width * scale}px`;
    plane.style.height = `${height * scale}px`;
    clone.style.transform = `scale(${scale})`;
    percent.textContent = `${Math.round(scale * 100)}%`;
  };
  const fit = () => {
    zoom(Math.min(1, Math.max(1, viewport.clientWidth - 48) / width, Math.max(1, viewport.clientHeight - 48) / height));
    viewport.scrollTo(0, 0);
  };
  const cleanup = () => {
    observer.disconnect();
    window.removeEventListener("resize", fit);
    dialog.remove();
    if (closeViewer === close) closeViewer = undefined;
    if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
  };
  const nativeAction = (action: () => Promise<void>) => {
    void action().catch(() => { hint.textContent = tr("visual.failed"); });
  };
  const close = () => {
    if (native) { nativeAction(native.close); return; }
    dialog.close(); cleanup();
  };
  const observer = new MutationObserver(() => { if (!target.isConnected) close(); });
  controls.append(
    button(tr("visual.zoomOut"), () => zoom(scale / 1.25)), percent,
    button(tr("visual.zoomIn"), () => zoom(scale * 1.25)),
    ...(!native ? [button("100%", () => zoom(1)), button(tr("visual.fit"), fit), button(tr("dialog.close"), close)] : []),
  );
  header.append(title, controls);
  const hint = document.createElement("p");
  hint.className = "visual-viewer-hint";
  hint.textContent = tr(native ? "visual.nativeHint" : "visual.hint");
  dialog.append(header, viewport, hint);
  dialog.addEventListener("cancel", (event) => { event.preventDefault(); if (native) nativeAction(native.escape); else close(); });
  dialog.addEventListener("click", (event) => { if (event.target === dialog) close(); });
  // Do not let editor/global shortcuts consume the viewer's interactions.
  dialog.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (native && (event.key === "F11" || (event.key.toLowerCase() === "f" && event.ctrlKey && event.metaKey))) {
      event.preventDefault();
      nativeAction(native.fullscreen);
    }
  });
  viewport.addEventListener("wheel", (event) => {
    event.preventDefault();
    zoom(scale * (event.deltaY < 0 ? 1.1 : 1 / 1.1));
  }, { passive: false });
  let drag: { x: number; y: number; left: number; top: number } | null = null;
  viewport.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    drag = { x: event.clientX, y: event.clientY, left: viewport.scrollLeft, top: viewport.scrollTop };
    viewport.setPointerCapture(event.pointerId);
    viewport.classList.add("is-dragging");
  });
  viewport.addEventListener("pointermove", (event) => {
    if (!drag) return;
    viewport.scrollLeft = drag.left + drag.x - event.clientX;
    viewport.scrollTop = drag.top + drag.y - event.clientY;
  });
  const endDrag = () => { drag = null; viewport.classList.remove("is-dragging"); };
  viewport.addEventListener("pointerup", endDrag);
  viewport.addEventListener("pointercancel", endDrag);
  viewport.addEventListener("lostpointercapture", endDrag);
  document.body.appendChild(dialog);
  dialog.showModal();
  fit();
  observer.observe(document.body, { childList: true, subtree: true });
  window.addEventListener("resize", fit);
  closeViewer = close;
}

export function attachVisualActions(host: HTMLElement, target: HTMLElement, source: () => string, kind: "mermaid" | "math", inline = false) {
  host.classList.add("md-visual-actions-host");
  host.tabIndex = 0;
  if (inline) {
    host.classList.add("md-visual-inline-host");
    host.setAttribute("aria-label", tr("visual.viewer"));
  }
  const toolbar = document.createElement("span");
  toolbar.className = "md-visual-actions";
  toolbar.contentEditable = "false";
  const run = async (action: () => void | Promise<void>, copy = false) => {
    const buttons = toolbar.querySelectorAll("button");
    buttons.forEach((item) => { item.disabled = true; });
    try { await action(); if (copy) notifyToast(tr("editor.code.copied"), "success"); }
    catch (error) { notifyToast(error instanceof Error && error.message === tr("visual.notReady") ? error.message : tr("visual.failed"), "error"); }
    finally { buttons.forEach((item) => { item.disabled = false; }); }
  };
  toolbar.addEventListener("mousedown", (event) => { event.preventDefault(); event.stopPropagation(); });
  toolbar.addEventListener("keydown", (event) => event.stopPropagation());
  toolbar.addEventListener("contextmenu", (event) => { event.preventDefault(); event.stopPropagation(); });
  toolbar.append(
    button(tr("visual.enlarge"), () => void run(() => openVisualPreview(target, kind, source()))),
    button(tr("visual.copyImage"), () => void run(() => copyVisualImage(target), true)),
    button(tr(kind === "mermaid" ? "visual.copyMarkdown" : "visual.copyLatex"), () => void run(() => writeText(kind === "mermaid" ? `\`\`\`mermaid\n${source().replace(/\n+$/, "")}\n\`\`\`` : source()), true)),
  );
  host.appendChild(toolbar);
}
