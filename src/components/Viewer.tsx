// 内置浏览器面板：标签页 + 工具栏 + 按类型分发的渲染器。
//
// agent 侧的 `viewer_*` 工具会通过 `hub://viewer` 事件驱动这里：
// 打开标签、跳页、检索高亮、请求内容快照。人在这里也能正常操作。

import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { api, errText } from "../lib/api";
import type { TabView, ViewerKind } from "../lib/types";
import { useApp } from "../store/app";
import { Empty, Icon, Spinner } from "./ui";import { Markdown } from "./Chat";
import { PdfView } from "./PdfView";

export function ViewerPanel() {
  const viewer = useApp((s) => s.viewer);
  const width = useApp((s) => s.viewerWidth);
  const setWidth = useApp((s) => s.setViewerWidth);
  const toggleViewer = useApp((s) => s.toggleViewer);
  const activateTab = useApp((s) => s.activateTab);
  const closeTab = useApp((s) => s.closeTab);
  const goto = useApp((s) => s.goto);

  const dragging = useRef(false);
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const w = window.innerWidth - e.clientX;
      setWidth(w);
    };
    const onUp = () => {
      dragging.current = false;
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setWidth]);

  if (!viewer.visible) return null;

  const active = viewer.tabs.find((t) => t.id === viewer.activeId) ?? null;

  return (
    <aside className="viewer" style={{ width }}>
      <div
        className="viewer-resizer"
        onMouseDown={() => {
          dragging.current = true;
          document.body.style.cursor = "col-resize";
        }}
      />

      <div className="viewer-head">
        <div className="viewer-tabs">
          {viewer.tabs.map((t) => (
            <div
              key={t.id}
              className={"vtab" + (t.id === viewer.activeId ? " on" : "")}
              onClick={() => void activateTab(t.id)}
              title={t.path ?? t.url ?? t.title}
            >
              <KindIcon kind={t.kind} />
              <span className="vtitle">{t.title}</span>
              <button
                className="icon-btn"
                style={{ width: 16, height: 16 }}
                onClick={(e) => {
                  e.stopPropagation();
                  void closeTab(t.id);
                }}
                title="关闭"
              >
                <Icon name="close" size={10} />
              </button>
            </div>
          ))}
        </div>
        <button
          className="icon-btn"
          title="新标签页"
          onClick={async () => {
            try {
              await api.viewerOpenHome();
              const snap = await api.viewerSnapshot();
              useApp.setState({ viewer: snap });
            } catch (e) {
              useApp.getState().toast("error", errText(e));
            }
          }}
        >
          <Icon name="plus" />
        </button>

        {/* 本地文档的状态与动作并到这一行：原来下面还有一条路径栏，
            白白多占一行、还把正文挤掉一截（用户反馈「顶上的框挡住内容」）。
            完整路径现在放在标签的 tooltip 里。 */}
        {active && active.kind !== "web" && (
          <>
            {active.kind === "pdf" && (
              <span className="mono nowrap" style={{ fontSize: 11 }}>
                {active.page}/{active.totalPages || "?"}
              </span>
            )}
            <span className="muted mono nowrap" style={{ fontSize: 10.5 }} title="agent 已读到的正文字数">
              {active.snapshotChars > 0 ? `${(active.snapshotChars / 1000).toFixed(1)}k 字` : "未读"}
            </span>
            {active.path && (
              <>
                <button
                  className="icon-btn"
                  title="重新加载"
                  onClick={() => void api.viewerReload(active.id).catch((e) => useApp.getState().toast("error", errText(e)))}
                >
                  <Icon name="refresh" size={13} />
                </button>
                <button
                  className="icon-btn"
                  title="用系统程序打开"
                  onClick={() =>
                    void api
                      .openWithSystem(active.topicSlug ?? "", active.path ?? "")
                      .catch((e) => useApp.getState().toast("error", errText(e)))
                  }
                >
                  <Icon name="external" size={13} />
                </button>
              </>
            )}
          </>
        )}

        <button
          className="icon-btn"
          title="收起内置浏览器"
          onClick={() => void toggleViewer(false)}
        >
          <Icon name="close" />
        </button>
      </div>

      {active ? (
        <TabBody key={active.id} tab={active} goto={goto?.tabId === active.id ? goto : null} />
      ) : (
        <div className="viewer-body">
          <Empty icon="globe">
            内置浏览器是空的。
            <br />
            agent 打开 PDF 或网页时会显示在这里，
            <br />
            也可以用上面的「＋」自己开一个。
          </Empty>
        </div>
      )}
    </aside>
  );
}

