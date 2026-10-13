// 侧栏分界线：拖它改侧栏宽度（宽度存在 store + localStorage）。
//
// 为什么单独做成手柄：分界线原本只是一条 1px 边框，用户以为能拖但拖不动。
// 手柄做 4px、往左偏 4px 盖在边框上，既好抓又不占宽度。

import { useEffect, useState } from "react";
import { useDragCursor } from "../lib/dragcursor";
import { useApp } from "../store/app";

export function SidebarResizer() {
  const width = useApp((s) => s.sidebarWidth);
  const setWidth = useApp((s) => s.setSidebarWidth);
  const { begin, end, isDragging } = useDragCursor();

  // 手柄元素存成 state（而不是 useRef）：手柄是**条件渲染**的，用 useRef + 依赖数组时
  // 漏一个依赖就变成「手柄出现之后再也拖不动」——这个坑踩过一次（内置浏览器那条 effect
  // 忘了把「面板可见」放进依赖里）。存成 state，元素一出现 effect 就重新跑，结构上不会再漏。
  const [handle, setHandle] = useState<HTMLDivElement | null>(null);

  // 宽度落到 CSS 变量上，.sidebar 用的是 var(--sidebar-w)
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${width}px`);
  }, [width]);

  // 用 pointer 事件 + setPointerCapture：只听 window 的 mouseup 会在
  // 「松开时指针在原生子 WebView / 窗口外」时丢事件——之后鼠标不按键也会
  // 改宽度，直到再点一次才解锁。capture 把后续指针事件都锁到手柄上。
  // 光标与选区的接管走 lib/dragcursor.ts（那边保证任何一条收尾路径都能还回去）。
  useEffect(() => {
    if (!handle) return;
    const onMove = (e: PointerEvent) => {
      if (isDragging(e)) setWidth(e.clientX);
    };
    const onUp = (e: PointerEvent) => end(e);
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
    handle.addEventListener("lostpointercapture", onUp);
    return () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      handle.removeEventListener("lostpointercapture", onUp);
    };
  }, [handle, setWidth, end, isDragging]);

  return (
    <div
      ref={setHandle}
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
