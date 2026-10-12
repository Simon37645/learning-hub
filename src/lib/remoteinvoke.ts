// 替手机执行命令：桌面端前端这一侧。
//
// 手机上点「发送」时，命令是这么过来的：远程服务 → `hub://remote-invoke`
// （见 `src-tauri/src/remote/bridge.rs`）→ **在这里用同一个 invoke 执行**。
// 好处是手机和电脑跑的是同一批命令、同一份对话文件、同一套副作用，
// 不需要在 Rust 里手抄一张分派表，也就不会随时间漂移。
//
// 铁律和另外两条桥（`pdfshot.ts` 借前端渲染 PDF 页、远程渲染 Mermaid）一样：
// **两端都要回话**——执行成功要回，抛错也要回。不回的话手机那边会一直等到
// 30 秒超时，用户盯着一个不动的界面。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, errText } from "./api";

/** Rust 侧发过来的执行请求（字段名见 `remote::bridge::InvokeRequest`）。 */
interface RemoteInvokeRequest {
  id: string;
  cmd: string;
  args: Record<string, unknown> | null;
}

/**
 * 注册执行代理。**幂等**：`bootstrapStore` 有 initPromise 兜着，
 * 但这里再挡一道（StrictMode 会把 effect 跑两遍）。
 */
let listening: Promise<UnlistenFn> | null = null;

export function registerRemoteInvoker(): Promise<UnlistenFn> {
  listening ??= listen<RemoteInvokeRequest>("hub://remote-invoke", async (event) => {
    const { id, cmd, args } = event.payload;
    try {
      const value = await invoke(cmd, args ?? {});
      // `undefined` 过不了 JSON：统一成 null（远程那边本来也只当它是「没有返回值」）
      await api.remoteReportInvoke(id, true, value === undefined ? null : value);
    } catch (e) {
      await api.remoteReportInvoke(id, false, errText(e)).catch(() => {});
    }
  });
  return listening;
}