function KindIcon({ kind }: { kind: ViewerKind }) {
  const name = kind === "pdf" ? "file" : kind === "web" ? "globe" : kind === "image" ? "eye" : "file";
  return <Icon name={name as "file"} size={12} />;
}

// ---------------------------------------------------------------- 标签内容

function TabBody({
  tab,
  goto,
}: {
  tab: TabView;
  goto: { page: number | null; scroll: number | null; anchor: string | null; highlight: string | null } | null;
}) {
  const [readerMode, setReaderMode] = useState(false);

  return (
    <>
      {/* 只有网页才需要地址栏那一行；本地文档的状态已经并到顶部标签行里了 */}
      {tab.kind === "web" && <TabBar tab={tab} readerMode={readerMode} setReaderMode={setReaderMode} />}
      {tab.loading && (
        <div className="row" style={{ padding: "6px 12px", color: "var(--text-faint)", fontSize: 12 }}>
          <Spinner /> 加载中…
        </div>
      )}
      {tab.error && <div className="viewer-note">读取失败：{tab.error}</div>}

      <div className="viewer-body">
        {tab.kind === "pdf" ? (
          <PdfView tab={tab} highlight={goto?.highlight ?? null} />
        ) : tab.kind === "markdown" ? (
          <DocumentView tab={tab} goto={goto} kind="markdown" />
        ) : tab.kind === "text" ? (
          <DocumentView tab={tab} goto={goto} kind="text" />
        ) : tab.kind === "image" ? (
          <ImageView tab={tab} />
        ) : tab.kind === "web" ? (
          <WebTab tab={tab} readerMode={readerMode} setReaderMode={setReaderMode} />
        ) : (
          <NewTabPage />
        )}
      </div>
    </>
  );
}

