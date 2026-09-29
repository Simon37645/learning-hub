// 侧栏分界线：拖它改侧栏宽度（宽度存在 store + localStorage）。
//
// 为什么单独做成手柄：分界线原本只是一条 1px 边框，用户以为能拖但拖不动。
// 手柄做 4px、往左偏 4px 盖在边框上，既好抓又不占宽度。

import { useEffect, useRef } from "react";
import { useApp } from "../store/app";

export function SidebarResizer() {
  const width = useApp((s) => s.sidebarWidth);
  const setWidth = useApp((s) => s.setSidebarWidth);
  const dragging = useRef(false);

  // 宽度落到 CSS 变量上，.sidebar 用的是 var(--sidebar-w)
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${width}px`);
  }, [width]);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (dragging.current) setWidth(e.clientX);
    };
    const onUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setWidth]);

  return (
    <div
      className="sidebar-resizer"
      title="拖动调整侧栏宽度"
      onMouseDown={() => {
        dragging.current = true;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
      }}
    />
  );
}
