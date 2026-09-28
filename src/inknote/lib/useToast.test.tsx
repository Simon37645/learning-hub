import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { notifyToast, useToast } from "./useToast";
import Toast from "../components/Toast";

afterEach(() => { vi.useRealTimers(); document.body.replaceChildren(); });

it("shows widget feedback in the existing toast and replaces its dismissal timer", () => {
  vi.useFakeTimers();
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  function Feedback() {
    const { message, toastKind } = useToast();
    return <Toast message={message} kind={toastKind} />;
  }
  try {
    act(() => root.render(<Feedback />));
    act(() => notifyToast("已复制", "success"));
    expect(host.querySelector(".toast-success")?.textContent).toBe("已复制");
    act(() => vi.advanceTimersByTime(3000));
    act(() => notifyToast("复制失败", "error"));
    act(() => vi.advanceTimersByTime(2000));
    expect(host.querySelector(".toast-error")?.textContent).toBe("复制失败");
    act(() => vi.advanceTimersByTime(2200));
    expect(host.querySelector(".toast")).toBeNull();
  } finally { act(() => root.unmount()); }
});
