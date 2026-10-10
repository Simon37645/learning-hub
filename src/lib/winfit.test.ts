import { describe, expect, it } from "vitest";
import { innerForOuter } from "./winfit";

// 「贴满屏幕」这条链路上唯一能脱离窗口测的部分：外框 ↔ 内容区的换算。
// 真机上的表现（多显示器会不会突出去一块）靠 CDP 量窗口几何（见 AGENTS.md 的验证方式）。

describe("innerForOuter", () => {
  it("减掉那圈隐形边框（实测 22×13 物理像素）", () => {
    expect(
      innerForOuter(
        { width: 1502, height: 953 },
        { width: 1480, height: 940 },
        { width: 2560, height: 1528 },
      ),
    ).toEqual({ width: 2538, height: 1515 });
  });

  it("全屏时内外相等（没有边框）→ 原样返回", () => {
    const same = { width: 2560, height: 1600 };
    expect(innerForOuter(same, same, same)).toEqual(same);
  });

  it("边框量不出来（0 / 负数）时不乱减", () => {
    expect(innerForOuter({ width: 0, height: 0 }, { width: 0, height: 0 }, { width: 800, height: 600 })).toEqual({
      width: 800,
      height: 600,
    });
    // 极端情况：外框比内容区还小（不该发生，但别算出负数）
    expect(
      innerForOuter({ width: 10, height: 10 }, { width: 20, height: 20 }, { width: 800, height: 600 }),
    ).toEqual({ width: 800, height: 600 });
  });

  it("目标比边框还小时至少留 1 像素", () => {
    expect(
      innerForOuter({ width: 1502, height: 953 }, { width: 1480, height: 940 }, { width: 10, height: 5 }),
    ).toEqual({ width: 1, height: 1 });
  });
});
