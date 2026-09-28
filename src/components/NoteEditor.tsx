// 内置 Markdown 笔记编辑器。
//
// 内核移植自 InkNote（CodeMirror 6 的 Typora 式所见即所得：公式、图表、表格、代码块
// 都在正文里原地渲染），这里负责把它接到学习中枢的文件体系上：
//   - 读写走 `api`（主题目录内的相对路径，越界由后端拒绝）
//   - 把 InkNote 的编辑器桥（确认框 / 选图 / 保存请求 / 提示）挂到本组件
//   - 主题跟随学习中枢（data-theme / data-md-theme）

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import Editor, { type EditorRef } from "../inknote/components/Editor";
import type { EditorMode } from "../inknote/editor";
import { setEditorDocumentContext } from "../inknote/lib/tauri";
import { setEditorBridge } from "../inknote/lib/editorBridge";
import { setConfirmHandler } from "../inknote/lib/confirmBridge";
import { getStoredValue, initializeSettingsStore, setStoredValue } from "../inknote/lib/settingsStore";
import { getLocale, setLocale } from "../inknote/lib/i18n";
import { applyEditorLayoutPrefs } from "../inknote/lib/preferences";
import { notifyToast, useToast } from "../inknote/lib/useToast";
import Toast from "../inknote/components/Toast";
import { api, errText } from "../lib/api";
import { Icon } from "./ui";

interface Props {
  /** 主题目录名 */
  slug: string;
  /** 主题目录绝对路径（编辑器的资源解析基准） */
  topicDir: string;
  /** 主题内相对路径，例如 notes/特征值.md */
  path: string;
  onSaved?: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

export function NoteEditor({ slug, topicDir, path, onSaved, onDirtyChange }: Props) {
  const editorRef = useRef<EditorRef>(null);
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [mode, setMode] = useState<EditorMode>(() => "preview");
  const toast = useToast();

  // 编辑器偏好（字号/行高/宽度）用 InkNote 那套键存在 localStorage
  const [prefs] = useState(() => ({
    typewriter: getStoredValue("mdnote.typewriter") === "1",
    lineNumbers: getStoredValue("mdnote.lineNumbers") === "1",
    wordWrap: getStoredValue("mdnote.wordWrap") !== "0",
    tabSize: Number(getStoredValue("mdnote.tabSize") ?? "4") || 4,
    spellCheck: getStoredValue("mdnote.spellCheck") === "1",
  }));

  const absolutePath = useMemo(
    () => `${topicDir.replace(/[\\/]+$/, "")}/${path}`.replace(/\//g, "\\"),
    [topicDir, path],
  );

  // 让编辑器内的所有文件操作都落在这个主题里
  useEffect(() => {
    setEditorDocumentContext({ slug, dir: topicDir });
    return () => setEditorDocumentContext(null);
  }, [slug, topicDir]);

  // 主题由 store 的 applyTheme 统一写 html[data-theme]，
  // 这里只保证 markdown 主题有个默认值（编辑器认 data-md-theme）
  useEffect(() => {
    if (!document.documentElement.dataset.mdTheme) {
      document.documentElement.dataset.mdTheme = "github";
    }
  }, []);

  useEffect(() => {
    void initializeSettingsStore().then(() => {
      if (!getStoredValue("inknote.locale")) setLocale("zh");
      applyEditorLayoutPrefs();
    });
  }, []);

  // ---- 载入 ----
  useEffect(() => {
    let cancelled = false;
    setContent(null);
    setError(null);
    api
      .noteGet(slug, path)
      .then((n) => {
        if (cancelled) return;
        setContent(n.content);
        setDirty(false);
        onDirtyChange?.(false);
      })
      .catch((e) => !cancelled && setError(errText(e)));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, path]);

  // ---- 保存 ----
  const save = useCallback(async (): Promise<string | null> => {
    if (content === null) return null;
    setSaving(true);
    try {
      await api.noteSave(slug, path, content);
      setDirty(false);
      onDirtyChange?.(false);
      onSaved?.();
      notifyToast("已保存", "success");
      return path;
    } catch (e) {
      notifyToast(errText(e), "error");
      return null;
    } finally {
      setSaving(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [content, slug, path]);

  // Ctrl/Cmd+S：InkNote 的编辑器本身不接管保存，由宿主窗口处理
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        if (dirty) void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dirty, save]);

  // ---- 编辑器桥 ----
  useEffect(() => {
    setConfirmHandler(async (message) => window.confirm(message));
    setEditorBridge({
      confirm: async (message) => window.confirm(message),
      prompt: async (req) => window.prompt(req.label, req.defaultValue ?? ""),
      pickLink: async (defaultText) => {
        const url = window.prompt("链接地址", "https://");
        if (!url) return null;
        return { text: defaultText || url, url };
      },
      pickImage: async (defaultAlt) => {
        const picked = await openDialog({
          multiple: false,
          title: "选择图片（会复制到笔记旁边的 .inknote-assets/）",
          filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"] }],
        });
        if (typeof picked !== "string") return null;
        return { alt: defaultAlt, path: picked };
      },
      requestSave: save,
      requestSearch: (replace) => {
        editorRef.current?.runAction(replace ? "findReplace" : "find");
      },
      showError: (e) => notifyToast(errText(e), "error"),
      showMessage: (m) => notifyToast(m, "info"),
    });
    return () => setEditorBridge(null);
  }, [save]);

  if (error) {
    return (
      <div className="wb-pane">
        <div className="viewer-note">打不开这个文件：{error}</div>
      </div>
    );
  }

  if (content === null) {
    return (
      <div className="empty" style={{ height: "50%" }}>
        <span className="spinner" />
        <div>正在打开…</div>
      </div>
    );
  }

  return (
    <div className="inknote-scope" style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div className="row" style={{ padding: "6px 12px", gap: 8, borderBottom: "1px solid var(--border)" }}>
        <span className="mono" style={{ fontSize: 11.5, color: "var(--text-faint)" }}>
          {path}
        </span>
        {dirty && <span className="tag accent">未保存</span>}
        {saving && <span className="spinner" />}
        <div className="grow" />
        <button
          className="icon-btn"
          title={mode === "preview" ? "切到源码模式（Ctrl+/）" : "切到所见即所得（Ctrl+/）"}
          onClick={() => setMode((m) => (m === "preview" ? "source" : "preview"))}
        >
          <Icon name={mode === "preview" ? "pencil" : "eye"} />
        </button>
        <button className="btn sm" disabled={!dirty} onClick={() => void save()} title="保存（Ctrl+S）">
          保存
        </button>
      </div>

      <div style={{ flex: 1, minHeight: 0, display: "flex" }}>
        <Editor
          key={path}
          ref={editorRef}
          documentId={path}
          active
          locale={getLocale()}
          value={content}
          mode={mode}
          filePath={absolutePath}
          typewriter={prefs.typewriter}
          lineNumbers={prefs.lineNumbers}
          wordWrap={prefs.wordWrap}
          tabSize={prefs.tabSize}
          spellCheck={prefs.spellCheck}
          readOnly={false}
          onChange={(doc) => {
            setContent(doc);
            if (!dirty) {
              setDirty(true);
              onDirtyChange?.(true);
            }
          }}
          onModeChange={setMode}
        />
      </div>

      <Toast message={toast.message} kind={toast.toastKind} />
    </div>
  );
}

// 编辑器偏好写到 localStorage（供将来的设置面板调用）
export function setEditorPreference(key: string, value: string): void {
  setStoredValue(`mdnote.${key}`, value);
}
