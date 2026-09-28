// 「复制为 HTML」用的渲染入口。
//
// 原实现走 unified/remark/rehype 全套（10 个包、近 300 行）只为这一个动作；
// 学习中枢已经有一条 marked + KaTeX + DOMPurify + highlight.js 的渲染管线，
// 这里直接复用它，避免同一个应用里维护两套 Markdown 渲染。

import { renderMarkdown } from "../../lib/markdown";

export async function markdownToBodyHtml(markdown: string): Promise<string> {
  return renderMarkdown(markdown);
}
