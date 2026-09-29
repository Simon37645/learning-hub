// 应用自己的标题栏（无边框窗口）。
//
// 为什么自绘：系统标题栏属于 Windows 画的非客户区，学习中枢用的是 WebView2，
// 最大化/贴靠时那条原生区域会**压在网页内容上面**（用户看到「一条黑框一直挡着界面，
// 上面是最小化/窗口化/关闭」）。自绘之后窗口里只有我们自己的 DOM，
// 原生区域不再和界面抢地方；顺带把标题栏做得更紧凑（32px）。
//
// 交互细节：
// - 整条是拖拽区（data-tauri-drag-region），按钮上不要加，否则拖不动窗口
// - 双击标题栏 = 最大化/还原（和系统一致）
// - 需要 tauri.conf.json 里 `decorations: false`

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useApp } from "../store/app";
import { AppMark, Icon } from "./ui";

export function TitleBar() {
  const topicName = useApp((s) => s.topic?.meta.name ?? null);
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | null = null;
    void win.isMaximized().then(setMaximized).catch(() => {});
    void win
      .onResized(() => {
        void win.isMaximized().then(setMaximized).catch(() => {});
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  const win = getCurrentWindow();

  return (
    <div className="titlebar" data-tauri-drag-region onDoubleClick={() => void win.toggleMaximize()}>
      <span className="tb-brand" data-tauri-drag-region>
        <AppMark size={14} color="var(--accent)" />
        学习中枢
      </span>
      {topicName && (
        <span className="tb-topic" data-tauri-drag-region>
          {topicName}
        </span>
      )}
      <span className="grow" data-tauri-drag-region />
      <button className="tb-btn" title="最小化" onClick={() => void win.minimize()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
          <path d="M0 5h10" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
      <button className="tb-btn" title={maximized ? "还原" : "最大化"} onClick={() => void win.toggleMaximize()}>
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <rect x="0.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1" />
            <path d="M2.5 2.5V0.5h7v7h-2" fill="none" stroke="currentColor" strokeWidth="1" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1" />
          </svg>
        )}
      </button>
      <button className="tb-btn close" title="关闭" onClick={() => void win.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
          <path d="M0 0l10 10M10 0L0 10" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
    </div>
  );
}

/** 窗口顶部的拖拽区图标（用内置 Icon 也行，这里保持标题栏零依赖） */
export function TitleBarHint() {
  return <Icon name="hub" size={12} />;
}
