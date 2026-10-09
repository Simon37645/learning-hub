// PDF 渲染：pdf.js + 惰性画布 + 分页文本抽取（喂给 agent）。
//
// 为什么不用 <iframe src="file.pdf">：WebView2 的内置 PDF 阅读器无法被脚本控制，
// agent 也就没法「翻到第 3 页并读那一段」。自己画才能让 agent 真正操作文档。
//
// 尺寸策略：画布只画一次，**按固定 2 倍栅格渲染**，显示尺寸完全交给 CSS。
// 这样就不需要在 JS 里测量容器宽度、也不需要在缩放时重绘——
// 「测量 → 重绘」这条链路在面板动态出现/改变宽度时很容易拿到 0 或旧值。

import { useEffect, useRef, useState } from "react";
import * as pdfjs from "pdfjs-dist";
import workerSrc from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { api, errText } from "../lib/api";
import { forgetPdfDoc } from "../lib/pdfshot";
import type { PageText, TabView } from "../lib/types";
import { registerSnapshotProvider, useApp } from "../store/app";
import { Icon, Spinner } from "./ui";

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc;

/** 单份文档最多抽取多少页文本（防止超大 PDF 卡住） */
const MAX_TEXT_PAGES = 400;
/** 栅格化倍率：2 倍在 100% 缩放下足够清晰，放大到 200% 会略软 */
const RENDER_SCALE = 2;

