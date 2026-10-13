// 回归网：拖完分界线光标必须还回来。
//
// 真踩过：手柄上把「还光标」写在 `if (!hasPointerCapture(id)) return` 后面，
// 而 setPointerCapture 失败时（合成事件、指针失效、元素被卸载）那句就永远不执行，
// 光标一直停在 col-resize。所以这里专门盯「在别处松手」这条路。

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it } from "vitest";
import { useDragCursor } from "./dragcursor";

afterEach(() => {
  document.body.replaceChildren();
  document.body.style.cursor = "";
  document.body.style.userSelect = "";
});

/** pointerId 显式给，才验得了「别的指针按下/抬起不该打断这次拖动」。 */
function pev(type: string, pointerId: number): PointerEvent {
  const e = new PointerEvent(type, { bubbles: true, cancelable: true });
  Object.defineProperty(e, "pointerId", { value: pointerId });
  return e;
}

function mountHandle() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  function Handle() {
    const { begin, end, isDragging } = useDragCursor();
    return (
      <div
        id="h"
        onPointerDown={(e) => begin(e)}
        onPointerUp={(e) => end(e)}
        onPointerMove={(e) => {
          if (isDragging(e)) host.dataset.dragging = String(Number(host.dataset.dragging ?? "0") + 1);
        }}
      />
    );
  }
  act(() => root.render(<Handle />));
  return { host, el: host.querySelector("#h") as HTMLElement, root };
}

it("按住时接管光标，松手还回来", () => {
  const { el, root } = mountHandle();
  act(() => {
    el.dispatchEvent(pev("pointerdown", 7));
  });
  expect(document.body.style.cursor).toBe("col-resize");
  expect(document.body.style.userSelect).toBe("none");

  act(() => {
    el.dispatchEvent(pev("pointerup", 7));
  });
  expect(document.body.style.cursor).toBe("");
  expect(document.body.style.userSelect).toBe("");

  act(() => root.unmount());
});

it("指针在手柄外（window 上）松开也会还回来", () => {
  const { el, root } = mountHandle();
  act(() => {
    el.dispatchEvent(pev("pointerdown", 7));
  });
  expect(document.body.style.cursor).toBe("col-resize");

  // 事件只到 window：模拟「松开时指针在原生子 WebView / 窗口外」
  act(() => {
    window.dispatchEvent(pev("pointerup", 7));
  });
  expect(document.body.style.cursor).toBe("");
  act(() => root.unmount());
});

it("别的指针按下/抬起不打断这次拖动，窗口失焦才收", () => {
  const { el, host, root } = mountHandle();
  act(() => {
    el.dispatchEvent(pev("pointerdown", 7));
  });
  act(() => {
    window.dispatchEvent(pev("pointerup", 99));
  });
  expect(document.body.style.cursor).toBe("col-resize");

  // 拖动中：自己的 pointermove 算数，别人的不算
  act(() => {
    el.dispatchEvent(pev("pointermove", 7));
    el.dispatchEvent(pev("pointermove", 99));
  });
  expect(host.dataset.dragging).toBe("1");

  act(() => {
    window.dispatchEvent(new Event("blur"));
  });
  expect(document.body.style.cursor).toBe("");
  act(() => root.unmount());
});

it("手柄被卸载（比如内置浏览器收起）时也还回来", () => {
  const { el, root } = mountHandle();
  act(() => {
    el.dispatchEvent(pev("pointerdown", 7));
  });
  expect(document.body.style.cursor).toBe("col-resize");
  act(() => root.unmount());
  expect(document.body.style.cursor).toBe("");
});
