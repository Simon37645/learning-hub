// 应用外壳：布局、视图路由、全局快捷键。

import { useEffect, useState } from "react";
import { useApp } from "./store/app";
import { Sidebar, SearchPalette } from "./components/Sidebar";
import { SidebarResizer } from "./components/SidebarResizer";
import { Chat } from "./components/Chat";
import { Workbench } from "./components/Workbench";
import { ViewerPanel } from "./components/Viewer";
import { Agenda } from "./components/Agenda";
import { Settings } from "./components/Settings";
import { AppMark, Icon, Toasts } from "./components/ui";
import { DropOverlay } from "./components/DragDrop";
import { STAGE_LABEL } from "./lib/types";

export default function App() {
  const ready = useApp((s) => s.ready);
  const bootError = useApp((s) => s.bootError);
  const view = useApp((s) => s.view);
  const topic = useApp((s) => s.topic);

  useEffect(() => {
    void useApp.getState().init();
  }, []);

  // 全局快捷键
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        useApp.getState().setPaletteOpen(!useApp.getState().paletteOpen);
      } else if (mod && e.key.toLowerCase() === "b") {
        e.preventDefault();
        void useApp.getState().toggleViewer();
      } else if (mod && e.key === ",") {
        e.preventDefault();
        useApp.getState().setView("settings");
      } else if (mod && e.shiftKey && e.key.toLowerCase() === "o") {
        e.preventDefault();
        useApp.getState().setView("agenda");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (bootError) {
    return (
      <div className="boot">
        <div className="box">
          <AppMark size={90} color="var(--text-faint)" />
          <h1>启动失败</h1>
          <p>{bootError}</p>
          <button className="btn primary" onClick={() => window.location.reload()}>
            重试
          </button>
        </div>
      </div>
    );
  }

  if (!ready) {
    return (
      <div className="boot">
        <div className="box">
          <AppMark size={90} color="var(--text-faint)" />
          <p>正在准备工作区…</p>
        </div>
      </div>
    );
  }

  return (
    <div className="app">
      <Sidebar />
      <SidebarResizer />
      <div className="app-main">
        <div className="main-body">
          <div className="main-center">
            {view === "settings" ? (
              <Settings />
            ) : view === "agenda" ? (
              <Agenda />
            ) : topic ? (
              <TopicSurface />
            ) : (
              <Chat />
            )}
          </div>
          <ViewerPanel />
        </div>
      </div>
      <SearchPalette />
      <DropOverlay />
      <Toasts />
    </div>
  );
}

/** 打开主题后，主区域在「对话」和「工作台」之间切换。 */
function TopicSurface() {
  const slug = useApp((s) => s.topic?.slug);
  const topic = useApp((s) => s.topic)!;
  const streaming = useApp((s) => s.streaming);
  const [tab, setTab] = useState<"chat" | "workbench">("chat");

  // 换主题时回到对话
  useEffect(() => setTab("chat"), [slug]);

  return (
    <>
      <div className="chat-head" style={{ borderBottom: tab === "chat" ? "none" : undefined }}>
        <div className="segmented">
          <button className={tab === "chat" ? "on" : ""} onClick={() => setTab("chat")}>
            <Icon name="chat" size={12} /> 对话
          </button>
          <button className={tab === "workbench" ? "on" : ""} onClick={() => setTab("workbench")}>
            <Icon name="layers" size={12} /> 工作台
          </button>
        </div>
        <span className="tag">{STAGE_LABEL[topic.meta.stage]}阶段</span>
        {streaming && (
          <span className="row muted" style={{ fontSize: 11.5, gap: 6 }}>
            <span className="spinner" /> agent 正在工作
          </span>
        )}
        <div className="spacer" />
        <span className="muted mono" style={{ fontSize: 11 }}>
          {topic.stats.cardsDue > 0 ? `${topic.stats.cardsDue} 张卡片待复习` : ""}
        </span>
      </div>

      {tab === "chat" ? <Chat /> : <Workbench />}
    </>
  );
}
