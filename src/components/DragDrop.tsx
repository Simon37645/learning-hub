// 拖放导入：把文件从资源管理器拖进窗口，直接复制到当前主题的 materials/。
//
// 这是「导入资料」最自然的方式——用户手里有 PDF 时，第一反应是拖进来，
// 而不是去找菜单。配合 materials/ 的目录约定，拖进来之后 agent 立刻就能读到。

import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useApp } from "../store/app";
import { Icon } from "./ui";

export function useDragDropImport() {
  const [dragging, setDragging] = useState(false);
  const [count, setCount] = useState(0);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      try {
        const handle = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload as {
            type: string;
            paths?: string[];
          };
          if (payload.type === "enter" || payload.type === "over") {
            setDragging(true);
            setCount(payload.paths?.length ?? 0);
          } else if (payload.type === "leave") {
            setDragging(false);
            setCount(0);
          } else if (payload.type === "drop") {
            setDragging(false);
            setCount(0);
            const paths = payload.paths ?? [];
            if (paths.length === 0) return;
            const state = useApp.getState();
            if (!state.topic) {
              state.toast("warn", "先打开一个主题，拖进来的文件才有地方放");
              return;
            }
            void state.importMaterials(paths);
          }
        });
        if (cancelled) {
          handle();
        } else {
          unlisten = handle;
        }
      } catch (e) {
        // 浏览器里调试时没有这个 API，忽略即可
        console.warn("拖放导入不可用", e);
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return { dragging, count };
}

/** 拖拽时盖在窗口上的提示层 */
export function DropOverlay() {
  const { dragging, count } = useDragDropImport();
  const topic = useApp((s) => s.topic);

  if (!dragging) return null;

  return (
    <div className="drop-overlay">
      <div className="drop-card">
        <Icon name="download" size={28} />
        <div className="drop-title">
          {topic ? `松开即导入到「${topic.meta.name}」的 materials/` : "先打开一个主题，再拖文件进来"}
        </div>
        <div className="drop-sub">
          {count > 0 ? `${count} 个文件` : "文件"}会被复制进主题目录，agent 之后可以直接读它们
        </div>
      </div>
    </div>
  );
}
