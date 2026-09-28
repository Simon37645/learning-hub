// InkNote 文件 IO → 学习中枢 IPC 的适配层。
//
// InkNote 内部一律用**绝对路径**说话；学习中枢的约定是「主题目录 + 主题内相对路径」，
// 所以这里做一次双向映射：绝对路径 → (主题 slug, 相对路径)，再调后端命令。
// 只有三个函数是编辑器真正用到的：readFile / writeBinary / removePath。
//
// 编辑器当前的文档上下文（主题）由宿主组件通过 `setEditorDocumentContext` 注入。

import { invoke } from "@tauri-apps/api/core";

export interface EditorDocumentContext {
  /** 当前主题目录名 */
  slug: string;
  /** 当前主题目录的绝对路径 */
  dir: string;
}

let current: EditorDocumentContext | null = null;

export function setEditorDocumentContext(ctx: EditorDocumentContext | null): void {
  current = ctx;
}

export function getEditorDocumentContext(): EditorDocumentContext | null {
  return current;
}

/** 绝对路径 → 主题内相对路径（不在当前主题里就返回原样，交给后端报错） */
function toRelative(absPath: string): string {
  const dir = current?.dir;
  if (!dir) return absPath;
  const a = absPath.replace(/\\/g, "/");
  const d = dir.replace(/\\/g, "/").replace(/\/$/, "");
  if (a.toLowerCase().startsWith(d.toLowerCase() + "/")) {
    return a.slice(d.length + 1);
  }
  return absPath;
}

function needContext(): EditorDocumentContext {
  if (!current) {
    throw new Error("编辑器还没绑定主题目录");
  }
  return current;
}

export async function readFile(path: string): Promise<string> {
  const ctx = needContext();
  return invoke<string>("file_read_text", { topicSlug: ctx.slug, path: toRelative(path) });
}

export async function writeFile(path: string, content: string): Promise<void> {
  const ctx = needContext();
  await invoke("file_write_text", { topicSlug: ctx.slug, path: toRelative(path), content });
}

export async function writeBinary(path: string, bytes: number[]): Promise<void> {
  const ctx = needContext();
  await invoke("file_write_binary", { topicSlug: ctx.slug, path: toRelative(path), bytes });
}

export async function removePath(path: string): Promise<void> {
  const ctx = needContext();
  await invoke("file_delete", { topicSlug: ctx.slug, path: toRelative(path) });
}

export async function pathExists(path: string): Promise<boolean> {
  try {
    const ctx = needContext();
    return await invoke<boolean>("file_exists", { topicSlug: ctx.slug, path: toRelative(path) });
  } catch {
    return false;
  }
}
