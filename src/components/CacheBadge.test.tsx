// 对话框底部那枚「缓存 X%」胶囊：数值算法有单测（lib/usage.test.ts），
// 这里只盯住「渲染 + 悬停展开」这条交互，别再退回「一上来就摊开明细」。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it } from "vitest";
import { CacheBadge } from "./Chat";
import { useApp } from "../store/app";
import type { ChatMessage } from "../lib/types";

afterEach(() => {
  document.body.replaceChildren();
  useApp.setState({ messages: [], usage: null });
});

function assistant(inputTokens: number | null, cachedTokens: number | null): ChatMessage {
  return {
    id: `m${Math.random()}`,
    role: "assistant",
    blocks: [{ type: "text", text: "…" }],
    createdAt: "2026-10-07T00:00:00Z",
    meta: { inputTokens, outputTokens: 10, cachedTokens },
  };
}

function render() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<CacheBadge />));
  return { host, root };
}

it("累积计算出命中率，明细只在鼠标移上去时出现", () => {
  useApp.setState({
    messages: [assistant(10000, 9500), assistant(20000, 19000)],
    usage: {
      input: 20000,
      output: 800,
      cached: 19000,
      cacheWrite: 0,
      context: [
        { label: "对话消息", tokens: 8800 },
        { label: "技能", tokens: 1200 },
      ],
    },
  });

  const { host, root } = render();
  try {
    // 胶囊上是整体命中率：28500 / 30000 = 95%
    expect(host.querySelector(".cache-badge")?.textContent).toContain("缓存 95.0%");
    expect(host.querySelector(".cache-pop")).toBeNull();

    act(() => {
      host
        .querySelector(".cache-badge")!
        .dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    const pop = host.querySelector(".cache-pop");
    expect(pop).not.toBeNull();
    expect(pop!.textContent).toContain("缓存命中率（本对话累计）");
    expect(pop!.textContent).toContain("命中 2.9万 / 输入 3.0万");
    expect(pop!.textContent).toContain("2 轮");
    // 上下文构成的百分比：8800/10000 = 88%
    expect(pop!.textContent).toContain("88.0%");
    expect(pop!.textContent).toContain("对话消息");

    act(() => {
      host
        .querySelector(".cache-badge")!
        .dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    expect(host.querySelector(".cache-pop")).toBeNull();
  } finally {
    act(() => root.unmount());
  }
});

it("服务商没报用量时整块不出现（不给假的 0%）", () => {
  useApp.setState({ messages: [assistant(5000, null)], usage: null });
  const { host, root } = render();
  try {
    expect(host.querySelector(".cache-badge")).toBeNull();
  } finally {
    act(() => root.unmount());
  }
});
