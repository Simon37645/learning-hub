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
  /** 站点禁止内嵌时的原因；非空说明我们已经自动切到阅读模式了 */
  const [frameBlock, setFrameBlock] = useState<string | null>(null);
  const isLocal = !!tab.path;
  const reloadSeq = useApp((s) => s.reloadSeq[tab.id] ?? 0);

  // 打开网页前先看一眼响应头：站点用 X-Frame-Options / CSP frame-ancestors 拒绝被嵌时，
  // Chromium 只会把内嵌窗口画成一张「拒绝了我们的连接请求」的错误页（看着像断网）。
  // 与其让用户看那张图，不如直接切阅读模式——正文来自服务端提取，agent 读到的也是它。
  useEffect(() => {
    if (isLocal || !tab.url) return;
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
  }, [tab.url, tab.id, isLocal, reloadSeq, setReaderMode]);

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
        如果这里显示「拒绝了我们的连接请求」，那是站点自己发的
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
