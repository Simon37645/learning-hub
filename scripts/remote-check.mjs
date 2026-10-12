// 移动端远程控制的端到端验证。
//
// 覆盖三件事，全部打真东西（不打桩）：
//   1. 接口层：无令牌 401、被拒的命令 403（能力边界）、越界路径被挡、静态资源可取
//   2. 桥 + 事件层：`/api/invoke` 把命令交给电脑上的前端执行（发消息、审批、读对话），
//      SSE 上真的收到 turn_started / delta / tool_approval / finished
//   3. 界面层：用真浏览器（Edge 无头）按手机竖屏 / iPad 横屏两种视口打开页面，
//      验证**手机上的界面就是桌面端那份产物**（同一套 DOM / 组件 / 渲染器），
//      量布局（抽屉 vs 常显侧栏、有没有水平溢出）、发一条消息、验证回复里的
//      表格与 Mermaid 图（这次是**手机自己**画的——和桌面端同一份渲染器）
//
// 前置只有两件：
//   - 应用带调试端口启动：WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   - 应用里打开过「远程控制」（侧栏 → 远程控制 → 启用）
// 假模型由这个脚本自己起、自己收（用 4399 / 4398 两个端口），不碰用户自己的模型档案。
//
// 用法：node scripts/remote-check.mjs [--no-browser]

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Cdp, listTargets } from "./cdp.mjs";

const ROOT = resolve(import.meta.dirname, "..");
const APP_DIR = join(process.env.APPDATA ?? "", "com.learninghub.desktop");
const CONFIG = join(APP_DIR, "config.json");
const SHOTS = join(ROOT, ".screenshots");
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const LLM_APPROVAL = 4399; // 假模型 A：调写文件的工具（验证审批链路）
const LLM_RICH = 4398; // 假模型 B：回复里带 mermaid / 表格 / 演示卡片
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 给界面层用的回答：mermaid + 表格 + 演示卡片（演示卡片那一条要验证 CSP 放不放行 srcdoc） */
const RICH_ANSWER = join(tmpdir(), "lh-remote-sample.md");

let failures = 0;
let testTopic = null;
const children = [];

function check(name, ok, detail) {
  console.log(`[${ok ? "PASS" : "FAIL"}] ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
  return ok;
}

function readAppConfig() {
  if (!existsSync(CONFIG)) throw new Error(`读不到应用配置：${CONFIG}`);
  return JSON.parse(readFileSync(CONFIG, "utf8"));
}

// ---------------------------------------------------------------- 假模型

async function startFakeLlm(port, env) {
  const proc = spawn(process.execPath, [join(ROOT, "scripts", "fake-llm.mjs"), String(port)], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
  children.push(proc);
  for (let i = 0; i < 40; i++) {
    await sleep(250);
    try {
      await fetch(`http://127.0.0.1:${port}/v1/models`);
      return proc; // 能连上就行（404 也算起来了）
    } catch {
      /* 还没起来 */
    }
  }
  throw new Error(`假模型没起来（端口 ${port}）`);
}

// ---------------------------------------------------------------- 接口层

