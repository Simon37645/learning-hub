// 拖放导入：把文件从资源管理器拖进窗口，直接复制到当前主题的 materials/。
//
// 这是「导入资料」最自然的方式——用户手里有 PDF 时，第一反应是拖进来，
// 而不是去找菜单。配合 materials/ 的目录约定，拖进来之后 agent 立刻就能读到。

import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useApp } from "../store/app";
import { Icon, useOverlay } from "./ui";

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
              // 没有主题就没地方放文件。顺带指一条更常用的路：贴图给 agent 看是 Ctrl+V
              state.toast(
                "warn",
                "拖进来的文件要有主题才有地方放——先在左侧选一个主题；只想给 agent 看一张图的话，直接在输入框里 Ctrl+V 粘贴就行",
              );
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

  // 本组件是常驻挂载的：useOverlay 只能挂在「真的在显示」的那层，
  // 不然没拖文件时也占着一个浮层计数，原生网页视图就永远显示不出来了
  if (!dragging) return null;
  return <DropOverlayCard topic={topic?.meta.name ?? null} count={count} />;
}

function DropOverlayCard({ topic, count }: { topic: string | null; count: number }) {
  useOverlay(); // 拖放提示也不能被原生子 WebView 盖住

  return (
    <div className="drop-overlay">
      <div className="drop-card">
        <Icon name="download" size={28} />
        <div className="drop-title">
          {topic ? `松开即导入到「${topic}」的 materials/` : "先打开一个主题，再拖文件进来"}
        </div>
        <div className="drop-sub">
          {count > 0 ? `${count} 个文件` : "文件"}会被复制进主题目录，agent 之后可以直接读它们
        </div>
      </div>
    </div>
  );
}
