import { describe, expect, it } from "vitest";
import { advanceHeadingNumber, createHeadingCounters, numberHeadings } from "./headingNumber";

describe("heading numbering", () => {
  it("numbers from H2 and keeps H1 unnumbered", () => {
    expect(numberHeadings([1, 2])).toEqual([null, "1"]);
    expect(numberHeadings([2, 2, 2])).toEqual(["1", "2", "3"]);
  });

  it("nests deeper levels with dots", () => {
    expect(numberHeadings([2, 3, 4, 5, 6])).toEqual([
      "1",
      "1.1",
      "1.1.1",
      "1.1.1.1",
      "1.1.1.1.1",
    ]);
  });

  it("restarts deeper levels under a new parent", () => {
    expect(numberHeadings([2, 3, 3, 2, 3])).toEqual(["1", "1.1", "1.2", "2", "2.1"]);
  });

  it("resets everything on H1", () => {
    expect(numberHeadings([2, 3, 1, 2])).toEqual(["1", "1.1", null, "1"]);
  });

  it("keeps zero placeholders when a level is skipped, matching CSS counters", () => {
    // Typora 主题用的是 counter(heading-h2) "." counter(heading-h3) "." counter(heading-h4)，
    // 跳级时未被递增的那一级就是 0，所以 H2 直接接 H4 会得到 1.0.1。
    expect(numberHeadings([2, 4])).toEqual(["1", "1.0.1"]);
  });

  it("mutates the counters in place so a single tree walk can accumulate", () => {
    const counters = createHeadingCounters();
    expect(advanceHeadingNumber(counters, 2)).toBe("1");
    expect(advanceHeadingNumber(counters, 3)).toBe("1.1");
    expect(advanceHeadingNumber(counters, 3)).toBe("1.2");
    expect(advanceHeadingNumber(counters, 2)).toBe("2");
    expect(counters.slice(2)).toEqual([2, 0, 0, 0, 0]);
  });

  it("clamps levels beyond H6 instead of writing out of range", () => {
    const counters = createHeadingCounters();
    // Markdown 最多 H6；这里只保证越界级别被钳制、不会写到数组外，编号本身无意义。
    expect(advanceHeadingNumber(counters, 9)?.split(".")).toHaveLength(5);
    expect(counters).toHaveLength(7);
  });
});