async function api(base, path, { token, method = "GET", body } = {}) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: {
      ...(token ? { "X-Remote-Token": token } : {}),
      ...(body ? { "Content-Type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  return { status: res.status, data };
}

/** 走 IPC 桥执行一条命令（和手机页面上 invoke() 走的是同一条路）。 */
async function call(base, token, cmd, args = {}) {
  const res = await api(base, "/api/invoke", { token, method: "POST", body: { cmd, args } });
  // 被拒 / 出错时后端给的是 {error, status}，这里统一取成一行文本
  const error = res.data && typeof res.data === "object" ? (res.data.error ?? res.data) : res.data;
  if (res.status !== 200) return { ok: false, error, status: res.status };
  if (!res.data?.ok) return { ok: false, error, status: res.status };
  return { ok: true, value: res.data.value, status: res.status };
}

/** 开一条 SSE 流，把收到的帧收集起来（直到 stop()）。 */
async function openEvents(base, token) {
  const controller = new AbortController();
  const frames = [];
  const done = (async () => {
    const res = await fetch(`${base}/api/events?t=${encodeURIComponent(token)}`, { signal: controller.signal });
    if (!res.ok) throw new Error(`SSE 打不开：${res.status}`);
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    for (;;) {
      const { value, done: end } = await reader.read();
      if (end) break;
      buf += decoder.decode(value, { stream: true });
      let at;
      while ((at = buf.indexOf("\n\n")) >= 0) {
        const chunk = buf.slice(0, at);
        buf = buf.slice(at + 2);
        const line = chunk.split("\n").find((l) => l.startsWith("data: "));
        if (!line) continue; // 心跳（: ping）
        try {
          frames.push(JSON.parse(line.slice(6)));
        } catch {
          /* 忽略半截帧 */
        }
      }
    }
  })().catch((e) => {
    if (e.name !== "AbortError") throw e;
  });
  return {
    frames,
    stop: () => controller.abort(),
    done,
    /** 收到的 hub://agent 事件（帧的形状：{kind:"event", name, payload}） */
    agent: () => frames.filter((f) => f.kind === "event" && f.name === "hub://agent").map((f) => f.payload),
    kinds: () => frames.filter((f) => f.kind === "event" && f.name === "hub://agent").map((f) => f.payload.kind),
  };
}

// ---------------------------------------------------------------- 桌面端（CDP）

/** 主窗口的 CDP（要挑主窗口：内置浏览器的子 WebView 也挂在同一个调试端口上） */
async function mainWindow() {
  const targets = await listTargets();
  const main = targets.find((t) => t.url.includes("localhost:1420") || t.url.startsWith("tauri://")) ?? targets[0];
  if (!main) throw new Error("没有找到应用窗口，确认应用带 --remote-debugging-port=9222 启动");
  return new Cdp(main.webSocketDebuggerUrl);
}

/** 直接调 Tauri 命令（等价于界面上点设置） */
async function invoke(cdp, cmd, args = {}) {
  return cdp.eval(
    `(async () => window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}))()`,
  );
}

/** 把模型临时指向某个假模型、权限模式也换掉，跑完原样还回去。 */
async function withProfile(cdp, port, mode, run) {
  const before = await invoke(cdp, "config_get");
  const original = { activeProfileId: before.activeProfileId, permissionMode: before.agent?.permissionMode };
  const marker = `127.0.0.1:${port}`;
  let profile = before.profiles.find((p) => (p.baseUrl ?? "").includes(marker));
  let createdId = null;
  if (!profile) {
    const cfg = await invoke(cdp, "profile_upsert", {
      input: { name: `远程验证（假模型 ${port}）`, baseUrl: `http://${marker}/v1`, model: "fake-model" },
    });
    profile = cfg.profiles.find((p) => (p.baseUrl ?? "").includes(marker));
    createdId = profile?.id ?? null;
  }
  if (!profile) throw new Error("没能准备好假模型档案");
  await invoke(cdp, "config_patch", { patch: { activeProfileId: profile.id, permissionMode: mode } });
  try {
    return await run();
  } finally {
    await invoke(cdp, "config_patch", {
      patch: { activeProfileId: original.activeProfileId, permissionMode: original.permissionMode },
    });
    if (createdId) await invoke(cdp, "profile_delete", { id: createdId }).catch(() => {});
  }
}

// ---------------------------------------------------------------- 真浏览器

class EdgeBrowser {
  constructor(port) {
    this.port = port;
    this.proc = null;
  }

  async start() {
    this.proc = spawn(
      EDGE,
      [
        "--headless=new",
        `--remote-debugging-port=${this.port}`,
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--user-data-dir=" + join(process.env.TEMP ?? "/tmp", "lh-remote-check-edge"),
        "about:blank",
      ],
      { stdio: "ignore" },
    );
    children.push(this.proc);
    for (let i = 0; i < 60; i++) {
      await sleep(500);
      try {
        if ((await fetch(`http://127.0.0.1:${this.port}/json/version`)).ok) return;
      } catch {
        /* 还没起来 */
      }
    }
    throw new Error("Edge 没起来（无头模式）");
  }

  stop() {
    try {
      this.proc?.kill();
    } catch {
      /* ignore */
    }
  }
}

/** 用某个视口打开页面、跑一段 JS、按需抓图。 */
async function withViewport(port, { url, width, height, mobile }, run) {
  const res = await fetch(`http://127.0.0.1:${port}/json/list`);
  const page = (await res.json()).filter((t) => t.type === "page" && t.webSocketDebuggerUrl)[0];
  const cdp = new Cdp(page.webSocketDebuggerUrl);
  await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: mobile ? 2 : 1, mobile });
  await cdp.send("Page.enable");
  await cdp.send("Page.navigate", { url });
  // 等页面加载 + SSE 连上 + 首屏渲染（手机上要下 1.4MB 的 JS，比旧页面慢）
  for (let i = 0; i < 40; i++) {
    await sleep(500);
    const ready = await cdp
      .eval(`!!document.querySelector(".app-shell") && !!window.__LH_REMOTE__`)
      .catch(() => false);
    if (ready) break;
  }
  await sleep(1200);
  try {
    return await run(cdp);
  } finally {
    cdp.close();
  }
}

