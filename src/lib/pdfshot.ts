// PDF 单页栅格化：给 agent 的 `pdf_screenshot` 工具用。
//
// 为什么在前端做：PDF 是 pdf.js 在界面上画的（见 PdfView.tsx 的说明），Rust 侧没有渲染器。
// 这个模块**不依赖 PdfView 是否挂载**——它自己按 tab id 取字节、自己开文档、自己画到离屏画布，
// 所以用户把内置浏览器收起来、或者在看别的标签页时，agent 照样能看到某一页的图。
//
// 为什么要缓存文档：agent 常连着截好几页（「这几页的图我都看一下」），
// 每页都重新读盘 + 解析整份 PDF 太亏；解析一次按 tab 缓存，换页只重画。
// 文档被重新加载（viewerReload）时用 forgetPdfDoc 清掉，免得截到旧版本的页。

import * as pdfjs from "pdfjs-dist";
import workerSrc from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { api } from "./api";

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc;

/** 目标长边像素。和贴图的上限一致：再大只会更贵，不会更清楚 */
const MAX_EDGE = 1568;
/** 同时缓存几份文档（超过就丢最老的，避免看了一堆讲义后内存一直涨） */
const MAX_DOCS = 4;

const docs = new Map<string, Promise<pdfjs.PDFDocumentProxy>>();

export interface RenderedPageShot {
  /** PNG 的 base64（不带 data URL 前缀） */
  data: string;
  width: number;
  height: number;
}

function loadDoc(tabId: string): Promise<pdfjs.PDFDocumentProxy> {
  const cached = docs.get(tabId);
  if (cached) return cached;

  const task = api
    .viewerLoadBytes(tabId)
    .then((bytes) => pdfjs.getDocument({ data: new Uint8Array(bytes), useSystemFonts: true }).promise)
    .catch((e) => {
      // 失败不进缓存：用户重开一次标签就该能好
      docs.delete(tabId);
      throw e;
    });

  docs.set(tabId, task);
  while (docs.size > MAX_DOCS) {
    const oldest = docs.keys().next().value;
    if (oldest === undefined || oldest === tabId) break;
    void docs.get(oldest)?.then((d) => d.destroy()).catch(() => {});
    docs.delete(oldest);
  }
  return task;
}

/** 丢掉某个标签的文档缓存（文档被重新加载后调用）。 */
export function forgetPdfDoc(tabId: string): void {
  const cached = docs.get(tabId);
  docs.delete(tabId);
  void cached?.then((d) => d.destroy()).catch(() => {});
}

/**
 * 把某一页渲染成 PNG。
 *
 * `scale` 不传（或 ≤0）时按「**长边** MAX_EDGE 像素」自己算倍率——和贴图的上限一个口径
 * （见 `images.ts` 的 fitWithin）：固定倍率会让大页偏大、小页偏小；
 * 而按宽算的话，竖版 A4（595×842pt）会得到 1568×2218，比服务商自己允许的还大，
 * 传过去也是被它按长边缩回 1568，白花流量。
 */
export async function renderPdfPage(tabId: string, page: number, scale?: number): Promise<RenderedPageShot> {
  const doc = await loadDoc(tabId);
  if (!Number.isFinite(page) || page < 1 || page > doc.numPages) {
    throw new Error(`这份 PDF 一共 ${doc.numPages} 页，没有第 ${page} 页`);
  }
  const pdfPage = await doc.getPage(page);
  const base = pdfPage.getViewport({ scale: 1 });
  const longEdge = Math.max(1, base.width, base.height);
  const k = scale && scale > 0 ? scale : Math.min(3, Math.max(0.6, MAX_EDGE / longEdge));
  const viewport = pdfPage.getViewport({ scale: k });

  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.floor(viewport.width));
  canvas.height = Math.max(1, Math.floor(viewport.height));
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("拿不到画布，无法渲染这一页");

  // 先铺白底：PDF 页本身是透明的，不铺的话透明区域会变成黑块，
  // 模型看图会把整页当成黑底（深色主题下尤其明显）。
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  await pdfPage.render({ canvasContext: ctx, viewport }).promise;

  const url = canvas.toDataURL("image/png");
  const comma = url.indexOf(",");
  return { data: comma >= 0 ? url.slice(comma + 1) : url, width: canvas.width, height: canvas.height };
}

// 开发模式下挂到 window：`node scripts/cdp.mjs eval` 能直接验证渲染链路
// （和 store/app.ts 里那个 __hub 一个用途）。打包版没有这个入口。
if (import.meta.env.DEV) {
  (window as unknown as { __hubPdf?: unknown }).__hubPdf = { renderPdfPage, forgetPdfDoc };
}
