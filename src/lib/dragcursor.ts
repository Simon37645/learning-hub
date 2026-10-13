// 拖分界线时的光标接管：按住期间 body 上挂 col-resize + 禁止选中，松手还回去。
//
// 两个手柄共用这一份：侧栏分界线（SidebarResizer）与内置浏览器分界线（Viewer）。
//
// 为什么要抽出来：原来的写法把「还光标」和「这次拖动是不是我的」绑在一句话里——
// `if (!el.hasPointerCapture(id)) return;` 之后才 `document.body.style.cursor = ""`。
// 而 `setPointerCapture` 是会失败的（合成事件、指针已失效、元素中途卸载），
// 一旦失败光标就**永远停在 col-resize**（用户报的「拖动分界线后光标回不来」）。
//
// 现在的分工：
// - 「是不是我的拖动」看自己的 pointerId（不看 capture，capture 只用来在手滑出去之后
//   还能收到 pointermove）；
// - 「还光标」**无条件做**，而且 pointerup / pointercancel / lostpointercapture /
//   窗口失焦 / 组件卸载五条路都做——只要有一条走到，光标就不会卡住。

import { useCallback, useEffect, useRef } from "react";

interface PointerLike {
  pointerId: number;
}

export function useDragCursor() {
  const activeId = useRef<number | null>(null);

  /** 按下时调：接管光标。 */
  const begin = useCallback((e: PointerLike) => {
    activeId.current = e.pointerId;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  /** 收尾：带上 pointerId 时只认「当前这次拖动」（别的指针按下/抬起不该打断它），
   *  不带时（窗口失焦、卸载）无条件收。 */
  const end = useCallback((e?: PointerLike) => {
    if (activeId.current === null) return;
    if (e && e.pointerId !== activeId.current) return;
    activeId.current = null;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }, []);

  /** 这次 pointer 事件属不属于当前这次拖动（拖动中用，替代 hasPointerCapture）。 */
  const isDragging = useCallback((e: PointerLike) => activeId.current === e.pointerId, []);

  // 兜底：指针在窗口外松开、或事件被原生子 WebView 吃掉时，pointerup 到不了手柄上，
  // 光标就回不来。这几条挂在 window 上，只管把光标还回去（不碰宽度）。
  useEffect(() => {
    const onUp = (e: PointerEvent) => end(e);
    const onBlur = () => end();
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      window.removeEventListener("blur", onBlur);
      // 手柄本身被卸载（内置浏览器被收起、窄屏隐藏分界线）也要还回去
      end();
    };
  }, [end]);

  return { begin, end, isDragging };
}
