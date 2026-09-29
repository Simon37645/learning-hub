// 应用自己的标题栏（无边框窗口）。
//
// 为什么自绘：系统标题栏属于 Windows 画的非客户区，学习中枢用的是 WebView2，
// 最大化/贴靠时那条原生区域会**压在网页内容上面**（用户看到「一条黑框一直挡着界面，
// 上面是最小化/窗口化/关闭」）。自绘之后窗口里只有我们自己的 DOM。
//
// 关于「最大化」：**不能用系统的 maximize()**。无边框窗口在 Windows 上 maximize
// 会铺满整个屏幕（含任务栏），底部那排（资料 / 模型 / 思考 / 发送）就永远被任务栏盖住。
// 所以统一按**显示器工作区**摆窗口：`monitor.workArea` 是物理像素、已排除任务栏，
// 多显示器也取当前那块屏。
// - 「最大化」= 填满工作区，并记住原尺寸供还原
// - 尺寸一变就检查：超出工作区就拉回来（Win+↑ 贴靠、系统快捷键都纠正得回来）
//
// 交互细节：
// - 整条是拖拽区（data-tauri-drag-region），按钮上不要加，否则拖不动窗口
// - 双击标题栏 = 最大化/还原
// - 需要 tauri.conf.json 里 `decorations: false`，以及 capabilities 里的窗口权限

import { useCallback, useEffect, useRef, useState } from "react";
import { currentMonitor, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { useApp } from "../store/app";
import { AppMark, Icon } from "./ui";

export function TitleBar() {
  const topicName = useApp((s) => s.topic?.meta.name ?? null);
  const [filled, setFilled] = useState(false);
  const restore = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const win = getCurrentWindow();

  /** 当前显示器的工作区（物理像素，已排除任务栏） */
  const workArea = useCallback(async () => {
    const mon = await currentMonitor();
    return mon?.workArea ?? null;
  }, []);

  /** 窗口超出工作区就拉回来——不管它是怎么变大的（贴靠、系统快捷键、上次遗留） */
  const clampToWorkArea = useCallback(async () => {
    const wa = await workArea();
    if (!wa) return;
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    const right = pos.x + size.width;
    const bottom = pos.y + size.height;
    const waRight = wa.position.x + wa.size.width;
    const waBottom = wa.position.y + wa.size.height;
    const fits =
      right <= waRight + 1 &&
      bottom <= waBottom + 1 &&
      pos.x >= wa.position.x - 1 &&
      pos.y >= wa.position.y - 1;
    if (fits) return;
    const w = Math.min(size.width, wa.size.width);
    const h = Math.min(size.height, wa.size.height);
    if (w !== size.width || h !== size.height) await win.setSize(new PhysicalSize(w, h));
    if (pos.x < wa.position.x || pos.y < wa.position.y || right > waRight || bottom > waBottom) {
      await win.setPosition(new PhysicalPosition(wa.position.x, wa.position.y));
    }
  }, [win, workArea]);

  useEffect(() => {
    void clampToWorkArea();
    let timer: number | null = null;
    let unlisten: (() => void) | null = null;
    void win
      .onResized(() => {
        if (timer !== null) window.clearTimeout(timer);
        timer = window.setTimeout(() => void clampToWorkArea(), 120);
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      unlisten?.();
    };
  }, [win, clampToWorkArea]);

  /** 最大化 = 填满工作区（不是系统 maximize，见文件头注释） */
  async function toggleFill() {
    const wa = await workArea();
    if (!wa) return;
    if (!filled) {
      const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
      restore.current = { x: pos.x, y: pos.y, w: size.width, h: size.height };
      await win.setSize(new PhysicalSize(wa.size.width, wa.size.height));
      await win.setPosition(new PhysicalPosition(wa.position.x, wa.position.y));
      setFilled(true);
    } else {
      const back = restore.current;
      if (back) {
        await win.setSize(new PhysicalSize(back.w, back.h));
        await win.setPosition(new PhysicalPosition(back.x, back.y));
      }
      setFilled(false);
    }
  }

  return (
    <div className="titlebar" data-tauri-drag-region onDoubleClick={() => void toggleFill()}>
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
      <button className="tb-btn" title={filled ? "还原" : "最大化"} onClick={() => void toggleFill()}>
        {filled ? (
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