function TabBar({
  tab,
  readerMode,
  setReaderMode,
}: {
  tab: TabView;
  readerMode: boolean;
  setReaderMode: (v: boolean) => void;
}) {
  const toast = useApp((s) => s.toast);
  const [addr, setAddr] = useState(tab.url ?? "");
  useEffect(() => setAddr(tab.url ?? ""), [tab.url]);

  return (
    <div className="viewer-bar">
      {tab.kind === "web" ? (
        <>
          <Icon name="globe" size={13} />
          <input
            className="input"
            style={{ flex: 1, minWidth: 0, padding: "3px 8px", fontSize: 11.5, fontFamily: "var(--font-mono)" }}
            value={addr}
            placeholder="输入网址后回车"
            onChange={(e) => setAddr(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && addr.trim()) {
                void useApp.getState().openTab({ url: addr.trim() });
              }
            }}
          />
          <button
            className={"icon-btn" + (readerMode ? " active" : "")}
            title="阅读模式（显示服务端提取的正文，也是 agent 读到的内容）"
            onClick={() => setReaderMode(!readerMode)}
          >
            <Icon name="list" size={13} />
          </button>
          <button
            className="icon-btn"
            title="在系统浏览器中打开"
            onClick={() => tab.url && void openExternal(tab.url)}
          >
            <Icon name="external" size={13} />
          </button>
        </>
      ) : (
        <span className="url" title={tab.path ?? ""}>
          {tab.path ?? tab.title}
        </span>
      )}

      {tab.kind === "pdf" && (
        <span className="mono nowrap" style={{ fontSize: 11 }}>
          {tab.page}/{tab.totalPages || "?"}
        </span>
      )}

      <span className="muted mono nowrap" style={{ fontSize: 10.5 }} title="agent 已读到的正文字数">
        {tab.snapshotChars > 0 ? `${(tab.snapshotChars / 1000).toFixed(1)}k 字` : "未读"}
      </span>

      {tab.path && (
        <>
          <button
            className="icon-btn"
            title="重新加载"
            onClick={() => void api.viewerReload(tab.id).catch((e) => toast("error", errText(e)))}
          >
            <Icon name="refresh" size={13} />
          </button>
          <button
            className="icon-btn"
            title="用系统程序打开"
            onClick={() =>
              void api
                .openWithSystem(tab.topicSlug ?? "", tab.path ?? "")
                .catch((e) => toast("error", errText(e)))
            }
          >
            <Icon name="external" size={13} />
          </button>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- Markdown / 文本

function DocumentView({
  tab,
  goto,
  kind,
}: {
  tab: TabView;
  goto: { scroll: number | null; anchor: string | null; highlight: string | null } | null;
  kind: "markdown" | "text";
}) {
  const [text, setText] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const reloadSeq = useApp((s) => s.reloadSeq[tab.id] ?? 0);

  useEffect(() => {
    let cancelled = false;
    api
      .viewerLoadText(tab.id)
      .then((t) => !cancelled && setText(t))
      .catch((e) => !cancelled && setErr(errText(e)));
    return () => {
      cancelled = true;
    };
  }, [tab.id, reloadSeq]);

  // 滚动定位 + 高亮
  useEffect(() => {
    const root = ref.current;
    if (!root || !text) return;
    if (goto?.scroll != null) {
      root.scrollTop = root.scrollHeight * goto.scroll;
    }
    if (goto?.anchor) {
      const headings = Array.from(root.querySelectorAll("h1,h2,h3,h4,h5,h6"));
      const target = headings.find((h) => (h.textContent ?? "").includes(goto.anchor!));
      target?.scrollIntoView({ block: "start" });
    }
    if (goto?.highlight) highlightText(root, goto.highlight);
  }, [goto, text]);

  if (err) return <div className="viewer-note">{err}</div>;
  if (text === null)
    return (
      <div className="empty" style={{ height: "100%" }}>
        <Spinner />
      </div>
    );

  return (
    <div className="viewer-scroll" ref={ref}>
      {kind === "markdown" ? (
        <div className="reader">
          <Markdown source={text} onLink={(href) => void useApp.getState().openUrl(href)} />
        </div>
      ) : (
        <pre
          style={{
            margin: 0,
            padding: 14,
            fontFamily: "var(--font-mono)",
            fontSize: 11.5,
            lineHeight: 1.7,
            whiteSpace: "pre-wrap",
            overflowWrap: "anywhere",
          }}
        >
          {text}
        </pre>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 图片

function ImageView({ tab }: { tab: TabView }) {
  const [url, setUrl] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let objectUrl: string | null = null;
    api
      .viewerLoadBytes(tab.id)
      .then((buf) => {
        objectUrl = URL.createObjectURL(new Blob([buf]));
        setUrl(objectUrl);
      })
      .catch((e) => setErr(errText(e)));
    return () => {
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [tab.id]);

  if (err) return <div className="viewer-note">{err}</div>;
  if (!url)
    return (
      <div className="empty" style={{ height: "100%" }}>
        <Spinner />
      </div>
    );

  return (
    <div style={{ padding: 12, display: "grid", placeItems: "center", height: "100%" }}>
      <img src={url} alt={tab.title} style={{ maxWidth: "100%", maxHeight: "100%", borderRadius: 6 }} />
    </div>
  );
}

// ---------------------------------------------------------------- 网页

/**
 * 网页标签的渲染。远端网页默认走**原生子 WebView**（后端 `viewer::webview` 模块，
 * 挂在主窗口上的真 WebView2，不受 X-Frame-Options / CSP frame-ancestors 约束）；
 * 这里的宿主 div 只负责占位与量矩形，页面画在 DOM 之上的原生层里。
 *
 * 所以这个组件要管三件事：
 * 1. 什么时候显示 / 隐藏原生层——阅读模式、专注模式、有全屏浮层时都要让路；
 * 2. 宿主矩形变了把 bounds 同步给后端（CSS px，Rust 按 scale_factor 换算）；
 * 3. 原生视图创建失败时退回原来的 iframe 兜底路径（含 frame_block 自动切阅读模式）。
 */
function WebTab({
  tab,
  readerMode,
  setReaderMode,
}: {
  tab: TabView;
  readerMode: boolean;
  setReaderMode: (v: boolean) => void;
}) {
  const [reader, setReader] = useState<{ text: string | null; err: string | null; loading: boolean }>({
    text: null,
    err: null,
    loading: false,
  });
  const [localHtml, setLocalHtml] = useState<string | null>(null);
  /** iframe 兜底路径下站点拒绝内嵌的原因；非空说明已经自动切到阅读模式了 */
  const [frameBlock, setFrameBlock] = useState<string | null>(null);
  /** 原生视图创建失败 → 退回 iframe 兜底（本标签内不再重试） */
  const [nativeFailed, setNativeFailed] = useState(false);
  const isLocal = !!tab.path;
  const zen = useApp((s) => s.zen);
  const overlayCount = useApp((s) => s.overlayCount);
  const setNativeWebviewUp = useApp((s) => s.setNativeWebviewUp);
  const reloadSeq = useApp((s) => s.reloadSeq[tab.id] ?? 0);
  const hostRef = useRef<HTMLDivElement>(null);

  const useNative = !isLocal && !nativeFailed;
  const canShow = useNative && !readerMode && !zen && overlayCount === 0;
  // ensure 是异步的：期间条件可能已经翻转（比如刚弹出浮层），完成时要按最新状态收场
  const canShowRef = useRef(canShow);
  canShowRef.current = canShow;

  // 显示 / 隐藏的编排：条件不满足就让原生层让路（隐藏不销毁，页面状态保留）。
  // 这是 zen / 阅读模式 / 全屏浮层三条让路路径的唯一入口——条件翻 false 时必须
  // 显式 hide，原生子 WebView 是独立原生窗口，不会因为 DOM 被盖住就自己消失。
  useEffect(() => {
    if (!useNative) return;
    const hide = () => {
      setNativeWebviewUp(false);
      void api.viewerWebviewSetVisible(tab.id, false).catch(() => {});
    };
    if (!canShow || !tab.url) {
      hide();
      return;
    }
    const host = hostRef.current;
    // 原生路径下宿主 div 一定已挂载；万一这一拍还没提交，等下一轮依赖变化再来
    if (!host) return;
    const rect = host.getBoundingClientRect();
    if (rect.width < 2 || rect.height < 2) {
      hide(); // 面板被收成一条缝：藏起来，别摆一个 1×1 的原生层出来
      return;
    }
    api
      .viewerWebviewEnsure(tab.id, tab.url, { x: rect.left, y: rect.top, w: rect.width, h: rect.height })
      .then(() => {
        if (canShowRef.current) setNativeWebviewUp(true);
        else hide(); // ensure 在飞的这会儿浮层弹出来了：别把网页亮在浮层上面
      })
      .catch((e) => {
        // 创建失败（平台不支持等）：退回 iframe 兜底，别让用户对着一片空白
        console.warn("原生网页视图创建失败，退回内嵌渲染", e);
        setNativeWebviewUp(false);
        setNativeFailed(true);
      });
  }, [useNative, canShow, tab.id, tab.url, setNativeWebviewUp]);

  // 宿主矩形变化（窗口缩放、拖侧栏、面板收展）→ 同步 bounds。
  // rAF 合并高频回调，拖动时不刷爆 IPC；位置尺寸没变就不发。
  useEffect(() => {
    if (!useNative) return;
    const host = hostRef.current;
    if (!host) return;
    let raf = 0;
    let last = "";
    const sync = () => {
      raf = 0;
      const rect = host.getBoundingClientRect();
      const key = `${rect.left.toFixed(1)}|${rect.top.toFixed(1)}|${rect.width.toFixed(1)}|${rect.height.toFixed(1)}`;
      if (key === last) return;
      last = key;
      // 太小交给上面那个 effect 走隐藏分支，这里不白传 bounds
      if (rect.width < 2 || rect.height < 2) return;
      void api
        .viewerWebviewBounds(tab.id, { x: rect.left, y: rect.top, w: rect.width, h: rect.height })
        .catch(() => {});
      // 矩形从「没有」恢复（最小化回来、面板从一条缝重新展开）时，ensure effect
      // 不会重跑（它的 deps 不含矩形），webview 可能还停在 hide 状态，用户会永远
      // 看着占位提示——这里顺手亮回来。放进同一个 key 判重里：矩形没变就不发，
      // 拖侧栏不会每帧都调；canShow 为 false 时绝不亮（浮层/阅读模式/zen 的让路
      // 不受影响），webview 还没建时后端 set_visible 是无害 no-op。
      if (canShowRef.current) {
        void api.viewerWebviewSetVisible(tab.id, true).catch(() => {});
      }
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(sync);
    };
    const ro = new ResizeObserver(schedule);
    ro.observe(host);
    window.addEventListener("resize", schedule);
    schedule(); // 创建时传过去的 rect 可能已经旧了，挂载先对齐一次
    return () => {
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener("resize", schedule);
    };
    // readerMode 必须在 deps 里：阅读模式会卸载宿主 div，切回来时是**新建**的节点，
    // RO 得跟着重挂，否则之后拖侧栏/缩窗就没人再同步 bounds 了
  }, [useNative, tab.id, readerMode]);

  // 卸载 / 切走时藏起来（不销毁：收起再展开页面还在原地）
  useEffect(() => {
    if (!useNative) return;
    return () => {
      setNativeWebviewUp(false);
      void api.viewerWebviewSetVisible(tab.id, false).catch(() => {});
    };
  }, [useNative, tab.id, setNativeWebviewUp]);

  // agent 让重载（viewer_reload → reloadSeq +1）：原生层里刷新页面。
  // deps 不带 tab.url：网页里点链接也会经 on_navigation 更新 tab.url，
  // 带上它就会在 reloadSeq≥1 之后每次导航把刚打开的页面整页闪刷一遍。
  useEffect(() => {
    if (!useNative || reloadSeq === 0) return;
    void api.viewerWebviewReload(tab.id).catch(() => {});
  }, [useNative, tab.id, reloadSeq]);

  // iframe 兜底路径才探测响应头：原生视图是顶层浏览上下文，不受那些头约束。
  // 站点拒绝被嵌时 Chromium 只会画一张「拒绝了我们的连接请求」，不如直接切阅读模式。
  useEffect(() => {
    if (!nativeFailed || isLocal || !tab.url) return;
    let cancelled = false;
    setFrameBlock(null);
    api
      .webFrameCheck(tab.url)
      .then((r) => {
        if (cancelled || r.embeddable) return;
        setFrameBlock(r.reason);
        setReaderMode(true);
      })
      .catch(() => {
        /* 检查失败不拦着用户：照常渲染，让他自己看结果 */
      });
    return () => {
      cancelled = true;
    };
  }, [tab.url, tab.id, isLocal, nativeFailed, reloadSeq, setReaderMode]);

  // 本地 HTML：读出源码用 sandbox iframe 渲染
  useEffect(() => {
    if (!isLocal) return;
    let cancelled = false;
    api
      .viewerLoadText(tab.id)
      .then((t) => !cancelled && setLocalHtml(t))
      .catch(() => !cancelled && setLocalHtml(null));
    return () => {
      cancelled = true;
    };
  }, [isLocal, tab.id, reloadSeq]);

  // 阅读模式：取 agent 视角的正文
  useEffect(() => {
    if (!readerMode || isLocal) return;
    let cancelled = false;
    setReader({ text: null, err: null, loading: true });
    api
      .viewerGetContent(tab.id)
      .then((t) => !cancelled && setReader({ text: t, err: null, loading: false }))
      .catch((e) => !cancelled && setReader({ text: null, err: errText(e), loading: false }));
    return () => {
      cancelled = true;
    };
  }, [readerMode, tab.id, isLocal, reloadSeq]);

  if (isLocal) {
    if (localHtml === null)
      return (
        <div className="empty" style={{ height: "100%" }}>
          <Spinner />
        </div>
      );
    return (
      <iframe
        className="viewer-frame"
        title={tab.title}
        // 本地 HTML 只渲染展示，不允许脚本，避免它去调 IPC
        sandbox=""
        srcDoc={localHtml}
      />
    );
  }

  // 阅读模式（手动按钮，或兜底路径下被 frame_block 自动切过来）：两条路径共用
  if (readerMode) {
    if (reader.loading)
      return (
        <div className="empty" style={{ height: "100%" }}>
          <Spinner />
          <div>正在提取正文…</div>
        </div>
      );
    if (reader.err) return <div className="viewer-note">{reader.err}</div>;
    return (
      <div className="viewer-scroll">
        {frameBlock && (
          <div className="viewer-note" style={{ margin: "8px 12px 0" }}>
            {frameBlock}，所以不允许被嵌进这里——已切到阅读模式，显示的是服务端提取的正文
            （agent 读到的也是它）。
            <button className="btn sm" style={{ marginLeft: 8 }} onClick={() => tab.url && void openExternal(tab.url)}>
              用系统浏览器看原页面
            </button>
          </div>
        )}
        <div className="reader">
          <Markdown source={reader.text ?? ""} onLink={(h) => void useApp.getState().openUrl(h)} />
        </div>
      </div>
    );
  }

  // 默认：原生子 WebView。页面画在 DOM 之上的原生层里，这个 div 只是它的「影子」
  if (useNative) {
    return (
      <div className="viewer-webview" ref={hostRef}>
        <span className="viewer-webview-hint">原生网页视图</span>
      </div>
    );
  }

  // iframe 兜底：原生视图创建失败才走到这里（老行为，包括 X-Frame-Options 的说明）
  return (
    <>
      <iframe
        className="viewer-frame"
        title={tab.title}
        src={tab.url ?? undefined}
        sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox"
        referrerPolicy="no-referrer"
      />
      <div className="viewer-note" style={{ position: "absolute", bottom: 8, left: 8, right: 8, margin: 0 }}>
        原生视图不可用，已退回内嵌渲染。如果这里显示「拒绝了我们的连接请求」，那是站点自己发的
        <code className="mono">X-Frame-Options</code> / <code className="mono">CSP frame-ancestors</code>
        在拒绝被嵌入（GitHub、知乎、Google 都这样），不是网络问题。点工具栏的「阅读模式」看正文，
        或用系统浏览器打开原页面。
      </div>
    </>
  );
}

// ---------------------------------------------------------------- 新标签页

function NewTabPage() {
  const [url, setUrl] = useState("");
  const config = useApp((s) => s.config);

  return (
    <div style={{ padding: 20, maxWidth: 460, margin: "0 auto" }}>
      <div style={{ fontSize: 14, marginBottom: 12, color: "var(--text-sub)" }}>新标签页</div>
      <div className="row">
        <input
          className="input"
          autoFocus
          placeholder="输入网址，回车打开"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && url.trim()) void useApp.getState().openTab({ url: url.trim() });
          }}
        />
        <button
          className="btn primary"
          onClick={() => url.trim() && void useApp.getState().openTab({ url: url.trim() })}
        >
          打开
        </button>
      </div>

      <div className="col" style={{ marginTop: 18, gap: 6 }}>
        <div className="muted" style={{ fontSize: 12 }}>
          常用
        </div>
        {[
          ["arXiv", "https://arxiv.org"],
          ["Wikipedia", "https://zh.wikipedia.org"],
          ["MDN", "https://developer.mozilla.org/zh-CN"],
          ["Bing", "https://www.bing.com"],
        ].map(([name, href]) => (
          <button key={href} className="btn ghost" style={{ justifyContent: "flex-start" }} onClick={() => void useApp.getState().openUrl(href)}>
            <Icon name="globe" size={13} /> {name}
          </button>
        ))}
        {config?.viewer.homeUrl && (
          <button
            className="btn ghost"
            style={{ justifyContent: "flex-start" }}
            onClick={() => void useApp.getState().openUrl(config.viewer.homeUrl)}
          >
            <Icon name="hub" size={13} /> 主页（{config.viewer.homeUrl}）
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 文本高亮

/** 把容器里所有匹配的文字包进 <mark>（只动文本节点，不破坏结构） */
function highlightText(root: HTMLElement, query: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 0;
  let count = 0;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (!node.nodeValue || !node.nodeValue.toLowerCase().includes(q)) return NodeFilter.FILTER_REJECT;
      const parent = (node as Text).parentElement;
      if (!parent) return NodeFilter.FILTER_REJECT;
      const tag = parent.tagName.toLowerCase();
      if (tag === "script" || tag === "style" || tag === "mark") return NodeFilter.FILTER_REJECT;
      return NodeFilter.FILTER_ACCEPT;
    },
  });

  const targets: Text[] = [];
  let n: Node | null;
  while ((n = walker.nextNode())) targets.push(n as Text);

  for (const node of targets) {
    const value = node.nodeValue ?? "";
    const lower = value.toLowerCase();
    const frag = document.createDocumentFragment();
    let i = 0;
    let hit = false;
    while (true) {
      const idx = lower.indexOf(q, i);
      if (idx === -1) break;
      if (idx > i) frag.appendChild(document.createTextNode(value.slice(i, idx)));
      const mark = document.createElement("mark");
      mark.className = "hit";
      mark.textContent = value.slice(idx, idx + q.length);
      frag.appendChild(mark);
      i = idx + q.length;
      hit = true;
      count++;
    }
    if (hit) {
      if (i < value.length) frag.appendChild(document.createTextNode(value.slice(i)));
      node.parentNode?.replaceChild(frag, node);
    }
  }
  return count;
}

/** 供外部（如工具卡片）判断是否要用阅读模式 */
export function useActiveTab(): TabView | null {
  const viewer = useApp((s) => s.viewer);
  return useMemo(() => viewer.tabs.find((t) => t.id === viewer.activeId) ?? null, [viewer]);
}
