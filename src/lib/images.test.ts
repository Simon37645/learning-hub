import { describe, expect, it } from "vitest";
import { base64FromDataUrl, chooseEncoding, fitWithin, isImageFile, shouldResize } from "./images";

// 贴图这条链路里唯一能脱离浏览器测的部分：尺寸与编码的取舍。
// 「缩放」本身要 canvas，那部分只在真机上手测（应用里的输入框）。

describe("fitWithin", () => {
  it("只缩不放", () => {
    expect(fitWithin(800, 600)).toEqual({ width: 800, height: 600 });
    expect(fitWithin(100, 100)).toEqual({ width: 100, height: 100 });
  });

  it("等比缩到长边上限", () => {
    expect(fitWithin(4000, 3000)).toEqual({ width: 1568, height: 1176 });
    expect(fitWithin(3000, 4000)).toEqual({ width: 1176, height: 1568 });
    // 长边正好等于上限：不动
    expect(fitWithin(1568, 2000)).toEqual({ width: 1229, height: 1568 });
  });

  it("自定义上限与退化尺寸", () => {
    expect(fitWithin(1000, 500, 500)).toEqual({ width: 500, height: 250 });
    // 量不到尺寸（0 / NaN）时不要算出 0×0 的画布，调用方会据此跳过缩放
    expect(fitWithin(0, 100)).toEqual({ width: 0, height: 0 });
    expect(fitWithin(Number.NaN, 100)).toEqual({ width: 0, height: 0 });
  });
});

describe("shouldResize", () => {
  it("小图原样发，避免重编码把字糊掉", () => {
    expect(shouldResize(200_000, 1200, 800)).toBe(false);
  });

  it("体积大或边长超标都要处理", () => {
    expect(shouldResize(2_000_000, 1200, 800)).toBe(true);
    expect(shouldResize(200_000, 4000, 800)).toBe(true);
    expect(shouldResize(200_000, 800, 4000)).toBe(true);
  });
});

describe("chooseEncoding", () => {
  it("PNG 装得下就留 PNG（透明通道转 JPEG 会变黑）", () => {
    expect(chooseEncoding("image/png", 500_000)).toBe("image/png");
  });

  it("PNG 压下来还是太大就退 JPEG", () => {
    expect(chooseEncoding("image/png", 5_000_000)).toBe("image/jpeg");
  });

  it("其它格式一律 JPEG", () => {
    expect(chooseEncoding("image/jpeg", 100)).toBe("image/jpeg");
    expect(chooseEncoding("image/webp", 100)).toBe("image/jpeg");
    expect(chooseEncoding("", 100)).toBe("image/jpeg");
  });
});

describe("base64FromDataUrl", () => {
  it("剥掉 data URL 前缀", () => {
    expect(base64FromDataUrl("data:image/png;base64,AAAA")).toBe("AAAA");
    expect(base64FromDataUrl("data:image/jpeg;base64,//9j")).toBe("//9j");
  });

  it("本来就是裸 base64 时原样返回", () => {
    expect(base64FromDataUrl("AAAA")).toBe("AAAA");
  });
});

describe("isImageFile", () => {
  it("按 MIME 或扩展名都认", () => {
    expect(isImageFile({ type: "image/png", name: "a" })).toBe(true);
    expect(isImageFile({ name: "截图.PNG" })).toBe(true);
    expect(isImageFile({ name: "photo.jpeg" })).toBe(true);
    expect(isImageFile({ name: "讲义.pdf" })).toBe(false);
    expect(isImageFile({ type: "application/pdf", name: "讲义.pdf" })).toBe(false);
    expect(isImageFile({})).toBe(false);
  });
});
