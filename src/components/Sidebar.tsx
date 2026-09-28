// 左侧栏：新建主题 / 搜索主题（上），主题列表（下），用户与设置（最底）。

import { useEffect, useMemo, useState } from "react";
import { useApp } from "../store/app";
import { STAGE_LABEL, THEME_LABEL, type StudyStage } from "../lib/types";
import { hotkey, relTime } from "../lib/format";
import { Dropdown, Empty, Field, Icon, MenuItem, MenuLabel, MenuSep, Modal } from "./ui";
import { McpDialog, SkillsDialog } from "./Extend";
import { api } from "../lib/api";

export function Sidebar() {
  const topics = useApp((s) => s.topics);
  const activeSlug = useApp((s) => s.topic?.slug ?? null);
  const view = useApp((s) => s.view);
  const config = useApp((s) => s.config);
  const openTopic = useApp((s) => s.openTopic);
  const setView = useApp((s) => s.setView);
  const setPaletteOpen = useApp((s) => s.setPaletteOpen);
  const toast = useApp((s) => s.toast);

  const [newOpen, setNewOpen] = useState(false);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [skillCount, setSkillCount] = useState(0);
  const [mcpCount, setMcpCount] = useState(0);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [creating, setCreating] = useState(false);

  const sorted = useMemo(
    () =>
      [...topics].sort((a, b) => {
        const ka = a.meta.lastOpenedAt ?? a.meta.updatedAt;
        const kb = b.meta.lastOpenedAt ?? b.meta.updatedAt;
        return ka < kb ? 1 : -1;
      }),
    [topics],
  );

  const totalDue = sorted.reduce((n, t) => n + t.stats.cardsDue, 0);

  // 角标：当前生效的技能数 / 已连接的 MCP 服务器数
  useEffect(() => {
    const load = async () => {
      try {
        const ov = await api.skillsOverview(activeSlug);
        setSkillCount(ov.global.filter((x) => x.enabled).length + ov.topic.filter((x) => x.enabled).length);
      } catch {
        /* 忽略 */
      }
      try {
        const mcp = await api.mcpOverview(activeSlug);
        setMcpCount([...mcp.global, ...mcp.topic].filter((x) => x.connected && x.enabledHere).length);
      } catch {
        /* 忽略 */
      }
    };
    void load();
  }, [activeSlug, topics]);

  async function submitNew() {
    if (!name.trim() || creating) return;
    setCreating(true);
    const created = await useApp.getState().createTopic(name.trim(), desc.trim());
    setCreating(false);
    if (created) {
      setNewOpen(false);
      setName("");
      setDesc("");
      await openTopic(created.slug);
    }
  }

  return (
    <aside className="sidebar">
      <div className="sidebar-top">
        <button className="side-item primary" onClick={() => setNewOpen(true)}>
          <Icon name="plus" />
          <span className="grow">新建主题</span>
        </button>
        <button className="side-item" onClick={() => setPaletteOpen(true)}>
          <Icon name="search" />
          <span className="grow">搜索主题</span>
          <span className="side-kbd">{hotkey("K")}</span>
        </button>

        {/* 技能与 MCP：放在最上面，和「新建/搜索主题」同级 */}
        <button className="side-item" onClick={() => setSkillsOpen(true)}>
          <Icon name="puzzle" />
          <span className="grow">技能</span>
          {skillCount > 0 && <span className="side-kbd">{skillCount}</span>}
        </button>
        <button className="side-item" onClick={() => setMcpOpen(true)}>
          <Icon name="plug" />
          <span className="grow">MCP 服务器</span>
          {mcpCount > 0 && <span className="side-kbd">{mcpCount}</span>}
        </button>

        <div className="menu-sep" style={{ margin: "6px 8px" }} />

        <button
          className="side-item"
          style={view === "home" ? { background: "var(--bg-active)", color: "var(--text)" } : undefined}
          onClick={() => {
            useApp.getState().leaveTopic();
            setView("home");
          }}
        >
          <Icon name="hub" />
          <span className="grow">首页</span>
        </button>
        <button
          className="side-item"
          style={view === "agenda" ? { background: "var(--bg-active)", color: "var(--text)" } : undefined}
          onClick={() => setView("agenda")}
        >
          <Icon name="calendar" />
          <span className="grow">日程</span>
          {totalDue > 0 && <span className="side-kbd">{totalDue}</span>}
        </button>
      </div>

      <div className="side-section">
        <span>主题</span>
        <span className="count">{sorted.length}</span>
      </div>

      <div className="topic-list">
        {sorted.length === 0 ? (
          <div style={{ padding: "10px 12px", color: "var(--text-faint)", fontSize: 12.5, lineHeight: 1.7 }}>
            还没有主题。
            <br />
            点上面的「新建主题」，或者直接跟 agent 说你想学什么。
          </div>
        ) : (
          sorted.map((t) => (
            <TopicRow
              key={t.slug}
              slug={t.slug}
              name={t.meta.name}
              emoji={t.meta.emoji ?? null}
              stage={t.meta.stage}
              due={t.stats.cardsDue}
              cards={t.stats.cards}
              notes={t.stats.notes}
              tasks={t.stats.tasksOpen}
              active={t.slug === activeSlug}
              onClick={() => openTopic(t.slug)}
            />
          ))
        )}
      </div>

      <div className="sidebar-foot">
        <div className="avatar">{(config?.userName ?? "学").slice(0, 1)}</div>
        <div className="name">{config?.userName ?? "旅行者"}</div>
        <button
          className="icon-btn"
          title="内置浏览器"
          onClick={() => useApp.getState().toggleViewer()}
        >
          <Icon name="panel" />
        </button>
        <button
          className="icon-btn"
          title="工作区目录"
          onClick={async () => {
            try {
              const p = await api.revealInExplorer(activeSlug);
              toast("info", `已在文件管理器中打开：${p}`);
            } catch (e) {
              toast("error", String(e));
            }
          }}
        >
          <Icon name="folder" />
        </button>
        <ThemeButton />
        <button
          className={"icon-btn" + (view === "settings" ? " active" : "")}
          title="设置"
          onClick={() => setView("settings")}
        >
          <Icon name="settings" />
        </button>
      </div>

      {skillsOpen && <SkillsDialog onClose={() => setSkillsOpen(false)} />}
      {mcpOpen && <McpDialog onClose={() => setMcpOpen(false)} />}

      {newOpen && (
        <Modal
          title="新建主题"
          icon="plus"
          onClose={() => setNewOpen(false)}
          footer={
            <>
              <span className="left muted" style={{ fontSize: 11.5 }}>
                会在工作区里创建一个同名目录
              </span>
              <button className="btn" onClick={() => setNewOpen(false)}>
                取消
              </button>
              <button className="btn primary" onClick={submitNew} disabled={!name.trim() || creating}>
                {creating ? "创建中…" : "创建"}
              </button>
            </>
          }
        >
          <Field label="主题名" hint="会成为目录名，建议用「学科/主题」的形式，例如「线性代数」「React 源码」">
            <input
              className="input"
              autoFocus
              value={name}
              placeholder="想学什么？"
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && submitNew()}
            />
          </Field>
          <Field label="一句话说明" hint="写清「学它做什么」，agent 每次都会看到这句">
            <input
              className="input"
              value={desc}
              placeholder="例如：为了看懂论文里的矩阵分解"
              onChange={(e) => setDesc(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && submitNew()}
            />
          </Field>
        </Modal>
      )}
    </aside>
  );
}

