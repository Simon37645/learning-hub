// 生成 README 用的界面截图。
//
// 前提：应用带远程调试端口启动（WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222），
// 并且当前工作区里有示例主题（npm run demo:seed）。
//
// 用法：node scripts/shots.mjs [场景名...]     不传就全拍
//
// 为什么单独写一个脚本：README 的图必须和真实界面一致，
// 手截图容易拍到中间态（加载中、弹窗半开），脚本化之后重复生成也不会漂。

import fs from "node:fs";
import { connect } from "./cdp.mjs";

const OUT = "docs/images";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 每个场景：先做一串动作，再截图 */
/** 记录已产出的截图大小，用来识别「抓到同一帧」 */
const seen = new Set();

const SCENES = {
  home: async (p) => {
    await clickText(p, ".side-item", "首页");
    await sleep(1500);
    await p.shot(`${OUT}/home.png`);
    await p.shot(`${OUT}/hero.png`);
  },
  chat: async (p) => {
    await openTopic(p);
    await clickText(p, ".segmented button", "对话");
    await sleep(1200);
    // 滚到对话末尾，让最后一段（工具卡片 + 正文）完整入镜
    await p.eval(`(() => { const el = document.querySelector('.chat-scroll'); if (el) el.scrollTop = el.scrollHeight; return true; })()`);
    await sleep(900);
    await p.shot(`${OUT}/chat.png`);
  },
  viewer: async (p) => {
    await ensureWorkbench(p);
    await openTopic(p);
    await clickText(p, ".segmented button", "工作台");
    await sleep(600);
    await clickText(p, ".wb-tab", "资料");
    await sleep(900);
    // 打开 PDF
    await p.eval(`(() => { const r = document.querySelector('.wb-pane .list-row'); if (r) r.click(); return !!r; })()`);
    await sleep(5000);
    await p.shot(`${OUT}/viewer.png`);
  },
  mindmap: async (p) => {
    await ensureWorkbench(p);
    await openTopic(p);
    await clickText(p, ".segmented button", "工作台");
    await sleep(500);
    await resetNoteList(p);
    await p.eval(`(() => { const r = Array.from(document.querySelectorAll('.wb-pane .list-row')).find(x => x.innerText.includes('知识框架')); if (r) r.click(); return !!r; })()`);
    await sleep(4500);
    await p.shot(`${OUT}/mindmap.png`);
  },
  editor: async (p) => {
    await ensureWorkbench(p);
    await resetNoteList(p);
    await p.eval(`(() => { const r = Array.from(document.querySelectorAll('.wb-pane .list-row')).find(x => x.innerText.includes('特征值与特征向量')); if (r) r.click(); return !!r; })()`);
    await sleep(4000);
    await p.shot(`${OUT}/editor.png`);
  },
  lesson: async (p) => {
    await ensureWorkbench(p);
    await clickText(p, ".wb-tab", "概览");
    await sleep(1200);
    await p.shot(`${OUT}/lesson.png`);
  },
  cards: async (p) => {
    await ensureWorkbench(p);
    await clickText(p, ".wb-tab", "卡片");
    await sleep(1100);
    await clickText(p, ".btn", "开始复习");
    await sleep(800);
    await clickText(p, ".btn", "显示答案");
    await sleep(700);
    await p.shot(`${OUT}/cards.png`);
  },
  quiz: async (p) => {
    await ensureWorkbench(p);
    await clickText(p, ".wb-tab", "测验");
    await sleep(1100);
    // 打开试卷
    await p.eval(`(() => { const r = document.querySelector('.wb-pane .list-row'); if (r) r.click(); return !!r; })()`);
    await sleep(1200);
    // 选几个答案 + 写一句简答，交卷
    await p.eval(`(() => {
      const radios = document.querySelectorAll('.wb-pane input[type=radio]');
      if (radios[0]) radios[0].click();
      const boxes = document.querySelectorAll('.wb-pane input[type=checkbox]');
      if (boxes[0]) boxes[0].click();
      if (boxes[1]) boxes[1].click();
      if (boxes[4]) boxes[4].click();
      const ta = document.querySelector('.wb-pane textarea');
      if (ta) {
        const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        setter.call(ta, '因为 (A-λI)v=0 要有非零解，等价于 A-λI 不可逆，也就是行列式为 0。');
        ta.dispatchEvent(new Event('input', { bubbles: true }));
      }
      return true;
    })()`);
    await sleep(500);
    await clickText(p, ".btn", "交卷");
    await sleep(2600);
    await p.shot(`${OUT}/quiz.png`);
  },
  /** 章节（子主题）：侧栏树 + 面包屑 + 继承自父主题的资料 */
  chapters: async (p) => {
    await ensureWorkbench(p);
    await openTopic(p, "第三章 特征值");
    await clickText(p, ".segmented button", "工作台");
    await sleep(700);
    await clickText(p, ".wb-tab", "资料");
    await sleep(1100);
    await p.shot(`${OUT}/chapters.png`);
  },
  mcp: async (p) => {
    await clickText(p, ".sidebar-top .side-item", "MCP");
    await sleep(1400);
    await p.shot(`${OUT}/mcp.png`);
    await p.eval(`(() => { document.querySelector('.modal-head .icon-btn')?.click(); return true; })()`);
    await sleep(400);
  },
  agenda: async (p) => {
    await clickText(p, ".side-item", "日程");
    await sleep(1600);
    await p.shot(`${OUT}/agenda.png`);
  },
  settings: async (p) => {
    await p.eval(`(() => { const b = Array.from(document.querySelectorAll('.icon-btn')).find(x => x.title === '设置'); if (b) b.click(); return !!b; })()`);
    await sleep(1400);
    await p.shot(`${OUT}/settings.png`);
  },
};

