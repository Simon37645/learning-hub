// 联网搜索条数钳制的回归网：设置页的数字输入框与后端的夹法（1-50）必须一致，
// 不然会出现「界面存 99、实际按 50 发」的口径错位。

import { describe, expect, it } from "vitest";
import { clampSearchCount, COUNT_MAX, COUNT_MIN, DEFAULT_SEARCH_COUNT } from "./websearch";

describe("clampSearchCount", () => {
  it("合法值原样保留（含上下边界）", () => {
    expect(clampSearchCount(8)).toBe(8);
    expect(clampSearchCount(1)).toBe(COUNT_MIN);
    expect(clampSearchCount(50)).toBe(COUNT_MAX);
  });

  it("越界钳到边界", () => {
    expect(clampSearchCount(0)).toBe(COUNT_MIN);
    expect(clampSearchCount(99)).toBe(COUNT_MAX);
    expect(clampSearchCount(-3)).toBe(COUNT_MIN);
  });

  it("小数取整", () => {
    expect(clampSearchCount(8.6)).toBe(9);
    expect(clampSearchCount(8.4)).toBe(8);
  });

  it("非法输入回默认值 8（按次计费，默认要省着用）", () => {
    expect(clampSearchCount("")).toBe(DEFAULT_SEARCH_COUNT);
    expect(clampSearchCount("abc")).toBe(DEFAULT_SEARCH_COUNT);
    expect(clampSearchCount(undefined)).toBe(DEFAULT_SEARCH_COUNT);
    expect(clampSearchCount(null)).toBe(DEFAULT_SEARCH_COUNT);
    expect(clampSearchCount(NaN)).toBe(DEFAULT_SEARCH_COUNT);
  });
});
