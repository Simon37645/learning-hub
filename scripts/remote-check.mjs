// 移动端远程控制的端到端验证。
//
// 覆盖三件事，全部打真东西（不打桩）：
//   1. 接口层：无令牌 401、带令牌能读状态、能开对话、能发消息、越界路径被挡
//   2. 事件层：SSE 上真的收到 turn_started / delta / tool_approval / finished；
//      审批从「手机侧」点允许后，工具真的执行、第二轮回复真的回来
//   3. 界面层：用真浏览器（Edge 无头）按手机竖屏 / iPad 横屏两种视口打开页面，
//      量布局（抽屉 vs 双栏）、发一条消息、验证回复里的演示卡片与 Mermaid 图
//      （图是**桌面端**渲染的——手机页面不装 mermaid），并抓几张截图
//
// 前置只有两件：
//   - 应用带调试端口启动：WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   - 应用里打开过「远程控制」（侧栏 → 远程控制 → 启用）
// 假模型由这个脚本自己起、自己收（用 4399 / 4398 两个端口），不碰用户自己的模型档案。
//
// 用法：node scripts/remote-check.mjs [--no-browser]

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
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
    kinds: () => frames.map((f) => (f.kind === "agent" ? f.event.kind : f.kind)),
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
  await sleep(3500); // 等页面加载 + SSE 连上 + 首屏渲染
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
  const port = remote.port ?? 8787;
  const token = remote.token;
  const base = `http://127.0.0.1:${port}`;
  console.log(`远程服务：${base}（令牌 ${token.slice(0, 6)}…）\n`);

  // ------------------------------------------------ 1. 接口层
  check("没有令牌 → 401", (await api(base, "/api/state")).status === 401);
  check("错令牌 → 401", (await api(base, "/api/state", { token: "0".repeat(32) })).status === 401);
  const ping = await api(base, "/api/ping", { token });
  check("带令牌 ping 通", ping.status === 200 && ping.data?.ok === true);

  const state = await api(base, "/api/state", { token });
  const topicCount = state.data?.topics?.length ?? 0;
  check("读到主题清单", state.status === 200 && topicCount > 0, `${topicCount} 个主题`);

  const page = await fetch(`${base}/?t=${token}`);
  const html = await page.text();
  check("手机页面能拿到", page.status === 200 && html.includes("app.js"));
  check("样式表能拿到", (await fetch(`${base}/app.css`)).status === 200);
  const js = await fetch(`${base}/app.js`);
  check("脚本能拿到", js.status === 200 && (await js.text()).includes("EventSource"));
  check("图标能拿到", (await fetch(`${base}/icon.svg`)).status === 200);
  check("清单能拿到", (await fetch(`${base}/manifest.webmanifest`)).status === 200);
  check(
    "附件接口挡住越界路径",
    (await api(base, "/api/file?path=../../../../Windows/win.ini", { token })).status === 403,
  );

  // ------------------------------------------------ 2. 事件层（发消息 → 审批 → 工具 → 收尾）
  const cdp = await mainWindow();
  await startFakeLlm(LLM_APPROVAL, {
    FAKE_TOOL: "fs_write",
    FAKE_TOOL_ARGS: JSON.stringify({ path: "notes/remote-check.md", content: "# 远程验证\n" }),
  });
  await withProfile(cdp, LLM_APPROVAL, "ask", async () => {
    const topic = state.data.topics[0].slug;
    testTopic = topic;
    const chatId = (await api(base, "/api/new_chat", { token, method: "POST" })).data.chatId;
    check("能开一条新对话", typeof chatId === "string" && chatId.length > 20);

    const events = await openEvents(base, token);
    await sleep(300);
    const sent = await api(base, "/api/send", {
      token,
      method: "POST",
      body: { topic, chat: chatId, text: "远程验证：请把这次结论写进 notes/remote-check.md" },
    });
    check("发消息被受理", sent.status === 200 && !!sent.data?.turnId, `turnId=${sent.data?.turnId?.slice(0, 8)}`);

    let approval = null;
    for (let i = 0; i < 60 && !approval; i++) {
      await sleep(250);
      approval = events.frames.find((f) => f.kind === "agent" && f.event.kind === "tool_approval");
    }
    if (approval) {
      check("收到 tool_approval（手机侧能看到待确认）", true, approval.event?.call?.summary);
      const approved = await api(base, "/api/approve", {
        token,
        method: "POST",
        body: { requestId: approval.event.call.requestId, allow: true, always: false },
      });
      check("从手机侧批准成功", approved.data?.handled === true);
    } else {
      console.log("（跳过审批验证：这次假模型用的是读工具）");
    }

    let finished = null;
    for (let i = 0; i < 160 && !finished; i++) {
      await sleep(250);
      finished = events.frames.find((f) => f.kind === "agent" && f.event.kind === "finished");
    }
    check("这一轮跑完（finished）", !!finished);
    const kinds = events.kinds();
    check("SSE 收到流式增量", kinds.includes("delta"));
    check("SSE 收到工具开始/结束", kinds.includes("tool_started") && kinds.includes("tool_finished"));
    check("SSE 收到落库消息", kinds.includes("message"));

    const transcript = await api(base, `/api/transcript?topic=${encodeURIComponent(topic)}&chat=${chatId}`, { token });
    const roles = (transcript.data ?? []).map((m) => m.role);
    check("对话记录里有完整回合", roles.includes("user") && roles.includes("assistant"), roles.join("→"));

    events.stop();
    await events.done;
  });
  cdp.close();

  // ------------------------------------------------ 3. 界面层（真浏览器）
  if (!noBrowser && existsSync(EDGE)) {
    mkdirSync(SHOTS, { recursive: true });
    // 假模型 B：回复里带 mermaid / 表格（用仓库里那份示例回答）
    await startFakeLlm(LLM_RICH, {
      FAKE_TOOL: "fs_list",
      FAKE_TOOL_ARGS: JSON.stringify({ path: "notes" }),
      FAKE_ANSWER_FILE: join(ROOT, "scripts", "sample-answer.md"),
    });
    const browser = new EdgeBrowser(9333);
    await browser.start();
    const cdp2 = await mainWindow();
    try {
      await withProfile(cdp2, LLM_RICH, "full", async () => {
        const url = `${base}/?t=${token}`;

        // 手机竖屏：先清掉上一轮留下的「当前对话」，免得断言读到历史消息
        await withViewport(9333, { url, width: 390, height: 844, mobile: true }, async (vp) => {
          await vp.eval(`(async () => { try { localStorage.clear(); } catch (e) {} return true; })()`);
          await vp.send("Page.navigate", { url });
          await sleep(3000);
          const probe = await vp.eval(`(async () => {
            const rail = document.querySelector(".rail");
            const cs = getComputedStyle(rail);
            return {
              railHidden: cs.transform !== "none" && cs.transform !== "matrix(1, 0, 0, 1, 0, 0)",
              topics: document.querySelectorAll("[data-slug]").length,
              hasComposer: !!document.querySelector("#text"),
              dot: document.querySelector("#dot")?.className,
              overflowX: document.documentElement.scrollWidth > window.innerWidth + 1,
              hint: !!document.querySelector(".hint"),
            };
          })()`);
          check("竖屏：抽屉默认收起", probe.railHidden === true);
          check("竖屏：没有水平溢出", probe.overflowX === false);
          check("竖屏：事件流已连上（绿点）", String(probe.dot).includes("on"), probe.dot);
          check("竖屏：列出了主题", probe.topics > 0, `${probe.topics} 个`);
          check("竖屏：空态有提示（不是白板）", probe.hint === true);
          await vp.screenshot(join(SHOTS, "remote-phone.png"));

          const clicked = await vp.eval(`(() => {
            document.querySelector("#menu-btn").click();
            const row = document.querySelector("[data-slug]");
            if (!row) return { ok: false };
            row.click();
            return { ok: true };
          })()`);
          await sleep(1200);
          const after = await vp.eval(`(async () => ({
            drawerOpen: document.querySelector(".rail").classList.contains("open"),
            hasChats: document.querySelectorAll("[data-chat]").length,
          }))()`);
          check("竖屏：点 ☰ 抽屉能打开", after.drawerOpen === true && clicked.ok === true);
          await vp.screenshot(join(SHOTS, "remote-phone-drawer.png"));
        });

        // iPad 横屏：列表栏固定、对话在右边
        await withViewport(9333, { url, width: 1024, height: 768, mobile: false }, async (vp) => {
          const probe = await vp.eval(`(async () => {
            const rail = document.querySelector(".rail");
            const r = rail.getBoundingClientRect();
            const messages = document.querySelector(".messages").getBoundingClientRect();
            return {
              position: getComputedStyle(rail).position,
              railWidth: Math.round(r.width),
              railVisible: r.x >= -1 && r.width > 100,
              messagesLeft: Math.round(messages.x),
              overflowX: document.documentElement.scrollWidth > window.innerWidth + 1,
            };
          })()`);
          check("横屏：列表栏固定显示", probe.position === "relative" && probe.railVisible, `w=${probe.railWidth}`);
          check("横屏：对话区在列表右侧", probe.messagesLeft >= probe.railWidth - 2, `messages.x=${probe.messagesLeft}`);
          check("横屏：没有水平溢出", probe.overflowX === false);
          await vp.screenshot(join(SHOTS, "remote-ipad.png"));
        });

        // 手机竖屏：新建对话 → 发消息 → 等回复（含表格与 Mermaid 图）渲染完
        await withViewport(9333, { url, width: 390, height: 844, mobile: true }, async (vp) => {
          await vp.eval(`(async () => { document.querySelector("#new-btn").click(); return true; })()`);
          await sleep(800);
          const before = await vp.eval(`(async () => document.querySelectorAll(".msg").length)()`);
          const typed = await vp.eval(`(() => {
            const ta = document.querySelector("#text");
            const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
            setter.call(ta, "给我一张特征值与特征向量的思维导图");
            ta.dispatchEvent(new Event("input", { bubbles: true }));
            document.querySelector("#send-btn").click();
            return { ok: true };
          })()`);
          check("手机页面能提交消息", typed.ok === true && before === 0, `起始 ${before} 条`);

          let st = null;
          for (let i = 0; i < 70; i++) {
            await sleep(400);
            st = await vp.eval(`(async () => ({
              user: document.querySelectorAll(".msg.user").length,
              assistant: document.querySelectorAll(".msg.assistant").length,
              streaming: !!document.getElementById("streaming-bubble"),
              tools: document.querySelectorAll(".tool").length,
              demo: document.querySelectorAll(".demo iframe").length,
              diagramState: document.querySelector("[data-mermaid]")?.getAttribute("data-state") || null,
              svg: document.querySelectorAll(".diagram svg").length,
              table: document.querySelectorAll(".md table").length,
              thinking: document.querySelectorAll(".thinking").length,
            }))()`);
            if (st.assistant > 0 && !st.streaming && st.diagramState && st.diagramState !== "loading") break;
          }
          check(
            "手机页面上看到完整回复（流式渲染收尾）",
            st?.assistant > 0 && st.streaming === false,
            `user=${st?.user} assistant=${st?.assistant} tools=${st?.tools}`,
          );
          check("表格渲染出来了", st?.table > 0, `${st?.table} 张`);
          check(
            "Mermaid 图由桌面端画好送回",
            st?.diagramState === "done" && st?.svg > 0,
            `state=${st?.diagramState} svg=${st?.svg}`,
          );
          await vp.screenshot(join(SHOTS, "remote-phone-chat.png"));

          // 图里真有节点才说明「借桌面端渲染」这条链是通的（不是空壳 SVG）
          const svgInfo = await vp.eval(`(async () => {
            const svg = document.querySelector(".diagram svg");
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