/**
 * 点击含有指定文字的第一个元素（选择器 + 文字，比精确选择器抗改）。
 * 点不中直接抛错——静默失败会拍出一堆一模一样的图，而且很难发现。
 */
async function clickText(page, selector, text) {
  const hit = await page.eval(
    `(() => {
      const list = Array.from(document.querySelectorAll(${JSON.stringify(selector)}));
      const el = list.find(x => (x.innerText || '').includes(${JSON.stringify(text)}));
      if (el) { el.scrollIntoView({ block: 'center' }); el.click(); }
      return el ? (el.innerText || '').slice(0, 30) : null;
    })()`,
  );
  if (hit === null) {
    throw new Error(`点不到：${selector} 里含「${text}」的元素`);
  }
  return hit;
}

/** 回到「笔记列表」，保证后面的打开笔记动作是从列表点进去的 */
async function resetNoteList(page) {
  await clickText(page, ".wb-tab", "概览");
  await new Promise((r) => setTimeout(r, 500));
  await clickText(page, ".wb-tab", "笔记");
  await new Promise((r) => setTimeout(r, 700));
}

/** 打开主题并切到工作台：工作台相关场景自己保证起点，单独跑也能出图 */
async function ensureWorkbench(page) {
  await openTopic(page);
  await clickText(page, ".segmented button", "工作台");
  await new Promise((r) => setTimeout(r, 700));
}

/** 打开一个主题（主题行的类名是 .topic-row，不是 .side-item） */
async function openTopic(page, name = "线性代数") {
  await clickText(page, ".topic-row", name);
  await new Promise((r) => setTimeout(r, 1600));
}

async function main() {
  const wanted = process.argv.slice(2);
  const names = wanted.length > 0 ? wanted : Object.keys(SCENES);
  const cdp = await connect();

  // 统一成明亮模式：README 里浅色更清楚
  await cdp.eval(`(() => { document.documentElement.dataset.theme = 'light'; return true; })()`);
  await new Promise((r) => setTimeout(r, 400));

  for (const name of names) {
    const scene = SCENES[name];
    if (!scene) {
      console.error(`没有这个场景：${name}`);
      continue;
    }
    try {
      const shot = async (path) => {
        // 截图前先确认界面上没有渲染失败的残留（Mermaid 的错误框曾经被拍进 README）
        const dirty = await cdp.eval(
          `(() => {
            const t = document.body.innerText;
            return {
              mermaidError: t.includes('Syntax error in text'),
              failedBlocks: document.querySelectorAll('pre[data-mermaid=failed], .mermaid-error').length,
            };
          })()`,
        );
        if (dirty.mermaidError || dirty.failedBlocks > 0) {
          throw new Error(`界面上有渲染失败的残留（mermaidError=${dirty.mermaidError}, failed=${dirty.failedBlocks}），先修好再截图`);
        }
        const first = await cdp.screenshot(path);
        const h1 = fs.readFileSync(path).length;
        // 同一帧重拍一次：WebView2 在窗口不可见时可能不产生新帧
        await new Promise((r) => setTimeout(r, 900));
        const second = await cdp.screenshot(path);
        const h2 = fs.readFileSync(path).length;
        if (h1 !== h2 && seen.has(h2)) {
          console.warn(`  ! ${path} 疑似重复帧（${h2} 字节），已重拍`);
        }
        seen.add(h2);
        return second;
      };
      await scene({ eval: cdp.eval.bind(cdp), shot });
      console.log(`✓ ${name}`);
    } catch (e) {
      console.error(`✗ ${name}: ${e.message}`);
    }
  }
  cdp.close();
}

main().catch((e) => {
  console.error("截图失败：", e.message);
  process.exit(1);
});
