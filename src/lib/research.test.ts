import { describe, expect, it } from "vitest";
import { parseResearchInput, researchReportTitle, RESEARCH_PREFIX } from "./research";

describe("parseResearchInput", () => {
  it("普通消息原样返回、不触发", () => {
    expect(parseResearchInput("什么是特征值？")).toEqual({ deep: false, text: "什么是特征值？" });
    // 正好在聊「研究方法」这类以 /研究 开头的词不触发
    expect(parseResearchInput("/研究方法有哪些")).toEqual({ deep: false, text: "/研究方法有哪些" });
    expect(parseResearchInput("")).toEqual({ deep: false, text: "" });
  });

  it("「/研究 」前缀触发并剥掉前缀", () => {
    expect(parseResearchInput("/研究 量子计算的现状")).toEqual({ deep: true, text: "量子计算的现状" });
    expect(parseResearchInput(`${RESEARCH_PREFIX}\n多行的问题`)).toEqual({ deep: true, text: "多行的问题" });
    expect(parseResearchInput("  /研究   前后空白要压掉  ")).toEqual({ deep: true, text: "前后空白要压掉" });
  });

  it("只有前缀没有问题：算触发但文本为空（发送方按空消息拒掉）", () => {
    expect(parseResearchInput("/研究")).toEqual({ deep: true, text: "" });
    expect(parseResearchInput("   /研究   ")).toEqual({ deep: true, text: "" });
  });
});

describe("researchReportTitle", () => {
  it("优先取第一个 Markdown 标题", () => {
    expect(researchReportTitle("前言不算\n# 高血压指南 2026 修订要点\n## 分节").trim()).toBe(
      "高血压指南 2026 修订要点",
    );
    expect(researchReportTitle("### 三级标题也能当标题\n正文")).toBe("三级标题也能当标题");
  });

  it("没有标题时压成一行截 24 字", () => {
    const t = researchReportTitle("这是一段很长的报告正文，没有标题，只能截前面一小段当作笔记的标题用了。");
    expect(t.length).toBeLessThanOrEqual(24);
    expect(t).toBe("这是一段很长的报告正文，没有标题，只能截前面一小");
  });

  it("空正文给兜底标题", () => {
    expect(researchReportTitle("   \n  ")).toBe("深度研究");
  });
});
