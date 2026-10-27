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
// - 尺寸一变就检查：贴着工作区原点、略微超出的（边框差顶出去的那种）拉回来；
//   用户主动摆放的跨屏/超大窗口不碰（见 clampToWorkArea 的说明）
//
// 交互细节：
// - 整条是拖拽区（data-tauri-drag-region），按钮上不要加，否则拖不动窗口
// - 双击标题栏 = 最大化/还原
// - 需要 tauri.conf.json 里 `decorations: false`，以及 capabilities 里的窗口权限
//   （**漏一项就是静默失效**，`lib.rs` 里有个测试盯着这张清单）
//
// 远程（手机浏览器）里这条的作用完全不同：没有窗口可控制，它变成
// 「品牌 + 当前主题 + 连接状态 + ☰（窄屏展开侧栏）」的一行。

import { useCallback, useEffect, useRef, useState } from "react";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { fitOuter } from "../lib/winfit";
import { isRemote, remoteBridge } from "../lib/remote";
import { useApp } from "../store/app";
import { AppMark, Icon } from "./ui";

export function TitleBar() {
  const topicName = useApp((s) => s.topic?.meta.name ?? null);
  const sidebarOpen = useApp((s) => s.sidebarOpen);
  const setSidebarOpen = useApp((s) => s.setSidebarOpen);
  const [filled, setFilled] = useState(false);
  const restore = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const remote = isRemote();
  const win = remote ? null : getCurrentWindow();

  /** 当前显示器的工作区（物理像素，已排除任务栏） */
  const workArea = useCallback(async () => {
    const mon = await currentMonitor();
    return mon?.workArea ?? null;
  }, []);

  /**
   * 窗口超出工作区就拉回来——**只认「我们自己摆出来的」那类越界**。
   *
   * 这条兜底（eb41f66）修的是 setSize 内容区/外框口径差造成的约 22px 突出：
   * 那类越界的特征是位置就贴着工作区左上角（fitOuter 的落点）、只大出去一点点。
   * 用户**主动**的摆放——拉伸窗口跨过屏边界、拖到缩放不同的第二块屏——位置是
   * 任意的，一律钳回去等于挫败摆放意图（多显示器下松手 120ms 就被弹回主体屏）。
   * 所以位置不在工作区原点附近的越界视为用户自己的意图，不动。
   */
  const clampToWorkArea = useCallback(async () => {
    if (!win) return;
    // 专注模式（F11）期间不插手：系统刚把窗口铺满整块屏，这里再按「工作区」拉一下
    // 会把它缩成工作区大小（任务栏露出来），多显示器时还可能摆到别的屏上。
    if (useApp.getState().zen) return;
    if (await win.isFullscreen().catch(() => false)) return;
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
    // 只修「贴着工作区原点、略微超界」的形态（边框差 / DPI 换算把我们 fit 好的
    // 窗口顶出去一点）。位置在别处 = 贴靠、系统快捷键、用户自己拖的——都不碰。
    const atOrigin =
      Math.abs(pos.x - wa.position.x) <= 24 && Math.abs(pos.y - wa.position.y) <= 24;
    if (!atOrigin) return;
    // 尺寸与位置一起按**外框**口径设：setSize 设的是内容区，拿工作区尺寸直接设
    // 会永远差那圈边框（见 lib/winfit.ts），窗口就一直是「大出去一点点」的状态。
    const w = Math.min(size.width, wa.size.width);
    const h = Math.min(size.height, wa.size.height);
    await fitOuter(win, wa.position.x, wa.position.y, w, h);
  }, [win, workArea]);

  useEffect(() => {
    if (!win) return;
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
    if (!win) return;
    const wa = await workArea();
    if (!wa) return;
    if (!filled) {
      const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
      restore.current = { x: pos.x, y: pos.y, w: size.width, h: size.height };
      await fitOuter(win, wa.position.x, wa.position.y, wa.size.width, wa.size.height);
      setFilled(true);
    } else {
      const back = restore.current;
      if (back) await fitOuter(win, back.x, back.y, back.w, back.h);
      setFilled(false);
    }
  }

  return (
    <div
      className="titlebar"
      data-tauri-drag-region={remote ? undefined : true}
      onDoubleClick={() => void toggleFill()}
    >
      {remote && (
        <button
          className="tb-btn tb-menu"
          title={sidebarOpen ? "收起列表" : "展开列表"}
          onClick={() => setSidebarOpen(!sidebarOpen)}
        >
          <Icon name="list" size={14} />
        </button>
      )}
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
      {remote ? (
        <RemoteBadge />
      ) : (
        <>
          <button className="tb-btn" title="最小化" onClick={() => void win?.minimize()}>
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
          <button className="tb-btn close" title="关闭" onClick={() => void win?.close()}>
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <path d="M0 0l10 10M10 0L0 10" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
        </>
      )}
    </div>
  );
}

/**
 * 远程模式下右上角那一点：绿=事件流连着，灰=断了（浏览器自己会重连）。
 *
 * 为什么要在界面上留这个：手机上最难受的故障是「看着像好的、其实已经不更新了」，
 * 有个一眼能看出来的指示器，用户就知道该下拉刷新还是去检查电脑那边。
 */
function RemoteBadge() {
  const [on, setOn] = useState(true);
  useEffect(() => {
    const tick = () => setOn(remoteBridge()?.connected !== false);
    tick();
    const t = window.setInterval(tick, 2000);
    return () => window.clearInterval(t);
  }, []);
  return (
    <span className="tb-remote" title={on ? "已连上电脑（远程控制）" : "连接断了，正在重连…"}>
      <span className={"dot" + (on ? " on" : "")} />
      远程
    </span>
  );
}

/** 窗口顶部的拖拽区图标（用内置 Icon 也行，这里保持标题栏零依赖） */
export function TitleBarHint() {
  return <Icon name="hub" size={12} />;
}
