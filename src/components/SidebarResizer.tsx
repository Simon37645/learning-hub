// 侧栏分界线：拖它改侧栏宽度（宽度存在 store + localStorage）。
//
// 为什么单独做成手柄：分界线原本只是一条 1px 边框，用户以为能拖但拖不动。
// 手柄做 4px、往左偏 4px 盖在边框上，既好抓又不占宽度。

import { useEffect, useRef } from "react";
import { useApp } from "../store/app";

export function SidebarResizer() {
  const width = useApp((s) => s.sidebarWidth);
  const setWidth = useApp((s) => s.setSidebarWidth);
  const handle = useRef<HTMLDivElement>(null);

  // 宽度落到 CSS 变量上，.sidebar 用的是 var(--sidebar-w)
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${width}px`);
  }, [width]);

  // 用 pointer 事件 + setPointerCapture：只听 window 的 mouseup 会在
  // 「松开时指针在原生子 WebView / 窗口外」时丢事件——之后鼠标不按键也会
  // 改宽度，直到再点一次才解锁。capture 把后续指针事件都锁到手柄上。
  useEffect(() => {
    const el = handle.current;
    if (!el) return;
    const onMove = (e: PointerEvent) => {
      if (el.hasPointerCapture(e.pointerId)) setWidth(e.clientX);
    };
    const end = (e: PointerEvent) => {
      if (!el.hasPointerCapture(e.pointerId)) return;
      el.releasePointerCapture(e.pointerId);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", end);
    el.addEventListener("pointercancel", end);
    return () => {
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", end);
      el.removeEventListener("pointercancel", end);
    };
  }, [setWidth]);

  return (
    <div
      ref={handle}
      className="sidebar-resizer"
      title="拖动调整侧栏宽度"
      onPointerDown={(e) => {
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* 合成事件下会抛，抓不住就退化为普通拖动 */
        }
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
      }}
    />
  );
}
