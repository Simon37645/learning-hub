// 端到端冒烟：驱动真实应用跑通一次「输入 → 发送 → 审批 → 工具执行 → 第二轮回复」。
//
// 前置：
//   node scripts/fake-llm.mjs 4321                     （另开一个终端）
//   应用带调试端口启动：set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   模型档案指向 http://127.0.0.1:4321/v1，权限模式「每次确认」
//
// 用法：node scripts/e2e-smoke.mjs [截图路径]

import { connect } from "./cdp.mjs";

const SHOT = process.argv[2] ?? ".screenshots/e2e-chat.png";
const MESSAGE = "请把这次结论写成一篇笔记，然后解释一下你的依据。";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 在 React 受控组件上「真的」输入文字 */
const TYPE_JS = (selector, text) => `(() => {
  const el = document.querySelector(${JSON.stringify(selector)});
  if (!el) return { ok: false, reason: "textarea not found" };
  el.focus();
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
  setter.call(el, ${JSON.stringify(text)});
  el.dispatchEvent(new Event("input", { bubbles: true }));
  return { ok: true, value: el.value.slice(0, 40) };
})()`;

function check(name, ok, detail) {
  const mark = ok ? "PASS" : "FAIL";
  console.log(`[${mark}] ${name}${detail ? ` — ${detail}` : ""}`);
  return ok;
}

async function main() {
  const cdp = await connect();
  let failures = 0;
  const results = [];
  const expect = (name, ok, detail) => {
    if (!check(name, ok, detail)) failures++;
    results.push({ name, ok, detail });
  };

  try {
    console.log(`连接成功，开始冒烟：${MESSAGE}\n`);

    // 0. 基本就绪
    const ready = await cdp.eval(`document.querySelector('.composer textarea') !== null`);
    expect("对话输入框存在", ready === true);
    if (!ready) throw new Error("界面没准备好");

    // 0.1 开一个新对话（避免受上一次遗留消息影响判定）
    await cdp.eval(`(() => {
      const btn = document.querySelector('.chat-head button[title="新对话"]');
      if (btn) btn.click();
      return true;
    })()`);
    await sleep(500);
    const cleared = await cdp.eval(`document.querySelectorAll('.msg').length`);
    expect("新对话已清空消息", cleared === 0, `遗留 ${cleared} 条`);

    // 1. 输入并发送
    const typed = await cdp.eval(TYPE_JS(".composer textarea", MESSAGE));
    expect("注入消息文本", typed?.ok === true, typed?.value);

    const sent = await cdp.eval(`(() => {
      const btns = Array.from(document.querySelectorAll('.composer-bar button'));
      const send = btns.find(b => b.title && b.title.includes("发送")) ?? btns[btns.length - 1];
      if (!send || send.disabled) return { ok: false, reason: "send disabled" };
      send.click();
      return { ok: true };
    })()`);
    expect("点击发送", sent?.ok === true, sent?.reason);

    // 2. 用户气泡立刻出现
    await sleep(600);
    const userBubble = await cdp.eval(`(() => {
      const els = Array.from(document.querySelectorAll('.msg.user .bubble'));
      return els.length ? els[els.length - 1].innerText.slice(0, 30) : null;
    })()`);
    expect("用户消息已渲染", !!userBubble, userBubble);

    // 3. 等审批弹窗（fs/note 写入 → Risk::Write → 每次确认模式下必须弹）
    let approval = null;
    for (let i = 0; i < 40; i++) {
      approval = await cdp.eval(`(() => {
        const modal = document.querySelector('.overlay .modal');
        if (!modal) return null;
        const title = modal.querySelector('.modal-head')?.innerText ?? "";
        const buttons = Array.from(modal.querySelectorAll('.modal-foot button')).map(b => b.innerText.trim());
        return { title: title.replace(/\\n/g, " "), buttons };
      })()`);
      if (approval) break;
      await sleep(250);
    }
    expect("弹出工具审批", !!approval, approval ? `${approval.title} | ${approval.buttons.join(" / ")}` : "超时未见弹窗");

    if (approval) {
      const clicked = await cdp.eval(`(() => {
        const modal = document.querySelector('.overlay .modal');
        const btn = Array.from(modal.querySelectorAll('.modal-foot button')).find(b => b.innerText.includes("允许一次"));
        if (!btn) return { ok: false, buttons: Array.from(modal.querySelectorAll('.modal-foot button')).map(b=>b.innerText) };
        btn.click();
        return { ok: true };
      })()`);
      expect("点击「允许一次」", clicked?.ok === true, clicked?.ok ? "" : JSON.stringify(clicked?.buttons));
    }

    // 4. 等第二轮回复流完
    let done = false;
    let state = null;
    for (let i = 0; i < 120; i++) {
      state = await cdp.eval(`(() => {
        const scroll = document.querySelector('.chat-scroll');
        const text = scroll ? scroll.innerText : "";
        return {
          streaming: !!document.querySelector('.typing-caret'),
          hasToolCard: !!document.querySelector('.tool-card'),
          toolCount: document.querySelectorAll('.tool-card').length,
          katexCount: document.querySelectorAll('.katex').length,
          codeBlocks: document.querySelectorAll('.md pre code').length,
          tableCount: document.querySelectorAll('.md table').length,
          quoteCount: document.querySelectorAll('.md blockquote').length,
          secondRound: text.includes("第 2 轮"),
          error: document.querySelector('.toast.error')?.innerText ?? null,
          tail: text.slice(-260),
        };
      })()`);
      if (state.error) break;
      if (state.secondRound && !state.streaming) {
        done = true;
        break;
      }
      await sleep(400);
    }

    expect("没有错误提示", !state?.error, state?.error ?? "");
    expect("工具卡片已渲染", (state?.toolCount ?? 0) >= 1, `tool-card 数量 ${state?.toolCount}`);
    expect("第二轮回复到达", state?.secondRound === true, done ? "" : `tail: ${state?.tail?.slice(-120)}`);
    expect("流式已结束", state?.streaming === false);
    expect("公式已渲染（KaTeX）", (state?.katexCount ?? 0) > 0, `katex 节点 ${state?.katexCount}`);
    expect("代码块已渲染", (state?.codeBlocks ?? 0) > 0, `pre code ${state?.codeBlocks}`);
    expect("引用块已渲染", (state?.quoteCount ?? 0) > 0, `blockquote ${state?.quoteCount}`);

    // 重复渲染检查：严格模式下事件若被注册两次，内容会整齐翻倍
    const dup = await cdp.eval(`(() => {
      const agents = Array.from(document.querySelectorAll('.msg.agent'));
      const last = agents[agents.length - 1];
      const t = last?.querySelector('.bubble')?.innerText ?? '';
      const marker = '用于验证渲染的片段';
      return { occurrences: t.split(marker).length - 1, chars: t.length, agentCount: agents.length };
    })()`);
    expect(
      "最终回复只渲染一次",
      dup?.occurrences === 1,
      `标记出现 ${dup?.occurrences} 次（agent 消息 ${dup?.agentCount} 条，末条 ${dup?.chars} 字）`,
    );

    const shot = await cdp.screenshot(SHOT);
    console.log(`\n截图：${shot}`);

    console.log(`\n${failures === 0 ? "全部通过 ✅" : `有 ${failures} 项失败 ❌`}`);
    process.exit(failures === 0 ? 0 : 1);
  } finally {
    cdp.close();
  }
}

main().catch((e) => {
  console.error("冒烟失败：", e.message);
  process.exit(2);
});