/** 主题模式循环切换：跟随系统 → 明亮 → 深色 */
function ThemeButton() {
  const theme = useApp((s) => s.config?.appearance?.theme ?? "system");
  const setTheme = useApp((s) => s.setTheme);
  const next: Record<string, "system" | "light" | "dark"> = {
    system: "light",
    light: "dark",
    dark: "system",
  };
  const icon = theme === "light" ? "sun" : theme === "dark" ? "moon" : "auto";
  return (
    <button
      className="icon-btn"
      title={`主题：${THEME_LABEL[theme]}（点击切换）`}
      onClick={() => void setTheme(next[theme])}
    >
      <Icon name={icon} size={14} />
    </button>
  );
}

function TopicRow({
  slug,
  name,
  emoji,
  stage,
  due,
  cards,
  notes,
  tasks,
  active,
  onClick,
}: {
  slug: string;
  name: string;
  emoji: string | null;
  stage: StudyStage;
  due: number;
  cards: number;
  notes: number;
  tasks: number;
  active: boolean;
  onClick: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const updateTopic = useApp((s) => s.updateTopic);
  const deleteTopic = useApp((s) => s.deleteTopic);

  return (
    <>
      <div className={"topic-row" + (active ? " active" : "")} onClick={onClick} title={name}>
        <span style={{ flex: "none", fontSize: 13 }}>{emoji ?? "📘"}</span>
        <span className="topic-name">{name}</span>

        <span className="topic-badges">
          {due > 0 && (
            <span className="badge-dot badge-due" title={`${due} 张卡片待复习`}>
              <Icon name="target" size={11} />
              {due}
            </span>
          )}
          {tasks > 0 && (
            <span className="badge-dot" title={`${tasks} 个未完成任务`}>
              <Icon name="check" size={11} />
              {tasks}
            </span>
          )}
        </span>

        <span className="row-actions" onClick={(e) => e.stopPropagation()}>
          <Dropdown
            trigger={() => (
              <button className="icon-btn" title="更多">
                <Icon name="more" size={14} />
              </button>
            )}
          >
            {(close) => (
              <>
                <MenuLabel>{STAGE_LABEL[stage]}阶段 · {notes} 笔记 / {cards} 卡片</MenuLabel>
                <MenuSep />
                {(Object.keys(STAGE_LABEL) as StudyStage[]).map((s) => (
                  <MenuItem
                    key={s}
                    selected={s === stage}
                    onClick={() => {
                      void updateTopic({ stage: s });
                      close();
                    }}
                  >
                    切到{STAGE_LABEL[s]}阶段
                  </MenuItem>
                ))}
                <MenuSep />
                <MenuItem
                  onClick={() => {
                    void api
                      .revealInExplorer(slug)
                      .catch((e) => useApp.getState().toast("error", String(e)));
                    close();
                  }}
                >
                  <Icon name="folder" size={13} /> 打开目录
                </MenuItem>
                <MenuItem
                  onClick={async () => {
                    const { open } = await import("@tauri-apps/plugin-dialog");
                    const picked = await open({
                      multiple: true,
                      title: "选择资料（会复制到该主题的 materials/）",
                    });
                    const paths = picked ? (Array.isArray(picked) ? picked : [picked]) : [];
                    if (paths.length === 0) return;
                    await useApp.getState().openTopic(slug);
                    await useApp.getState().importMaterials(paths as string[]);
                    close();
                  }}
                >
                  <Icon name="download" size={13} /> 导入资料…
                </MenuItem>
                <MenuItem onClick={() => void useApp.getState().startSession(`学习 ${name}`)}>
                  <Icon name="play" size={13} /> 开始学习会话
                </MenuItem>
                <MenuSep />
                <MenuItem
                  danger
                  onClick={() => {
                    setConfirming(true);
                    close();
                  }}
                >
                  <Icon name="trash" size={13} /> 删除主题
                </MenuItem>
              </>
            )}
          </Dropdown>
        </span>
      </div>

      {confirming && (
        <Modal
          title={`删除主题「${name}」？`}
          icon="alert"
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirming(false)}>
                取消
              </button>
              <button
                className="btn danger"
                onClick={() => {
                  void deleteTopic(slug);
                  setConfirming(false);
                }}
              >
                移入回收站
              </button>
            </>
          }
        >
          <div className="sub">
            不会真删。整个目录会被移动到 <code className="mono">工作区/.hub/trash/</code>，
            需要时手动搬回来即可。
          </div>
        </Modal>
      )}
    </>
  );
}

