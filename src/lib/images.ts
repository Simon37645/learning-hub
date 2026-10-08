// 贴图 / 拖图 / 选图：把各种来源的图片变成能直接发给后端的一串 base64。
//
// 为什么在前端缩放：手机截图动辄 3000×4000，直接发过去有两个代价——
// 传输与上下文都是按图块算的（长边超过 1568px 之后**不会更清楚，只会更贵**），
// 而且有些服务商对请求体积有硬限制。缩放是纯客户端的活，放这里最省事：
// Rust 那边就不必为了改个尺寸引一个图像库。
//
// 类型不在这里定死：后端认字节里的魔数，前端报的 media_type 只是参考。

import type { ImageUpload } from "./types";

/** 长边上限。两家服务商都按「切成 512/768px 的块」计费，超过这个尺寸只烧钱。 */
export const MAX_EDGE = 1568;
/** 小于这个体积就不动它（原样发，避免重编码把截图里的字糊掉） */
const RESIZE_OVER_BYTES = 1_500_000;
/** 编码后的目标体积：超过就退成 JPEG */
const TARGET_BYTES = 3_500_000;

export type Encoding = "image/png" | "image/jpeg";

/** 等比缩到长边不超过 max。纯函数，好测。 */
export function fitWithin(
  width: number,
  height: number,
  max: number = MAX_EDGE,
): { width: number; height: number } {
  if (!(width > 0) || !(height > 0)) return { width: 0, height: 0 };
  const scale = Math.min(1, max / Math.max(width, height));
  return {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}

/** 要不要动这张图：太大或边长超标才处理。 */
export function shouldResize(bytes: number, width: number, height: number): boolean {
  return bytes > RESIZE_OVER_BYTES || Math.max(width, height) > MAX_EDGE;
}

/**
 * 缩放后用什么格式导出。
 *
 * 截图（PNG）里有大色块，JPEG 往往能小一个量级，但 PNG 的透明通道转 JPEG 会变黑——
 * 所以先按 PNG 编，只有结果还是太大才退 JPEG。
 */
export function chooseEncoding(sourceType: string, encodedBytes: number): Encoding {
  if (sourceType !== "image/png") return "image/jpeg";
  return encodedBytes <= TARGET_BYTES ? "image/png" : "image/jpeg";
}

/** `data:image/png;base64,xxxx` → `xxxx`（后端两种都收，这里统一成裸 base64）。 */
export function base64FromDataUrl(dataUrl: string): string {
  const at = dataUrl.indexOf("base64,");
  return at >= 0 ? dataUrl.slice(at + 7) : dataUrl;
}

/** 从 DataTransfer / ClipboardEvent 里挑出图片文件。 */
export function imageFilesFrom(items: DataTransferItemList | null | undefined): File[] {
  if (!items) return [];
  const out: File[] = [];
  for (const item of Array.from(items)) {
    if (item.kind !== "file" || !item.type.startsWith("image/")) continue;
    const file = item.getAsFile();
    if (file) out.push(file);
  }
  return out;
}

export function isImageFile(file: { type?: string; name?: string }): boolean {
  if (file.type?.startsWith("image/")) return true;
  // 拖进来的文件有时没有 type（尤其是从某些压缩包里拖出来），按扩展名兜一下
  return /\.(png|jpe?g|gif|webp)$/i.test(file.name ?? "");
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("读不出这个文件"));
    reader.readAsDataURL(blob);
  });
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("这张图打不开（格式可能不被支持）"));
    img.src = url;
  });
}

function canvasToBlob(canvas: HTMLCanvasElement, type: Encoding): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error("图片压缩失败"))),
      type,
      0.92,
    );
  });
}

/**
 * 一张图准备好之后的东西。
 *
 * `upload` 才是发给后端的载荷（后端会**再按魔数判一次**类型，这里报的只是参考）；
 * `mediaType` 与 `preview` 是给界面用的：缩略图要有一个 MIME 正确的 data URL，
 * 靠文件名后缀猜类型会在 `.jpeg` 这类名字上猜错。
 */
export interface PreparedImage {
  upload: ImageUpload;
  mediaType: string;
  preview: string;
}

/**
 * 一张图 → 待上传的载荷。量到尺寸会一起带上（后端用它估 token）。
 *
 * 尺寸量不出来（图坏了）时抛错——发送一张读不出来的图没有意义，
 * 让用户在输入框旁边看到「这张图打不开」比默默发一张空图好。
 */
export async function prepareImage(raw: Blob, name: string): Promise<PreparedImage> {
  const dataUrl = await blobToDataUrl(raw);
  const img = await loadImage(dataUrl);
  const width = img.naturalWidth;
  const height = img.naturalHeight;

  if (!shouldResize(raw.size, width, height)) {
    // 原样发：不重编码，截图里的字才不会被糊掉
    const mediaType = raw.type || "image/png";
    const data = base64FromDataUrl(dataUrl);
    return {
      upload: { name, data, width, height },
      mediaType,
      preview: `data:${mediaType};base64,${data}`,
    };
  }

  const size = fitWithin(width, height);
  const canvas = document.createElement("canvas");
  canvas.width = size.width;
  canvas.height = size.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("画布不可用，没法压缩这张图");
  // 缩小时开平滑，字才不至于糊成一片
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, 0, 0, size.width, size.height);

  let encoding = chooseEncoding(raw.type, raw.size);
  let blob = await canvasToBlob(canvas, encoding);
  if (encoding === "image/png" && blob.size > TARGET_BYTES) {
    // PNG 压下来还是太大：退 JPEG（截图里大色块多，能小一个量级）
    encoding = "image/jpeg";
    blob = await canvasToBlob(canvas, encoding);
  }
  const out = await blobToDataUrl(blob);
  const data = base64FromDataUrl(out);
  return {
    upload: { name, data, width: size.width, height: size.height },
    mediaType: encoding,
    preview: `data:${encoding};base64,${data}`,
  };
}
