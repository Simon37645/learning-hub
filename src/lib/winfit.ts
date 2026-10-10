// 窗口尺寸的一个坑，值得单独一个模块：**Tauri 的 setSize 设的是内容区尺寸**，
// 而显示器与工作区给的是**外框**尺寸。直接拿外框尺寸去 setSize，窗口会比目标大一圈——
// 那圈隐形边框（无边框可缩放窗口照样有，实测 22×13 物理像素）：
// 多显示器时就是「右边突出去一点点，跑到旁边那块屏上」（用户报过）。
//
// 所以「把窗口贴满某块屏 / 某个工作区」一律走这里：先量一次边框，减掉再设。
// 位置那边不用担心——setPosition 与 outerPosition 都是外框口径，是一致的。

import { PhysicalPosition, PhysicalSize, type Window } from "@tauri-apps/api/window";

/**
 * 目标外框尺寸 → `setSize` 要的内容区尺寸（纯函数，方便单测）。
 *
 * 全屏状态下窗口没有那圈边框（内外相等），减 0 就是原样，所以同一个函数两种状态都对。
 */
export function innerForOuter(
  outer: { width: number; height: number },
  inner: { width: number; height: number },
  target: { width: number; height: number },
): { width: number; height: number } {
  const dw = Math.max(0, outer.width - inner.width);
  const dh = Math.max(0, outer.height - inner.height);
  return { width: Math.max(1, target.width - dw), height: Math.max(1, target.height - dh) };
}

/** 把窗口的**外框**摆到 (x, y)，尺寸设成 width×height（贴满屏幕或工作区时用）。 */
export async function fitOuter(
  win: Window,
  x: number,
  y: number,
  width: number,
  height: number,
): Promise<void> {
  const [outer, inner] = await Promise.all([win.outerSize(), win.innerSize()]);
  const size = innerForOuter(outer, inner, { width, height });
  await win.setSize(new PhysicalSize(size.width, size.height));
  await win.setPosition(new PhysicalPosition(x, y));
}
