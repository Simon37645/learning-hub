import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { attachVisualActions, copyVisualImage, openVisualPreview, openVisualViewer } from "./visualActions";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { writeImage, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Image as NativeImage } from "@tauri-apps/api/image";
import { toCanvas } from "html-to-image";
import { notifyToast } from "../../lib/useToast";

const closeImage = vi.fn();
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeImage: vi.fn(), writeText: vi.fn() }));
vi.mock("@tauri-apps/api/image", () => ({ Image: { new: vi.fn() } }));
vi.mock("html-to-image", () => ({ toCanvas: vi.fn() }));
vi.mock("../../lib/useToast", () => ({ notifyToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn() }));

beforeEach(() => {
  vi.mocked(isTauri).mockReturnValue(false);
  vi.mocked(invoke).mockResolvedValue(undefined);
  vi.mocked(writeImage).mockResolvedValue();
  vi.mocked(writeText).mockResolvedValue();
  vi.mocked(NativeImage.new).mockResolvedValue({ close: closeImage } as unknown as NativeImage);
  vi.mocked(toCanvas).mockResolvedValue({ width: 2, height: 2, getContext: () => ({ getImageData: () => ({ data: new Uint8ClampedArray(16) }) }) } as unknown as HTMLCanvasElement);
});
afterEach(() => {
  document.querySelector("dialog")?.dispatchEvent(new Event("cancel", { cancelable: true }));
  document.body.replaceChildren();
  vi.clearAllMocks();
});

function fixture() {
  const host = document.createElement("div");
  const target = document.createElement("div");
  target.innerHTML = '<span class="katex">中文 x²</span>';
  host.appendChild(target);
  document.body.appendChild(host);
  return { host, target };
}

describe("diagram and formula actions", () => {
  it("opens a native window with source data instead of an in-page modal on desktop", async () => {
    const { target } = fixture();
    vi.mocked(isTauri).mockReturnValue(true);
    await openVisualPreview(target, "math", "x^2");
    expect(invoke).toHaveBeenCalledWith("open_visual_preview", { data: {
      kind: "math", source: "x^2", locale: "zh", theme: "light",
    } });
    expect(document.querySelector("dialog")).toBeNull();
  });

  it("only shows zoom controls in the native window and retains fullscreen and escape shortcuts", async () => {
    const { target } = fixture();
    const controls = { locale: "en" as const, close: vi.fn().mockResolvedValue(undefined), fullscreen: vi.fn().mockResolvedValue(undefined), escape: vi.fn().mockResolvedValue(undefined) };
    openVisualViewer(target, controls);
    const dialog = document.querySelector("dialog")!;
    expect(dialog.classList.contains("visual-viewer--native")).toBe(true);
    expect([...dialog.querySelectorAll("button")].map((button) => button.textContent)).toEqual(["Zoom out", "Zoom in"]);
    expect(dialog.querySelector("output")).not.toBeNull();
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "F11", bubbles: true, cancelable: true }));
    dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
    await Promise.resolve();
    expect(controls.fullscreen).toHaveBeenCalledOnce();
    expect(controls.escape).toHaveBeenCalledOnce();
    // Native close normally destroys this page; simulate it in the DOM test.
    dialog.remove();
  });

  it("copies current source without forwarding mousedown into the editor", async () => {
    const { host, target } = fixture();
    let code = "graph TD\n A --> B";
    attachVisualActions(host, target, () => code, "mermaid");
    const edit = vi.fn();
    host.addEventListener("mousedown", edit);
    const copy = host.querySelectorAll("button")[2];
    copy.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(edit).not.toHaveBeenCalled();
    code = "graph TD\n C --> D";
    copy.click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("```mermaid\ngraph TD\n C --> D\n```"));
    expect(notifyToast).toHaveBeenCalledWith("已复制", "success");
    expect(host.querySelector(".md-visual-status")).toBeNull();
  });

  it("copies pixels using the native clipboard and releases image resources", async () => {
    const { target } = fixture();
    await copyVisualImage(target);
    expect(NativeImage.new).toHaveBeenCalledWith(expect.any(Uint8Array), 2, 2);
    expect(writeImage).toHaveBeenCalledOnce();
    expect(closeImage).toHaveBeenCalledOnce();
    expect(document.querySelector(".visual-export")).toBeNull();
  });

  it("cleans up on clipboard failure and shows failure instead of success", async () => {
    const { host, target } = fixture();
    vi.mocked(writeImage).mockRejectedValue(new Error("unavailable"));
    attachVisualActions(host, target, () => "x^2", "math");
    host.querySelectorAll("button")[1].click();
    await vi.waitFor(() => expect(notifyToast).toHaveBeenCalledWith(expect.stringContaining("操作未完成"), "error"));
    expect(host.querySelector(".md-visual-status")).toBeNull();
    expect(closeImage).toHaveBeenCalledOnce();
    expect(document.querySelector(".visual-export")).toBeNull();
  });

  it("opens a separate viewer, zooms, and closes without mutating the original", () => {
    const { target } = fixture();
    const before = target.outerHTML;
    openVisualViewer(target);
    const dialog = document.querySelector("dialog")!;
    expect(dialog.open).toBe(true);
    const buttons = [...dialog.querySelectorAll("button")];
    buttons.find((b) => b.textContent === "100%")!.click();
    buttons.find((b) => b.textContent === "放大")!.click();
    expect(dialog.querySelector("output")?.textContent).toBe("125%");
    expect(target.outerHTML).toBe(before);
    dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
    expect(document.querySelector("dialog")).toBeNull();
  });

  it("refuses an unrendered diagram without touching the clipboard", async () => {
    const { target } = fixture();
    target.dataset.visualReady = "false";
    await expect(copyVisualImage(target)).rejects.toThrow();
    expect(writeImage).not.toHaveBeenCalled();
  });
});