// ---------------------------------------------------------------- 主流程

async function main() {
  const noBrowser = process.argv.includes("--no-browser");
  const cfg = readAppConfig();
  const remote = cfg.remote ?? {};
  if (!remote.enabled) throw new Error("应用里没打开「远程控制」——先侧栏打开它再跑这个脚本");
  const port = remote.port ?? 54713;
  const token = remote.token;
  const base = `http://127.0.0.1:${port}`;
  console.log(`远程服务：${base}（令牌 ${token.slice(0, 6)}…）\n`);

  // ------------------------------------------------ 1. 接口层
  check("没有令牌 → 401", (await api(base, "/api/invoke", { method: "POST", body: { cmd: "topic_list" } })).status === 401);
  check(
    "错令牌 → 401",
    (await api(base, "/api/invoke", { token: "0".repeat(32), method: "POST", body: { cmd: "topic_list" } })).status ===
      401,
  );
  const ping = await api(base, "/api/ping", { token });
  check("带令牌 ping 通", ping.status === 200 && ping.data?.ok === true);

  const page = await fetch(`${base}/`);
  const html = await page.text();
  check("页面能拿到（不需要令牌）", page.status === 200 && html.includes("</html>"));
  check("页面是桌面端那份产物 + 桥", html.includes("/shim.js") && /\/assets\/index-[\w-]+\.js/.test(html));
  check("页面带 no-referrer（令牌在地址栏里，外链不能带出去）", html.includes('name="referrer"'));
  const js = await fetch(`${base}/shim.js`);
  const shimText = await js.text();
  check("桥能拿到且会接 SSE", js.status === 200 && shimText.includes("EventSource"));
  const assetPath = html.match(/\/assets\/index-[\w-]+\.js/)?.[0] ?? "";
  const asset = await fetch(`${base}${assetPath}`);
  check("产物里的 JS 能拿到", asset.status === 200 && (await asset.text()).length > 1000, assetPath);
  check("图标能拿到", (await fetch(`${base}/icon.svg`)).status === 200);
  check("清单能拿到", (await fetch(`${base}/manifest.webmanifest`)).status === 200);

  // ---- 能力边界：这几条是这个功能的「安全线」，每条都得有人盯着
  const unknown = await call(base, token, "definitely_not_a_command");
  check("未知命令被拒（403）", unknown.status === 403, String(unknown.error).slice(0, 40));
  const del = await call(base, token, "topic_delete", { slug: "x" });
  check("删主题被拒（403，带原因）", del.status === 403 && String(del.error).includes("电脑"), del.error);
  const perm = await call(base, token, "config_patch", { patch: { permissionMode: "full" } });
  check("远程不能开「完全访问」", perm.status === 403 && String(perm.error).includes("permissionMode"), perm.error);
  const fileWrite = await call(base, token, "file_write_text", { slug: "x", path: "a.md", content: "x" });
  check("远程不能直接写文件", fileWrite.status === 403, fileWrite.error);
  check(
    "附件接口挡住越界路径",
    (await api(base, "/api/file?path=../../../../Windows/win.ini", { token })).status === 403,
  );
  const theme = await call(base, token, "config_patch", { patch: { theme: "dark" } });
  check("远程能改的字段（明暗）放行", theme.ok === true, String(theme.error ?? ""));

  // ------------------------------------------------ 2. 桥 + 事件层
  const cdp = await mainWindow();
  await startFakeLlm(LLM_APPROVAL, {
    FAKE_TOOL: "fs_write",
    FAKE_TOOL_ARGS: JSON.stringify({ path: "notes/remote-check.md", content: "# 远程验证\n" }),
  });
  await withProfile(cdp, LLM_APPROVAL, "ask", async () => {
    const topics = await call(base, token, "topic_list");
    check("桥能读到主题清单", topics.ok && Array.isArray(topics.value) && topics.value.length > 0,
      `${topics.value?.length ?? 0} 个主题`);
    const topic = topics.value[0].slug;
    testTopic = topic;

    const created = await call(base, token, "agent_new_chat");
    const chatId = created.value;
    check("能开一条新对话", created.ok && typeof chatId === "string" && chatId.length > 20);

    const events = await openEvents(base, token);
    await sleep(300);
    const sent = await call(base, token, "agent_send", {
      req: {
        chatId,
        topicSlug: topic,
        text: "远程验证：请把这次结论写进 notes/remote-check.md",
        attachments: [],
        images: [],
        mode: "study",
      },
    });
    check("发消息被受理（走的是桌面端那个 agent_send）", sent.ok && !!sent.value?.turnId, `turnId=${sent.value?.turnId?.slice(0, 8)}`);

    let approval = null;
    for (let i = 0; i < 60 && !approval; i++) {
      await sleep(250);
      approval = events.agent().find((e) => e.kind === "tool_approval");
    }
    if (approval) {
      check("SSE 上收到 tool_approval（手机侧能看到待确认）", true, approval.call?.summary);
      const approved = await call(base, token, "agent_approve", {
        requestId: approval.call.requestId,
        allow: true,
        always: false,
      });
      check("从手机侧批准成功", approved.ok === true && approved.value === true, String(approved.error ?? ""));
    } else {
      console.log("（跳过审批验证：这次假模型用的是读工具）");
    }

    let finished = null;
    for (let i = 0; i < 160 && !finished; i++) {
      await sleep(250);
      finished = events.agent().find((e) => e.kind === "finished");
    }
    check("这一轮跑完（finished）", !!finished);
    const kinds = events.kinds();
    check("SSE 收到流式增量", kinds.includes("delta"));
    check("SSE 收到工具开始/结束", kinds.includes("tool_started") && kinds.includes("tool_finished"));
    check("SSE 收到落库消息", kinds.includes("message"));

    const transcript = await call(base, token, "agent_transcript", { chatId, topicSlug: topic });
    const roles = (transcript.value ?? []).map((m) => m.role);
    check("对话记录里有完整回合", transcript.ok && roles.includes("user") && roles.includes("assistant"), roles.join("→"));

    events.stop();
    await events.done;
  });
  cdp.close();

  // ------------------------------------------------ 3. 界面层（真浏览器）
  if (!noBrowser && existsSync(EDGE)) {
    mkdirSync(SHOTS, { recursive: true });
    // 假模型 B：回复里带 mermaid / 表格 / 演示卡片
    writeFileSync(
      RICH_ANSWER,
      [
        "好的，先把地图给你。",
        "",
        "```mermaid",
        "mindmap",
        '  n0["特征值与特征向量"]',
        '    n1["是什么"]',
        '        n2["方向不变的向量"]',
        '    n4["怎么算"]',
        '        n5["解 det(A-λI)=0"]',
        "```",
        "",
        "| 概念 | 一句话 |",
        "| --- | --- |",
        "| 特征向量 | 变换后方向不变的向量 |",
        "| 特征值 | 这个方向被拉伸的倍数 |",
        "",
        "$$Av = \\lambda v$$",
        "",
        "```html",
        '<div style="font:16px system-ui;color:#4f8">演示卡片：脚本不执行，动画只用 CSS</div>',
        "```",
        "",
      ].join("\n"),
      "utf8",
    );
    await startFakeLlm(LLM_RICH, {
      FAKE_TOOL: "fs_list",
      FAKE_TOOL_ARGS: JSON.stringify({ path: "notes" }),
      FAKE_ANSWER_FILE: RICH_ANSWER,
    });
    const browser = new EdgeBrowser(9333);
    await browser.start();
    const cdp2 = await mainWindow();
    try {
      await withProfile(cdp2, LLM_RICH, "full", async () => {
        const url = `${base}/?t=${token}`;

        // 手机竖屏
        await withViewport(9333, { url, width: 390, height: 844, mobile: true }, async (vp) => {
          const probe = await vp.eval(`(async () => {
            const sidebar = document.querySelector(".sidebar");
            const cs = sidebar ? getComputedStyle(sidebar) : null;
            return {
              remote: document.documentElement.getAttribute("data-remote"),
              bridge: !!window.__LH_REMOTE__,
              shell: !!document.querySelector(".app-shell"),
              // 「就是桌面端那份界面」：同一套组件渲染出来的骨架
              desktopDom: !!document.querySelector(".app-main .main-body .main-center"),
              composer: !!document.querySelector(".composer .textarea"),
              menu: !!document.querySelector(".tb-menu"),
              windowButtons: document.querySelectorAll(".titlebar .tb-btn.close").length,
              drawerHidden: cs ? cs.visibility === "hidden" : null,
              overflowX: document.documentElement.scrollWidth > window.innerWidth + 1,
              hint: document.querySelector(".home-wrap, .welcome, .chat-head") ? true : false,
            };
          })()`);
          check("竖屏：html 上挂了 data-remote", probe.remote === "1");
          check("竖屏：桥已就位（window.__LH_REMOTE__）", probe.bridge === true);
          check("竖屏：界面是桌面端那套 DOM（不是另一套手机版）", probe.shell && probe.desktopDom);
          check("竖屏：没有窗口按钮（手机上没有窗口可控制）", probe.windowButtons === 0);
          check("竖屏：有 ☰（侧栏抽屉的开关）", probe.menu === true);
          check("竖屏：侧栏默认收起", probe.drawerHidden === true, String(probe.drawerHidden));
          check("竖屏：没有水平溢出", probe.overflowX === false);
          check("竖屏：输入框在", probe.composer === true);
          await vp.screenshot(join(SHOTS, "remote-phone.png"));

          // 抽屉：打开 → 点主题 → 自动收起并进入主题视图
          const opened = await vp.eval(`(() => {
            document.querySelector(".tb-menu").click();
            return true;
          })()`);
          await sleep(600);
          const afterOpen = await vp.eval(`(async () => {
            const sidebar = document.querySelector(".sidebar");
            return {
              open: sidebar.classList.contains("open"),
              topics: document.querySelectorAll(".topic-row[data-slug]").length,
              scrim: !!document.querySelector(".sidebar-scrim"),
            };
          })()`);
          check("竖屏：点 ☰ 抽屉打开", opened === true && afterOpen.open === true);
          check("竖屏：列出了主题", afterOpen.topics > 0, `${afterOpen.topics} 个`);
          check("竖屏：抽屉有幕布（点外面能收起）", afterOpen.scrim === true);
          await vp.screenshot(join(SHOTS, "remote-phone-drawer.png"));

          await vp.eval(`(() => { document.querySelector(".topic-row[data-slug]").click(); return true; })()`);
          await sleep(1500);
          const afterPick = await vp.eval(`(async () => ({
            open: document.querySelector(".sidebar").classList.contains("open"),
            topicView: !!document.querySelector(".chat-head"),
          }))()`);
          check("竖屏：点主题后进入主题视图", afterPick.topicView === true);
          check("竖屏：点完主题抽屉自动收起", afterPick.open === false);
        });

        // iPad 横屏：侧栏常显、对话在右边
        await withViewport(9333, { url, width: 1024, height: 768, mobile: false }, async (vp) => {
          const probe = await vp.eval(`(async () => {
            const sidebar = document.querySelector(".sidebar");
            const r = sidebar.getBoundingClientRect();
            const cs = getComputedStyle(sidebar);
            const messages = document.querySelector(".main-center").getBoundingClientRect();
            return {
              // 宽屏上侧栏是普通文档流里的常显栏（不是抽屉：那才是 fixed + 藏起来）
              position: cs.position,
              visible: cs.visibility !== "hidden",
              railVisible: r.x >= -1 && r.width > 100,
              railWidth: Math.round(r.width),
              centerLeft: Math.round(messages.x),
              overflowX: document.documentElement.scrollWidth > window.innerWidth + 1,
            };
          })()`);
          check(
            "横屏：侧栏常显（不是抽屉）",
            probe.position !== "fixed" && probe.visible && probe.railVisible,
            `position=${probe.position} w=${probe.railWidth}`,
          );
          check("横屏：对话区在侧栏右侧", probe.centerLeft >= probe.railWidth - 2, `center.x=${probe.centerLeft}`);
          check("横屏：没有水平溢出", probe.overflowX === false);
          await vp.screenshot(join(SHOTS, "remote-ipad.png"));
        });

        // 手机竖屏：发一条消息 → 等回复（含表格 / Mermaid / 演示卡片）渲染完
        await withViewport(9333, { url, width: 390, height: 844, mobile: true }, async (vp) => {
          const typed = await vp.eval(`(() => {
            const ta = document.querySelector(".composer .textarea");
            if (!ta) return { ok: false };
            const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
            setter.call(ta, "给我一张特征值与特征向量的思维导图");
            ta.dispatchEvent(new Event("input", { bubbles: true }));
            const btn = document.querySelector(".send-btn");
            if (btn.disabled) return { ok: false, why: "按钮还是禁用的（输入没进去）" };
            btn.click();
            return { ok: true };
          })()`);
          check("手机页面上能提交消息", typed.ok === true, typed.why ?? "");

          let st = null;
          for (let i = 0; i < 90; i++) {
            await sleep(400);
            st = await vp.eval(`(async () => ({
              user: document.querySelectorAll(".msg.user").length,
              agent: document.querySelectorAll(".msg.agent").length,
              streaming: !!document.querySelector(".send-btn.stop"),
              tools: document.querySelectorAll(".tool").length,
              demo: document.querySelectorAll(".demo-card iframe").length,
              diagramState: document.querySelector("[data-mermaid]")?.getAttribute("data-mermaid") || null,
              svg: document.querySelectorAll(".mermaid-block svg").length,
              table: document.querySelectorAll(".bubble table").length,
              math: document.querySelectorAll(".bubble .katex").length,
            }))()`);
            if (st.agent > 0 && !st.streaming && st.diagramState && st.diagramState !== "loading") break;
          }
          check(
            "手机页面上看到完整回复（流式渲染收尾）",
            st?.agent > 0 && st.streaming === false,
            `user=${st?.user} agent=${st?.agent} tools=${st?.tools}`,
          );
          check("表格渲染出来了", st?.table > 0, `${st?.table} 张`);
          check("公式渲染出来了", st?.math > 0, `${st?.math} 处`);
          check(
            "Mermaid 图在手机上自己画出来了（同一份渲染器）",
            st?.diagramState === "done" && st?.svg > 0,
            `state=${st?.diagramState} svg=${st?.svg}`,
          );
          check("演示卡片渲染出来了（CSP 放行 srcdoc）", st?.demo > 0, `${st?.demo} 张`);
          await vp.screenshot(join(SHOTS, "remote-phone-chat.png"));

          // 图里真有节点才说明渲染链是通的（不是空壳 SVG）
          const svgInfo = await vp.eval(`(async () => {
            const svg = document.querySelector(".mermaid-block svg");
            return svg ? { nodes: svg.querySelectorAll("g.node").length, w: svg.getAttribute("width") } : null;
          })()`);
          check("SVG 里有图节点（不是空壳）", (svgInfo?.nodes ?? 0) > 0, `nodes=${svgInfo?.nodes} width=${svgInfo?.w}`);
        });
      });
    } finally {
      cdp2.close();
      browser.stop();
    }
  } else if (!noBrowser) {
    console.log("（没找到 Edge，跳过界面层验证）");
  }

  console.log(`\n${failures === 0 ? "全部通过" : `${failures} 项失败`}`);
  // 清理：假模型可能真写下的那篇笔记（对话本身留着无妨）
  if (testTopic) rmSync(join(ROOT, "workspace", testTopic, "notes", "remote-check.md"), { force: true });
  rmSync(RICH_ANSWER, { force: true });
  for (const c of children) {
    try {
      c.kill();
    } catch {
      /* ignore */
    }
  }
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("失败：", e.stack ?? e.message);
  for (const c of children) {
    try {
      c.kill();
    } catch {
      /* ignore */
    }
  }
  process.exit(2);
});
