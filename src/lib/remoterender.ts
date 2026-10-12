// 替**手机页面**渲染图（目前只有 Mermaid）。
//
// 手机上不装 mermaid：桌面端已经有唯一一份渲染入口（`inknote/lib/mermaid.ts`
// 的 `renderDiagram`，负责唯一 id、串行队列、错误抑制）。远程服务发
// `hub://remote` 过来，这里画完把 SVG 交回去（`remote_report_render`）。
//
// 和 `lib/pdfshot.ts` 同一套路（那边是「替 agent 渲染 PDF 页」），
// 共同点是**两端都要回话**：画不出来也要回，否则手机那边要等到超时。

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "./api";
import { renderDiagram } from "../inknote/lib/mermaid";

/** Rust 侧发过来的渲染请求（字段名见 `RemoteRenderRequest`）。 */
interface RemoteRenderRequest {
  id: string;
  lang: string;
  source: string;
}

/** 手机页面的明暗跟着它自己的偏好走，但图得和桌面端当前配色一致——
 *  这里直接读桌面端自己的主题（图是画在桌面端这份渲染器上的）。 */
function currentTheme(): "dark" | "default" {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "dark") return "dark";
  if (attr === "light") return "default";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "default";
}

/**
 * 注册渲染代理。**幂等**：`bootstrapStore` 有 initPromise 兜着，
 * 但这里再挡一道，免得将来别处调用时重复注册（StrictMode 会跑两遍 effect）。
 */
let listening: Promise<UnlistenFn> | null = null;

export function registerRemoteRenderer(): Promise<UnlistenFn> {
  listening ??= listen<RemoteRenderRequest>("hub://remote", async (event) => {
    const { id, lang, source } = event.payload;
    if (lang !== "mermaid") {
      await api.remoteReportRender(id, false, `画不了这种图：${lang}`).catch(() => {});
      return;
    }
    try {
      const svg = await renderDiagram(source, currentTheme());
      await api.remoteReportRender(id, true, svg);
    } catch (e) {
      // 错误文案给手机看，写清「是这张图的问题」而不是「应用坏了」
      const message = e instanceof Error ? e.message : String(e);
      await api.remoteReportRender(id, false, message.slice(0, 300)).catch(() => {});
    }
  });
  return listening;
}
