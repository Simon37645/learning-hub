import { describe, expect, it } from "vitest";
import { contextShares, summarizeCache } from "./usage";
import type { ChatMessage } from "./types";

function msg(input: number | null, cached: number | null, output = 100): ChatMessage {
  return {
    id: `m${Math.random()}`,
    role: "assistant",
    blocks: [{ type: "text", text: "…" }],
    createdAt: "2026-10-07T00:00:00Z",
    meta: {
      inputTokens: input,
      outputTokens: output,
      cachedTokens: cached,
    },
  };
}

describe("summarizeCache", () => {
  it("累计每轮：命中 / 输入，算整体命中率", () => {
    const s = summarizeCache([msg(10000, 9500), msg(20000, 18000)]);
    expect(s.turns).toBe(2);
    expect(s.input).toBe(30000);
    expect(s.cached).toBe(27500);
    expect(s.rate).toBeCloseTo(91.67, 1);
  });

  it("只统计报过用量的轮次——没数据的轮次既不计入也不拉低命中率", () => {
    const s = summarizeCache([
      msg(1000, 900),
      msg(5000, null), // 服务商没报缓存用量
      { ...msg(null, null), meta: { inputTokens: null, outputTokens: null } }, // 老消息，啥也没有
    ]);
    expect(s.turns).toBe(1);
    expect(s.input).toBe(1000);
    expect(s.rate).toBeCloseTo(90, 5);
  });

  it("一轮都没有就返回 0 轮（界面据此整块不显示）", () => {
    expect(summarizeCache([]).turns).toBe(0);
    expect(summarizeCache([msg(null, null)]).turns).toBe(0);
    expect(summarizeCache([]).rate).toBe(0);
  });

  it("没有输入量时不做除零", () => {
    expect(summarizeCache([msg(0, 0)]).rate).toBe(0);
  });
});

describe("contextShares", () => {
  it("按 token 数折算百分比", () => {
    const shares = contextShares([
      { label: "对话消息", tokens: 880 },
      { label: "技能", tokens: 120 },
    ]);
    expect(shares.map((s) => s.label)).toEqual(["对话消息", "技能"]);
    expect(shares[0].share).toBeCloseTo(88, 5);
    expect(shares[1].share).toBeCloseTo(12, 5);
  });

  it("空数据不炸", () => {
    expect(contextShares([])).toEqual([]);
    expect(contextShares([{ label: "x", tokens: 0 }])).toEqual([]);
  });
});