export function PdfView({ tab, highlight }: { tab: TabView; highlight: string | null }) {
  const [scrollEl, setScrollEl] = useState<HTMLDivElement | null>(null);
  const [doc, setDoc] = useState<pdfjs.PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [zoom, setZoom] = useState(0);
  const [visible, setVisible] = useState(tab.page);
  const reloadSeq = useApp((s) => s.reloadSeq[tab.id] ?? 0);
  const pagesRef = useRef<PageText[]>([]);
  const [textProgress, setTextProgress] = useState<{ done: number; total: number } | null>(null);

  // 载入文档
  useEffect(() => {
    let cancelled = false;
    setDoc(null);
    setError(null);
    pagesRef.current = [];
    // 文档被重新加载：pdf_screenshot 那边的文档缓存也要丢掉，否则截到的是旧版本
    forgetPdfDoc(tab.id);
    (async () => {
      try {
        const bytes = await api.viewerLoadBytes(tab.id);
        const task = pdfjs.getDocument({
          data: new Uint8Array(bytes),
          useSystemFonts: true,
        });
        const pdf = await task.promise;
        if (cancelled) return;
        setDoc(pdf);
        void api.viewerReportState(tab.id, 1, 0, pdf.numPages);
        void extractAllText(pdf);
      } catch (e) {
        if (!cancelled) {
          const msg = errText(e);
          setError(msg);
          void api.viewerReportSnapshot(tab.id, { error: msg });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.id, reloadSeq]);

  /** 逐页抽文本交给 Rust（agent 读到的是这份） */
  async function extractAllText(pdf: pdfjs.PDFDocumentProxy) {
    const total = Math.min(pdf.numPages, MAX_TEXT_PAGES);
    setTextProgress({ done: 0, total });
    const pages: PageText[] = [];
    for (let i = 1; i <= total; i++) {
      try {
        const page = await pdf.getPage(i);
        const content = await page.getTextContent();
        // 按 y 坐标断行，否则整页文字会连成一行
        let lastY: number | null = null;
        let buf = "";
        for (const item of content.items as { str?: string; transform?: number[] }[]) {
          if (typeof item.str !== "string") continue;
          const y = item.transform?.[5];
          if (lastY !== null && y !== undefined && Math.abs(y - lastY) > 2) buf += "\n";
          buf += item.str;
          if (y !== undefined) lastY = y;
        }
        pages.push({ page: i, text: buf.replace(/[ \t]+/g, " ").trim() });
      } catch {
        pages.push({ page: i, text: "" });
      }
      if (i % 5 === 0 || i === total) {
        pagesRef.current = pages;
        registerSnapshotProvider(tab.id, () => ({ pages, totalPages: pdf.numPages }));
      }
    }
    pagesRef.current = pages;
    registerSnapshotProvider(tab.id, () => ({ pages, totalPages: pdf.numPages }));
    setTextProgress(null);
    await api.viewerReportSnapshot(tab.id, { pages, totalPages: pdf.numPages });
  }

  useEffect(() => {
    return () => {
      registerSnapshotProvider(tab.id, null);
    };
  }, [tab.id]);

  // 跟踪当前可见页，回报给 Rust（agent 据此知道「用户在看第几页」）
  useEffect(() => {
    const root = scrollEl;
    if (!root || !doc) return;
    const onScroll = () => {
      const children = Array.from(root.querySelectorAll<HTMLElement>("[data-page]"));
      const mid = root.getBoundingClientRect().top + root.clientHeight * 0.35;
      let current = 1;
      for (const c of children) {
        if (c.getBoundingClientRect().top <= mid) current = Number(c.dataset.page);
      }
      if (current !== visible) {
        setVisible(current);
        void api.viewerReportState(tab.id, current, root.scrollTop / Math.max(1, root.scrollHeight));
      }
    };
    root.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
    return () => root.removeEventListener("scroll", onScroll);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc, visible, tab.id, scrollEl]);

  // agent 要求跳到某页
  useEffect(() => {
    if (!doc) return;
    const el = scrollEl?.querySelector<HTMLElement>(`[data-page="${tab.page}"]`);
    if (el) el.scrollIntoView({ block: "start" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.page, doc, scrollEl]);

  if (error) {
    return (
      <div className="viewer-note">
        这份 PDF 打不开：{error}
        <div style={{ marginTop: 8 }}>
          <button className="btn sm" onClick={() => api.openWithSystem(tab.topicSlug ?? "", tab.path ?? "")}>
            用系统程序打开
          </button>
        </div>
      </div>
    );
  }

  if (!doc) {
    return (
      <div className="empty" style={{ height: "100%" }}>
        <Spinner />
        <div>正在读取 PDF…</div>
      </div>
    );
  }

  // CSS 宽度系数：0 表示自适应面板宽度，正数表示固定缩放
  const pageStyle =
    zoom === 0
      ? undefined
      : { width: `${zoom * 100}%`, minWidth: 200, maxWidth: "none" as const };

  return (
    <div className="viewer-scroll" ref={setScrollEl} style={{ overflow: "auto" }}>
      {textProgress && (
        <div className="viewer-note" style={{ position: "sticky", top: 8, zIndex: 5 }}>
          正在把全文交给 agent 阅读… {textProgress.done}/{textProgress.total} 页
        </div>
      )}
      <div className="pdf-stage">
        {Array.from({ length: doc.numPages }, (_, i) => (
          <PdfPage
            key={i + 1}
            doc={doc}
            pageNo={i + 1}
            scrollRoot={scrollEl}
            style={pageStyle}
            highlight={visible === i + 1 ? highlight : null}
          />
        ))}
      </div>

      <div style={{ position: "sticky", bottom: 10, display: "flex", justifyContent: "center", pointerEvents: "none" }}>
        <div
          className="row"
          style={{
            pointerEvents: "auto",
            background: "var(--bg)",
            border: "1px solid var(--border)",
            borderRadius: 999,
            padding: "3px 6px",
            boxShadow: "var(--shadow-sm)",
            gap: 4,
          }}
        >
          <button className="icon-btn" title="缩小" onClick={() => setZoom((z) => Math.max(0, (z === 0 ? 1 : z) - 0.15))}>
            <Icon name="zoom-out" size={13} />
          </button>
          <button className="mono" style={{ border: "none", background: "none", cursor: "pointer", minWidth: 46 }} onClick={() => setZoom(0)} title="点击回到自适应宽度">
            {zoom === 0 ? "自适应" : `${Math.round(zoom * 100)}%`}
          </button>
          <button className="icon-btn" title="放大" onClick={() => setZoom((z) => Math.min(4, (z === 0 ? 1 : z) + 0.15))}>
            <Icon name="zoom-in" size={13} />
          </button>
        </div>
      </div>
    </div>
  );
}

function PdfPage({
  doc,
  pageNo,
  scrollRoot,
  style,
  highlight,
}: {
  doc: pdfjs.PDFDocumentProxy;
  pageNo: number;
  scrollRoot: HTMLDivElement | null;
  style?: React.CSSProperties;
  highlight: string | null;
}) {
  const holderRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const textLayerRef = useRef<pdfjs.TextLayer | null>(null);
  const [rendered, setRendered] = useState(false);
  const [aspect, setAspect] = useState(1.294); // A4 比例，未渲染时先占位
  const [matchCount, setMatchCount] = useState(0);

  // 进入视口附近才真正渲染
  useEffect(() => {
    const holder = holderRef.current;
    if (!holder || rendered) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        io.disconnect();
        void render();
      },
      { root: scrollRoot, rootMargin: "900px 0px" },
    );
    io.observe(holder);
    return () => io.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc, pageNo, rendered, scrollRoot]);

  /** 画布只画一次（固定 2 倍栅格），显示尺寸交给 CSS */
  async function render() {
    const canvas = canvasRef.current;
    if (!canvas) return;
    try {
      const page = await doc.getPage(pageNo);
      const viewport = page.getViewport({ scale: RENDER_SCALE });
      canvas.width = Math.floor(viewport.width);
      canvas.height = Math.floor(viewport.height);
      setAspect(viewport.height / viewport.width);

      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      await page.render({ canvasContext: ctx, viewport }).promise;
      setRendered(true);
      // 文字层要等画布排好版才能算出显示缩放
      requestAnimationFrame(() => void buildTextLayer());
    } catch {
      // 单页失败不影响其它页
    }
  }

  /**
   * 叠一层透明文字，让用户可以选中 / 复制。
   * pdf.js 的 TextLayer 用视口算每段文字的绝对位置，所以必须传「显示尺寸」对应的视口，
   * 而不是画布的 2 倍栅格视口。
   */
  async function buildTextLayer() {
    const holder = holderRef.current;
    const container = layerRef.current;
    if (!holder || !container) return;
    const baseWidth = (await doc.getPage(pageNo)).getViewport({ scale: 1 }).width;
    const displayWidth = holder.clientWidth;
    if (displayWidth < 40 || baseWidth <= 0) return;
    const scale = displayWidth / baseWidth;

    try {
      const page = await doc.getPage(pageNo);
      const viewport = page.getViewport({ scale });
      if (textLayerRef.current) {
        textLayerRef.current.update({ viewport });
        return;
      }
      const layer = new pdfjs.TextLayer({
        textContentSource: await page.getTextContent(),
        container,
        viewport,
      });
      textLayerRef.current = layer;
      await layer.render();
      applyHighlight();
    } catch {
      // 文字层失败不影响看 PDF
    }
  }

  /** 把检索词在文字层里标黄（比在页面上盖一个角标精确得多） */
  function applyHighlight() {
    const layer = textLayerRef.current;
    if (!layer) return;
    const q = (highlight ?? "").trim().toLowerCase();
    let hits = 0;
    for (const div of layer.textDivs) {
      div.classList.remove("highlight", "begin", "end");
      if (!q) continue;
      if ((div.textContent ?? "").toLowerCase().includes(q)) {
        div.classList.add("highlight");
        hits++;
      }
    }
    setMatchCount(hits);
  }

  // 面板宽度 / 缩放变化时，文字层也要跟着重排
  useEffect(() => {
    const holder = holderRef.current;
    if (!holder || !rendered) return;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => void buildTextLayer());
    });
    ro.observe(holder);
    return () => {
      ro.disconnect();
      cancelAnimationFrame(raf);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rendered, doc, pageNo]);

  // 卸载时释放文字层
  useEffect(() => {
    return () => {
      textLayerRef.current?.cancel();
      textLayerRef.current = null;
    };
  }, []);

  useEffect(() => {
    applyHighlight();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlight, rendered]);

  return (
    <div
      className="pdf-page"
      data-page={pageNo}
      ref={holderRef}
      style={{ aspectRatio: `${1} / ${aspect}`, ...style }}
    >
      <canvas ref={canvasRef} style={{ opacity: rendered ? 1 : 0 }} />
      <div className="textLayer" ref={layerRef} />
      {!rendered && (
        <div
          style={{
            position: "absolute",
            inset: 0,
            display: "grid",
            placeItems: "center",
            color: "var(--text-faint)",
            fontSize: 12,
          }}
        >
          第 {pageNo} 页
        </div>
      )}
      {matchCount > 0 && (
        <div className="pdf-hit-badge">命中「{highlight}」×{matchCount}</div>
      )}
    </div>
  );
}
