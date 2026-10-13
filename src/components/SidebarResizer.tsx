// 侧栏分界线：拖它改侧栏宽度（宽度存在 store + localStorage）。
//
// 为什么单独做成手柄：分界线原本只是一条 1px 边框，用户以为能拖但拖不动。
// 手柄做 4px、往左偏 4px 盖在边框上，既好抓又不占宽度。

import { useEffect, useRef } from "react";
import { useDragCursor } from "../lib/dragcursor";
import { useApp } from "../store/app";

export function SidebarResizer() {
  const width = useApp((s) => s.sidebarWidth);
  const setWidth = useApp((s) => s.setSidebarWidth);
  const handle = useRef<HTMLDivElement>(null);
  const { begin, end, isDragging } = useDragCursor();

  // 宽度落到 CSS 变量上，.sidebar 用的是 var(--sidebar-w)
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${width}px`);
  }, [width]);

  // 用 pointer 事件 + setPointerCapture：只听 window 的 mouseup 会在
  // 「松开时指针在原生子 WebView / 窗口外」时丢事件——之后鼠标不按键也会
  // 改宽度，直到再点一次才解锁。capture 把后续指针事件都锁到手柄上。
  // 光标与选区的接管走 lib/dragcursor.ts（那边保证任何一条收尾路径都能还回去）。
  useEffect(() => {
    const el = handle.current;
    if (!el) return;
    const onMove = (e: PointerEvent) => {
      if (isDragging(e)) setWidth(e.clientX);
    };
    const onUp = (e: PointerEvent) => end(e);
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("pointercancel", onUp);
    el.addEventListener("lostpointercapture", onUp);
    return () => {
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
      el.removeEventListener("pointercancel", onUp);
      el.removeEventListener("lostpointercapture", onUp);
    };
  }, [setWidth, end, isDragging]);

  return (
    <div
      ref={handle}
      className="sidebar-resizer"
      title="拖动调整侧栏宽度"
      onPointerDown={(e) => {
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* 合成事件下会抛。抓不住只是手滑出去之后收不到 pointermove，
             拖动与光标都不依赖它（见 lib/dragcursor.ts） */
        }
        begin(e);
      }}
    />
  );
}
