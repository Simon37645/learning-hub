// 一次性验证脚本：ui-state 恢复链路（v0.4.1-beta.1）。
// 用法：node scripts/restore-check.mjs <阶段：setup|verify>
//   setup  —— 在运行中的应用里确认布置好的工作区模样，然后点真实关闭按钮
//   verify —— 重启后连上主窗口，逐项比对恢复结果
//
// 注意：网页标签的原生子 WebView 与主窗口共用调试端口，这里固定连
// localhost:1420 的主窗口目标。

import { Cdp, listTargets } from "./cdp.mjs";

const phase = process.argv[2] ?? "setup";
let failures = 0;
const expect = (name, ok, detail) => {
  console.log(`[${ok ? "PASS" : "FAIL"}] ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
};

async function connectMain() {
  const targets = await listTargets();
  const main = targets.find((t) => (t.url ?? "").includes("localhost:1420"));
  if (!main) throw new Error("找不到主窗口目标（只剩子 WebView？）");
  return new Cdp(main.webSocketDebuggerUrl);
}

const c = await connectMain();
const state = await c.eval(`(() => JSON.stringify({
  view: document.querySelector('.chat-head') ? 'topic' : 'other',
  seg: document.querySelector('.segmented .on')?.textContent?.trim() ?? null,
  viewer: !!document.querySelector('aside.viewer'),
  tabs: Array.from(document.querySelectorAll('.viewer-tabs .vtab')).map(t => t.textContent.trim().slice(0, 14)),
  activeTab: document.querySelector('.viewer-tabs .vtab.on')?.textContent?.trim().slice(0, 14) ?? null,
}))()`);
console.log(phase === "setup" ? "关闭前状态：" : "重启后状态：", state);

if (phase === "arrange") {
  // 布置「工作区模样」：两个浏览器标签 + 切到工作台半区
  const r = await c.eval(`(async () => {
    const inv = window.__TAURI_INTERNALS__.invoke;
    await inv('viewer_open', { req: { path: 'notes/炎症 · 框架地图.md', topicSlug: '炎症', kind: 'markdown' } });
    await inv('viewer_open', { req: { url: 'https://example.com/', title: 'Example', kind: 'web', newTab: true } });
    const btn = Array.from(document.querySelectorAll('.segmented button')).find(b => b.textContent.includes('工作台'));
    if (btn) btn.click();
    return '布置完成';
  })()`);
  console.log(r);
} else if (phase === "setup") {
  // 用 **CDP 的真实鼠标事件**点关闭按钮，别用 element.click()：
  // 自绘标题栏里那个按钮对「非可信的合成 click」不响应（React 的 onClick 收不到，
  // 实测过；同一条标题栏里的最大化按钮却响应——原因没查到底）。
  // 真实鼠标事件才是用户做的事，这里要验的就是「用户点关闭能不能关掉」。
  const box = JSON.parse(
    await c.eval(`(() => {
      const b = document.querySelector('.tb-btn.close');
      if (!b) return 'null';
      const r = b.getBoundingClientRect();
      return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2 });
    })()`),
  );
  if (!box) throw new Error("找不到关闭按钮（界面还没起来？）");
  await c.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: box.x, y: box.y, button: "none" });
  await c.send("Input.dispatchMouseEvent", { type: "mousePressed", x: box.x, y: box.y, button: "left", clickCount: 1 });
  await c.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: box.x, y: box.y, button: "left", clickCount: 1 });
  console.log("已点真实关闭按钮（走 onCloseRequested → saveUiState → 退出）…");
} else {
  const s = JSON.parse(state);
  expect("回到主题视图", s.view === "topic", s.view);
  expect("对话/工作台恢复到上次的半区", s.seg === "工作台", s.seg);
  expect("内置浏览器回来了", s.viewer === true, String(s.viewer));
  // 布置阶段开了 3 个标签（Markdown + Example ×2），激活的是最后动的那个
  expect("标签数量与关前一致", s.tabs?.length === 3, JSON.stringify(s.tabs));
  expect("激活的是网页标签", (s.activeTab ?? "").includes("Example"), s.activeTab);
}
process.exit(failures ? 1 : 0);
