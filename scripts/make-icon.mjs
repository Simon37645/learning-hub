// 生成应用图标源图（1024x1024 PNG），不依赖任何图形库：
// 手写 PNG chunk + zlib deflate。2x 超采样做抗锯齿。
// 用法：node scripts/make-icon.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SS = 2; // 超采样倍率
const FINAL = 1024;
const S = FINAL * SS;

const INK = [0x13, 0x13, 0x16, 0xff]; // 近黑
const PAPER = [0xf6, 0xf6, 0xf4, 0xff]; // 米白
const ACCENT = [0xd8, 0x5a, 0x2b, 0xff]; // 砖红点缀

/** 圆角矩形（含符号距离场，边缘 1px 内做软过渡） */
function insideRoundRect(x, y, left, top, right, bottom, radius) {
  const cx = Math.min(Math.max(x, left + radius), right - radius);
  const cy = Math.min(Math.max(y, top + radius), bottom - radius);
  const dx = x - cx;
  const dy = y - cy;
  return Math.hypot(dx, dy) <= radius;
}

/** 点到线段的距离，用于画粗线（人字形） */
function distToSegment(px, py, x1, y1, x2, y2) {
  const vx = x2 - x1;
  const vy = y2 - y1;
  const wx = px - x1;
  const wy = py - y1;
  const len2 = vx * vx + vy * vy || 1;
  const t = Math.min(1, Math.max(0, (wx * vx + wy * vy) / len2));
  return Math.hypot(px - (x1 + t * vx), py - (y1 + t * vy));
}

function paint() {
  const buf = new Uint8Array(S * S * 4);
  const R = 224 * SS; // 圆角半径
  const stroke = 62 * SS; // 笔画粗细
  const cx = S / 2;

  // 三层人字形（向上收敛），越上层越短 —— 表示学习路径层层递进
  const chevrons = [
    { yTop: 640 * SS, halfW: 300 * SS, h: 150 * SS, color: PAPER },
    { yTop: 452 * SS, halfW: 232 * SS, h: 132 * SS, color: PAPER },
    { yTop: 268 * SS, halfW: 160 * SS, h: 116 * SS, color: ACCENT },
  ];

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      let px = [0, 0, 0, 0];
      if (insideRoundRect(x, y, 24 * SS, 24 * SS, S - 24 * SS, S - 24 * SS, R)) {
        px = INK.slice();
        for (const c of chevrons) {
          const d = Math.min(
            distToSegment(x, y, cx - c.halfW, c.yTop + c.h, cx, c.yTop),
            distToSegment(x, y, cx, c.yTop, cx + c.halfW, c.yTop + c.h),
          );
          if (d <= stroke / 2) px = c.color.slice();
        }
      }
      const i = (y * S + x) * 4;
      buf[i] = px[0];
      buf[i + 1] = px[1];
      buf[i + 2] = px[2];
      buf[i + 3] = px[3];
    }
  }
  return buf;
}

/** 2x2 盒式降采样 */
function downsample(src, size, factor) {
  const out = new Uint8Array((size / factor) * (size / factor) * 4);
  const n = factor * factor;
  for (let y = 0; y < size / factor; y++) {
    for (let x = 0; x < size / factor; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let dy = 0; dy < factor; dy++) {
        for (let dx = 0; dx < factor; dx++) {
          const i = ((y * factor + dy) * size + (x * factor + dx)) * 4;
          r += src[i]; g += src[i + 1]; b += src[i + 2]; a += src[i + 3];
        }
      }
      const o = (y * (size / factor) + x) * 4;
      out[o] = r / n; out[o + 1] = g / n; out[o + 2] = b / n; out[o + 3] = a / n;
    }
  }
  return out;
}

function crc32(bytes) {
  let c = ~0;
  for (let i = 0; i < bytes.length; i++) {
    c ^= bytes[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), Buffer.from(data)]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePng(rgba, size) {
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    Buffer.from(rgba.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 6;  // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, "..", "src-tauri");
mkdirSync(outDir, { recursive: true });
const png = encodePng(downsample(paint(), S, SS), FINAL);
const out = resolve(outDir, "app-icon.png");
writeFileSync(out, png);
console.log(`icon source written: ${out} (${png.length} bytes)`);
