import { describe, expect, it } from "vitest";
import { previewDocument, svgDataUrl } from "./htmlPreview";

describe("html / svg 预览", () => {
  it("把代码原样包进最小文档，脚本交给 sandbox 拦，不在这一层删", () => {
    const html = '<div style="color:red">hi</div><script>alert(1)</script>';
    const doc = previewDocument(html);
    expect(doc.startsWith("<!doctype html>")).toBe(true);
    expect(doc).toContain(html);
    // 预览靠 iframe sandbox 挡脚本；这里若把 script 删掉，源码与预览就不一致了
    expect(doc).toContain("<script>");
  });

  it("svg 走 data URL，需要转义的字符必须编码", () => {
    const svg = '<svg viewBox="0 0 10 10"><rect fill="#f2e08f" /></svg>';
    const url = svgDataUrl(svg);
    expect(url.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
    // # 若不编码，data URL 会从这里截断，SVG 直接渲染失败
    expect(url).not.toContain("#");
    expect(decodeURIComponent(url.split(",", 2)[1])).toBe(svg);
  });
});
