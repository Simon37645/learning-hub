// 通过 CDP（Chrome DevTools Protocol）驱动学习中枢的 WebView2。
//
// 用途：开发时给真实应用做自动化验证——量布局、点按钮、抓截图。
// 需要应用带远程调试端口启动：
//   Windows: set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 && npm run app:dev
//
// 用法：
//   node scripts/cdp.mjs targets
//   node scripts/cdp.mjs eval "document.title"
//   node scripts/cdp.mjs screenshot out.png
//   node scripts/cdp.mjs click ".wb-tab:nth-child(4)"
//   node scripts/cdp.mjs metrics

const PORT = process.env.CDP_PORT ?? "9222";
const BASE = `http://127.0.0.1:${PORT}`;

async function listTargets() {
  const res = await fetch(`${BASE}/json/list`);
  const all = await res.json();
  // 只要页面类型，且必须是我们的应用（排除 devtools 自己的页面）
  return all.filter((t) => t.type === "page" && t.webSocketDebuggerUrl);
}

class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.ready = new Promise((resolve, reject) => {
      this.ws.addEventListener("open", () => resolve());
      this.ws.addEventListener("error", (e) => reject(new Error("ws error: " + (e.message ?? e))));
    });
    this.ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(typeof ev.data === "string" ? ev.data : ev.data.toString());
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(JSON.stringify(msg.error)));
        else resolve(msg.result);
      }
    });
  }

  async send(method, params = {}) {
    await this.ready;
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`timeout: ${method}`));
        }
      }, 30000);
    });
  }

  /** 求值一段 JS，自动 await Promise、返回 JSON 化结果 */
  async eval(expression) {
    const wrapped = `(async () => { try { return JSON.stringify(await (${expression})); } catch (e) { return JSON.stringify({__error: String(e && e.stack || e)}); } })()`;
    const r = await this.send("Runtime.evaluate", {
      expression: wrapped,
      awaitPromise: true,
      returnByValue: true,
    });
    const raw = r?.result?.value;
    if (typeof raw !== "string") return raw;
    const parsed = JSON.parse(raw);
    if (parsed && parsed.__error) throw new Error(parsed.__error);
    return parsed;
  }

  async screenshot(path) {
    // 先让布局稳定
    await new Promise((r) => setTimeout(r, 250));
    const r = await this.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    const { writeFileSync, mkdirSync } = await import("node:fs");
    const { dirname, resolve } = await import("node:path");
    const full = resolve(path);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, Buffer.from(r.data, "base64"));
    return full;
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* ignore */
    }
  }
}

const METRICS_JS = `(() => {
  const box = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height),
             display: cs.display, alignItems: cs.alignItems, textAlign: cs.textAlign };
  };
  return {
    dpr: window.devicePixelRatio,
    viewport: { w: window.innerWidth, h: window.innerHeight },
    scroll: { x: document.documentElement.scrollWidth, y: document.documentElement.scrollHeight },
    sidebar: box(".sidebar"),
    mainBody: box(".main-body"),
    mainCenter: box(".main-center"),
    viewer: box(".viewer"),
    chat: box(".chat"),
    chatScroll: box(".chat-scroll"),
    empty: box(".empty"),
    homeInner: box(".home-inner"),
    quickCol: box(".empty .col"),
    composer: box(".composer"),
    title: document.title,
    hasError: !!document.querySelector(".toast.error"),
  };
})()`;

export { Cdp, listTargets };

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  const targets = await listTargets();
  if (targets.length === 0) {
    console.error(`没有找到可调试页面。确认应用带 --remote-debugging-port=${PORT} 启动。`);
    process.exit(1);
  }

  if (cmd === "targets") {
    for (const t of targets) console.log(`${t.title}\n  ${t.url}\n  ${t.webSocketDebuggerUrl}`);
    return;
  }

  const cdp = new Cdp(targets[0].webSocketDebuggerUrl);
  try {
    switch (cmd) {
      case "eval": {
        const out = await cdp.eval(rest.join(" "));
        console.log(typeof out === "string" ? out : JSON.stringify(out, null, 2));
        break;
      }
      case "metrics": {
        console.log(JSON.stringify(await cdp.eval(METRICS_JS), null, 2));
        break;
      }
      case "click": {
        const sel = rest[0];
        const out = await cdp.eval(`(() => {
          const el = document.querySelector(${JSON.stringify(sel)});
          if (!el) return { ok: false, reason: "not found" };
          el.scrollIntoView({ block: "center" });
          el.click();
          return { ok: true, text: (el.textContent || "").trim().slice(0, 60) };
        })()`);
        console.log(JSON.stringify(out));
        break;
      }
      case "type": {
        // 真实输入：先聚焦目标元素，再用 Input.insertText 送进去
        const sel = rest[0] && rest[0].startsWith(".") ? rest[0] : ".cm-content";
        const text = rest[0] && rest[0].startsWith(".") ? rest.slice(1).join(" ") : rest.join(" ");
        await cdp.eval(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (el) { el.focus(); } return !!el; })()`);
        await cdp.send("Input.insertText", { text });
        console.log(JSON.stringify({ typed: text.slice(0, 40), into: sel }));
        break;
      }
      case "text": {
        const out = await cdp.eval(`(() => {
          const el = document.querySelector(${JSON.stringify(rest[0])});
          return el ? (el.innerText || "").slice(0, 4000) : null;
        })()`);
        console.log(out ?? "(null)");
        break;
      }
      case "screenshot": {
        const p = await cdp.screenshot(rest[0] ?? "screenshot.png");
        console.log("saved: " + p);
        break;
      }
      default:
        console.error("用法: targets | metrics | eval <js> | click <selector> | text <selector> | screenshot <path>");
        process.exit(2);
    }
  } finally {
    cdp.close();
  }
}

/** 连接第一个页面目标 */
export async function connect() {
  const targets = await listTargets();
  if (targets.length === 0) {
    throw new Error(`没有找到可调试页面。确认应用带 --remote-debugging-port=${PORT} 启动。`);
  }
  return new Cdp(targets[0].webSocketDebuggerUrl);
}

// 只在被直接运行时才走命令行分支（被 import 时什么都不做）
const invokedDirectly = process.argv[1] && /cdp\.mjs$/.test(process.argv[1].replace(/\\/g, "/"));
if (invokedDirectly) {
  main().catch((e) => {
    console.error("失败:", e.message);
    process.exit(1);
  });
}
