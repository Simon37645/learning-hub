// 造一个最小可用 PDF（纯 Node，无依赖），用来验证内置浏览器的 PDF 通道。
//
// 生成的 PDF 带真实文字层，所以 pdf.js 能渲染、agent 也能抽取到文本。
// 用法：node scripts/make-demo-pdf.mjs <输出路径> [标题]

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

const out = resolve(process.argv[2] ?? "demo.pdf");
const title = process.argv[3] ?? "Demo Document";

// PDF 的 WinAnsi 编码装不下中文，这里用英文写，反正只是用来验证通道
const PAGE_1 = [
  "Learning Hub - Demo PDF",
  "================================",
  "",
  "1. What this file is for",
  "   This PDF exists so the built-in browser has something real to render,",
  "   and so the agent can extract a text layer and cite page numbers.",
  "",
  "2. How to check it works",
  "   - Open it from the Materials pane (click the file name)",
  "   - Ask the agent to summarize page 1",
  "   - Ask the agent to search a keyword and jump to the match",
  "",
  "3. Why not just use an iframe",
  "   An embedded native viewer cannot be scripted, so the agent could not",
  "   turn pages or read the text. Rendering it ourselves with pdf.js keeps",
  "   the document under both the user's and the agent's control.",
  "",
  "Page 1 of 2",
];

const PAGE_2 = [
  "4. Scheduling notes",
  "   Cards move through an SM-2 style scheduler:",
  "   Again -> 10 minutes, Good -> 1 day, then 3 days, then interval * ease.",
  "",
  "5. Data layout on disk",
  "   <workspace>/<topic>/notes        markdown notes",
  "   <workspace>/<topic>/materials    pdf, images, archives",
  "   <workspace>/<topic>/cards        cards.jsonl",
  "   <workspace>/<topic>/plan         tasks.jsonl",
  "   <workspace>/<topic>/sessions     study sessions",
  "",
  "Page 2 of 2",
];

const escapeText = (s) => s.replace(/\\/g, "\\\\").replace(/\(/g, "\\(").replace(/\)/g, "\\)");

/** 一页的内容流：浅色底 + 深色文字，每行 16pt 递增 */
function contentStream(lines) {
  const body = lines
    .map((l, i) => `BT /F1 11 Tf 1 0 0 1 64 ${740 - i * 16} Tm (${escapeText(l)}) Tj ET`)
    .join("\n");
  return `q 0.98 0.98 0.96 rg 0 0 612 792 re f Q\nq 0.15 0.15 0.16 rg\n${body}\nQ`;
}

const pages = [PAGE_1, PAGE_2].map(contentStream);

// ---- 组装对象：1=Catalog 2=Pages 3=Font 之后每页 2 个（内容流 + 页对象） ----
const objects = [];
const push = (body) => {
  objects.push(body);
  return objects.length;
};

const catId = push("");
const pgRootId = push("");
const fontId = push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");

const pageIds = [];
const contentIds = [];
for (const stream of pages) {
  contentIds.push(push(`<< /Length ${Buffer.byteLength(stream, "latin1")} >>\nstream\n${stream}\nendstream`));
  pageIds.push(push(""));
}

objects[catId - 1] = `<< /Type /Catalog /Pages ${pgRootId} 0 R >>`;
objects[pgRootId - 1] =
  `<< /Type /Pages /Kids [${pageIds.map((id) => `${id} 0 R`).join(" ")}] /Count ${pageIds.length} >>`;
pageIds.forEach((pId, i) => {
  objects[pId - 1] =
    `<< /Type /Page /Parent ${pgRootId} 0 R /MediaBox [0 0 612 792] ` +
    `/Resources << /Font << /F1 ${fontId} 0 R >> >> /Contents ${contentIds[i]} 0 R >>`;
});

// ---- 序列化并记录每个对象的字节偏移（xref 表要用） ----
let pdf = "%PDF-1.4\n%\xE2\xE3\xCF\xD3\n";
const offsets = [0];
objects.forEach((body, i) => {
  offsets.push(Buffer.byteLength(pdf, "latin1"));
  pdf += `${i + 1} 0 obj\n${body}\nendobj\n`;
});

const xrefStart = Buffer.byteLength(pdf, "latin1");
pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
for (let i = 1; i <= objects.length; i++) {
  pdf += `${String(offsets[i]).padStart(10, "0")} 00000 n \n`;
}
pdf +=
  `trailer\n<< /Size ${objects.length + 1} /Root ${catId} 0 R ` +
  `/Info << /Title (${escapeText(title)}) >> >>\nstartxref\n${xrefStart}\n%%EOF\n`;

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, Buffer.from(pdf, "latin1"));
console.log(`PDF written: ${out} (${Buffer.byteLength(pdf, "latin1")} bytes, ${pages.length} pages)`);