/** 搜索主题的命令面板 */
export function SearchPalette() {
  const open = useApp((s) => s.paletteOpen);
  const setOpen = useApp((s) => s.setPaletteOpen);
  const openTopic = useApp((s) => s.openTopic);
  const topics = useApp((s) => s.topics);
  const [q, setQ] = useState("");
  const [cursor, setCursor] = useState(0);
  const [matches, setMatches] = useState<{ slug: string; name: string; hits: string[] }[]>([]);

  // Esc 在任何位置都能关（不只在输入框里）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);

  useMemo(() => {
    if (!open) return;
    let alive = true;
    const run = async () => {
      try {
        const res = await api.topicSearch(q, 20);
        if (alive) {
          setMatches(
            res.map((m) => ({
              slug: m.summary.slug,
              name: m.summary.meta.name,
              hits: m.hits.slice(0, 2),
            })),
          );
          setCursor(0);
        }
      } catch {
        if (alive) setMatches([]);
      }
    };
    void run();
    return () => {
      alive = false;
    };
  }, [q, open]);

  if (!open) return null;

  const list = matches.length > 0
    ? matches
    : topics.map((t) => ({ slug: t.slug, name: t.meta.name, hits: [relTime(t.meta.lastOpenedAt ?? t.meta.updatedAt)] }));

  const choose = (i: number) => {
    const item = list[i];
    if (!item) return;
    setOpen(false);
    setQ("");
    void openTopic(item.slug);
  };

  return (
    <div
      className="overlay top"
      onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}
    >
      <div className="palette">
        <input
          className="palette-input"
          autoFocus
          placeholder="搜索主题名、标签或笔记正文…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setCursor((c) => Math.min(c + 1, list.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setCursor((c) => Math.max(c - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(cursor);
            } else if (e.key === "Escape") {
              setOpen(false);
            }
          }}
        />
        <div className="palette-list">
          {list.length === 0 ? (
            <Empty icon="search">没有匹配的主题</Empty>
          ) : (
            list.map((t, i) => (
              <div
                key={t.slug}
                className={"palette-item" + (i === cursor ? " on" : "")}
                onMouseEnter={() => setCursor(i)}
                onClick={() => choose(i)}
              >
                <Icon name="book" size={14} />
                <span className="p-title">{t.name}</span>
                <span className="p-hit grow">{t.hits.join("｜")}</span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
