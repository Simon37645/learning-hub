import { describe, expect, it } from "vitest";
import { demoDocument, renderDemoBlocksIn } from "./markdown";

// 演示卡片（agent 讲难懂内容时写的 ```html / ```svg）的纯逻辑：
// 「怎么包成文档」和「哪些代码块会被换成卡片」。真机上的渲染（iframe 里画成什么样）
// 靠 CDP 验（见 AGENTS.md 的验证方式）——这里守的是最容易写错的两条：
// 沙箱必须不给脚本、以及别把普通代码块也换掉。

function container(codeHtml: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = codeHtml;
  document.body.appendChild(root);
  return root;
}

describe("demoDocument", () => {
  it("html 原样交给 iframe（演示页自带 <html> 或片段都能渲染）", () => {
    const src = "<!doctype html><html><body><b>hi</b></body></html>";
    expect(demoDocument("html", src)).toBe(src);
  });

  it("svg 补一层 HTML 壳：居中、限宽、不留滚动条", () => {
    const doc = demoDocument("svg", '<svg viewBox="0 0 10 10"><circle r="4" /></svg>');
    expect(doc).toContain("<!doctype html>");
    expect(doc).toContain("place-items:center");
    expect(doc).toContain('<circle r="4" />');
  });
});

describe("renderDemoBlocksIn", () => {
  it("把 ```html 代码块换成沙箱卡片，并保留源码", () => {
    const root = container(
      '<p>看这个</p><pre><code class="language-html">&lt;div id="demo"&gt;x&lt;/div&gt;</code></pre>',
    );
    const n = renderDemoBlocksIn(root);

    expect(n).toBe(1);
    const card = root.querySelector<HTMLElement>(".demo-card");
    expect(card).toBeTruthy();
    const frame = card!.querySelector<HTMLIFrameElement>("iframe.demo-frame");
    // 沙箱是空字符串：不许脚本、不许同源——模型产出的 HTML 只被当成画面
    expect(frame!.getAttribute("sandbox")).toBe("");
    expect(frame!.srcdoc).toContain('<div id="demo">x</div>');
    // 源码留在卡片里（默认收起，点「源码」展开）
    const pre = card!.querySelector<HTMLPreElement>("pre");
    expect(pre!.hidden).toBe(true);
    expect(pre!.textContent).toContain('<div id="demo">x</div>');
    // 重复调用不会套娃（React 重渲染时会再跑一次）
    expect(renderDemoBlocksIn(root)).toBe(0);
    expect(root.querySelectorAll(".demo-card").length).toBe(1);
  });

  it("带 <script> 的演示要标出「脚本已禁用」", () => {
    const root = container(
      '<pre><code class="language-html">&lt;script&gt;alert(1)&lt;/script&gt;</code></pre>',
    );
    renderDemoBlocksIn(root);
    expect(root.querySelector(".demo-warn")?.textContent).toContain("脚本已禁用");
  });

  it("```svg 也渲染成卡片，普通代码块不动", () => {
    const root = container(
      '<pre><code class="language-svg">&lt;svg/&gt;</code></pre>' +
        '<pre><code class="language-python">print(1)</code></pre>',
    );
    expect(renderDemoBlocksIn(root)).toBe(1);
    expect(root.querySelectorAll(".demo-card").length).toBe(1);
    expect(root.querySelectorAll("pre").length).toBe(2); // 两张都还在（一张收进卡片）
    expect(root.querySelector<HTMLElement>(".demo-card")!.dataset.lang).toBe("svg");
  });

  it("空代码块不生成卡片（模型常常先写个围栏再补内容）", () => {
    const root = container('<pre><code class="language-html">   </code></pre>');
    expect(renderDemoBlocksIn(root)).toBe(0);
    expect(root.querySelector(".demo-card")).toBeNull();
  });
});
