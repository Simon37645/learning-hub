// 左侧栏：新建主题 / 搜索主题（上），主题树（下），用户与设置（最底）。
//
// 主题可以是「整门课」，也可以是它下面的「一章」（父主题里记 id）。
// 一章仍占工作区根下的一个目录，只是侧栏里缩进显示——这样学一章节时
// 既能收窄上下文，又不用把讲义重新导入一遍。

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useApp } from "../store/app";
import { STAGE_LABEL, THEME_LABEL, type StudyStage, type TopicSummary } from "../lib/types";
import { hotkey, relTime } from "../lib/format";
import { Dropdown, Empty, Field, Icon, MenuItem, MenuLabel, MenuSep, Modal } from "./ui";
import { McpDialog, SkillsDialog } from "./Extend";
import { MemoryPanel } from "./Memory";
import { api } from "../lib/api";

/** 折叠状态存本地：章节多了以后默认全展开太挤 */
const TREE_KEY = "hub.sidebar.collapsed";

interface TopicTreeNode {
  topic: TopicSummary;
  children: TopicTreeNode[];
}

/** 平铺列表拼成树。parent 存的是父主题 id；父主题不在了（被删或移出工作区）就当顶层，不让它消失。 */
function buildTopicTree(topics: TopicSummary[]): TopicTreeNode[] {
  const nodes = new Map<string, TopicTreeNode>();
  for (const t of topics) nodes.set(t.meta.id, { topic: t, children: [] });
  const roots: TopicTreeNode[] = [];
  for (const t of topics) {
    const node = nodes.get(t.meta.id)!;
    const parent = t.meta.parent ? nodes.get(t.meta.parent) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  return roots;
}

function loadCollapsed(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(TREE_KEY) ?? "[]") as unknown;
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function Sidebar() {
  const topics = useApp((s) => s.topics);
  const activeSlug = useApp((s) => s.topic?.slug ?? null);
  const topic = useApp((s) => s.topic);
  const activeChatId = useApp((s) => s.chatId);
  const chatIndex = useApp((s) => s.chatIndex);
  const loadChat = useApp((s) => s.loadChat);
  const loadChats = useApp((s) => s.loadChats);
  const view = useApp((s) => s.view);
  const config = useApp((s) => s.config);
  const openTopic = useApp((s) => s.openTopic);
  const setView = useApp((s) => s.setView);
  const setPaletteOpen = useApp((s) => s.setPaletteOpen);
  const toast = useApp((s) => s.toast);
  /** 记忆变化时 +1：角标上的条数要跟着刷新 */
  const memoryTick = useApp((s) => s.memoryTick);

  const [newOpen, setNewOpen] = useState(false);
  const [newParent, setNewParent] = useState<{ slug: string; name: string } | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);
  const [skillCount, setSkillCount] = useState(0);
  const [mcpCount, setMcpCount] = useState(0);
  const [memoryCount, setMemoryCount] = useState(0);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [creating, setCreating] = useState(false);
  const [collapsed, setCollapsed] = useState<string[]>(loadCollapsed);

  const sorted = useMemo(
    () =>
      [...topics].sort((a, b) => {
        const ka = a.meta.lastOpenedAt ?? a.meta.updatedAt;
        const kb = b.meta.lastOpenedAt ?? b.meta.updatedAt;
        return ka < kb ? 1 : -1;
      }),
    [topics],
  );
  const tree = useMemo(() => buildTopicTree(sorted), [sorted]);

  const toggleCollapse = (id: string) => {
    setCollapsed((prev) => {
      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id];
      try {
        localStorage.setItem(TREE_KEY, JSON.stringify(next));
      } catch {
        /* 隐私模式下写不了，不影响使用 */
      }
      return next;
    });
  };

  const totalDue = sorted.reduce((n, t) => n + t.stats.cardsDue, 0);

  // 展开了、确实有对话、还没拉过标题的主题：按需读一次。
  // 标题要读 jsonl 才知道，所以只在真的展开时才读，别一上来把所有主题都扫一遍。
  useEffect(() => {
    for (const t of topics) {
      if (collapsed.includes(t.meta.id)) continue;
      if ((t.stats.chats ?? 0) === 0) continue;
      if (chatIndex[t.slug]) continue;
      void loadChats(t.slug);
    }
  }, [topics, collapsed, chatIndex, loadChats]);

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
      try {
        // 角标显示「当前上下文真正会注入」的条数（与提示词一致），不是磁盘上的总条数
        const mem = await api.memoryOverview(activeSlug);
        setMemoryCount(mem.activeCount);
      } catch {
        /* 忽略 */
      }
    };
    void load();
  }, [activeSlug, topics, memoryTick]);

  const submitNew = async () => {
    if (!name.trim() || creating) return;
    setCreating(true);
    const created = await useApp.getState().createTopic(name.trim(), desc.trim(), newParent?.slug ?? null);
    setCreating(false);
    if (created) {
      setNewOpen(false);
      setName("");
      setDesc("");
      setNewParent(null);
      await openTopic(created.slug);
    }
  };

  const openNew = (parent?: { slug: string; name: string }) => {
    setNewParent(parent ?? null);
    setNewOpen(true);
  };

  /** 递归渲染主题树；子主题缩进一级，父主题可折叠 */
  const renderNode = (node: TopicTreeNode, depth: number) => {
    const t = node.topic;
    const isCollapsed = collapsed.includes(t.meta.id);
    const all = chatIndex[t.slug] ?? [];
    // 后端已按「置顶 → 最近使用 → 归档沉底」排好，这里只把归档的分出来
    const chats = all.filter((c) => !c.archived);
    const archived = all.filter((c) => c.archived);
    const onDisk = (t.stats.chats ?? 0) > 0;
    // 刚点「新对话」还没发第一条消息时，磁盘上还没有这个文件——先占一行
    const pendingNew = topic?.slug === t.slug && !!activeChatId && !chats.some((c) => c.id === activeChatId);
    // 只有归档对话的主题也要能展开，否则那些对话就永远看不见了
    const expandable = node.children.length > 0 || onDisk || pendingNew || archived.length > 0;
    const chatCount = all.length || t.stats.chats || 0;
    const open = !isCollapsed;

    const openChat = (chatId: string) => {
      void (async () => {
        if (topic?.slug !== t.slug) await openTopic(t.slug);
        await loadChat(chatId);
      })();
    };

    return (
      <Fragment key={t.slug}>
        <TopicRow
          slug={t.slug}
          name={t.meta.name}
          emoji={t.meta.emoji ?? null}
          stage={t.meta.stage}
          due={t.stats.cardsDue}
          cards={t.stats.cards}
          notes={t.stats.notes}
          tasks={t.stats.tasksOpen}
          active={t.slug === activeSlug && !pendingNew}
          depth={depth}
          childCount={node.children.length}
          chatCount={chatCount}
          expandable={expandable}
          collapsed={isCollapsed}
          parentSlug={t.meta.parent ?? null}
          onToggle={() => toggleCollapse(t.meta.id)}
          onNewSubtopic={() => openNew({ slug: t.slug, name: t.meta.name })}
          onClick={() => openTopic(t.slug)}
        />
        {open && (
          <>
            {node.children.map((c) => renderNode(c, depth + 1))}
            {chats.map((c) => (
              <ChatRow
                key={c.id}
                chatId={c.id}
                topicSlug={t.slug}
                title={c.title}
                time={c.updatedAt}
                depth={depth + 1}
                active={c.id === activeChatId}
                pinned={c.pinned}
                customTitle={c.customTitle}
                onClick={() => openChat(c.id)}
              />
            ))}
            {pendingNew && (
              <ChatRow topicSlug={t.slug} title="新对话" time={null} depth={depth + 1} active onClick={() => {}} />
            )}
            {archived.length > 0 && (
              <div className="chat-archived">
                <div className="chat-archived-head">
                  <Icon name="box" size={11} />
                  已归档 {archived.length}
                </div>
                {archived.map((c) => (
                  <ChatRow
                    key={c.id}
                    chatId={c.id}
                    topicSlug={t.slug}
                    title={c.title}
                    time={c.updatedAt}
                    depth={depth + 1}
                    active={c.id === activeChatId}
                    archived
                    customTitle={c.customTitle}
                    onClick={() => openChat(c.id)}
                  />
                ))}
              </div>
            )}
          </>
        )}
      </Fragment>
    );
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-top">
        <button className="side-item primary" onClick={() => openNew()}>
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
        <button className="side-item" onClick={() => setMemoryOpen(true)}>
          <Icon name="sparkle" />
          <span className="grow">记忆</span>
          {memoryCount > 0 && <span className="side-kbd">{memoryCount}</span>}
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
          tree.map((n) => renderNode(n, 0))
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
      {memoryOpen && <MemoryPanel onClose={() => setMemoryOpen(false)} />}

      {newOpen && (
        <Modal
          title={newParent ? "新建子主题" : "新建主题"}
          icon="plus"
          onClose={() => {
            setNewOpen(false);
            setNewParent(null);
          }}
          footer={
            <>
              <span className="left muted" style={{ fontSize: 11.5 }}>
                {newParent ? "会读得到父主题的资料与讲义（只读）" : "会在工作区里创建一个同名目录"}
              </span>
              <button
                className="btn"
                onClick={() => {
                  setNewOpen(false);
                  setNewParent(null);
                }}
              >
                取消
              </button>
              <button className="btn primary" onClick={submitNew} disabled={!name.trim() || creating}>
                {creating ? "创建中…" : "创建"}
              </button>
            </>
          }
        >
          {newParent && (
            <div className="sub" style={{ marginBottom: 10 }}>
              上级主题：<b>{newParent.name}</b>
              <button className="btn sm ghost" style={{ marginLeft: 8 }} onClick={() => setNewParent(null)}>
                改成顶层主题
              </button>
              <div style={{ marginTop: 4 }}>
                章节主题适合「这门课我只学这几章」：资料沿用父主题的，笔记、卡片、计划各自独立。
              </div>
            </div>
          )}
          <Field
            label="主题名"
            hint={
              newParent
                ? "建议写清是哪一章，例如「第三章 特征值」「第 5 讲 递归」"
                : "会成为目录名，建议用「学科/主题」的形式，例如「线性代数」「React 源码」"
            }
          >
            <input
              className="input"
              autoFocus
              value={name}
              placeholder={newParent ? "这一章叫什么？" : "想学什么？"}
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

/**
 * 一条对话（挂在所属主题下面，和子主题同一级缩进）。
 *
 * 为什么放侧栏而不是对话区顶部：对话区顶部的下拉一打开就挡住内容，
 * 而且换主题时看不见「另一个主题里我聊到哪了」。挂进树里，归属一目了然。
 */
/**
 * 一条对话：点一下切换过去，**按住拖到别的主题行上**就把它搬过去
 * （父主题的对话拖进子主题，或从子主题拖回父主题）。
 *
 * 为什么自己写拖拽：Tauri 在 Windows 上默认接管文件拖放（资料导入要用它），
 * HTML5 的 dragstart/drop 根本不会触发，所以这里用指针事件自己做：
 * 指针移动超过 5px 才算拖拽（否则还是普通的点击），跟随一个幽灵小卡片，
 * 经过的主题行高亮，松手调用 chat_move。
 */
function ChatRow({
  chatId,
  topicSlug,
  title,
  time,
  depth,
  active,
  pinned,
  archived,
  customTitle,
  onClick,
}: {
  chatId?: string;
  topicSlug: string;
  title: string;
  time: string | null;
  depth: number;
  active: boolean;
  /** 置顶：排在该主题对话列表最前面 */
  pinned?: boolean;
  /** 归档：收在「已归档」分组里 */
  archived?: boolean;
  /** 用户自己改过名（菜单里才给「恢复自动标题」） */
  customTitle?: boolean;
  onClick: () => void;
}) {
  const moveChat = useApp((s) => s.moveChat);
  const renameChat = useApp((s) => s.renameChat);
  const pinChat = useApp((s) => s.pinChat);
  const archiveChat = useApp((s) => s.archiveChat);
  const forkChat = useApp((s) => s.forkChat);
  const deleteChat = useApp((s) => s.deleteChat);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const [confirming, setConfirming] = useState(false);
  const drag = useRef<{
    el: HTMLElement;
    startX: number;
    startY: number;
    moved: boolean;
    ghost: HTMLDivElement | null;
    target: string | null;
  } | null>(null);
  const suppressClick = useRef(false);

  const clearTarget = (row: HTMLElement | null) => row?.classList.remove("drop-target");
  const findRow = (slug: string) =>
    document.querySelector<HTMLElement>(`.topic-row[data-slug="${CSS.escape(slug)}"]`);

  /**
   * 行内的按钮/输入框不参与拖拽。
   *
   * 更要紧的是：拖拽必须**等真的开始拖了**再 `setPointerCapture`。在 pointerdown 上就捕获，
   * 会把随后的 pointerup/mouseup 重定向到这一行上——浏览器发现「按下」与「抬起」不在同一个
   * 元素上，就**不会**给按钮补发 click，于是行内的「⋯」永远点不开
   * （就是用户报的「主题的三个点有用、对话的三个点没用」；主题行没有指针拖拽所以正常）。
   */
  const innerInteractive = (t: EventTarget | null) =>
    !!(t instanceof Element && t.closest("button, input, textarea, select, a, [contenteditable=true]"));

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    // 正在改名时不拖：输入框里的拖选文字不能被当成「把对话搬走」
    if (!chatId || editing || e.button !== 0 || innerInteractive(e.target)) return;
    drag.current = {
      el: e.currentTarget,
      startX: e.clientX,
      startY: e.clientY,
      moved: false,
      ghost: null,
      target: null,
    };
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    if (!d.moved && Math.hypot(e.clientX - d.startX, e.clientY - d.startY) < 5) return;
    if (!d.moved) {
      d.moved = true;
      // 到这里才确认是拖拽：捕获指针，好让手滑出行外时后续事件仍然回到这一行
      try {
        d.el.setPointerCapture(e.pointerId);
      } catch {
        /* 合成事件（脚本触发）没有真实指针，捕获不到也无所谓：移动事件照样会冒泡上来 */
      }
      const ghost = document.createElement("div");
      ghost.className = "chat-drag-ghost";
      ghost.textContent = title || "新对话";
      document.body.appendChild(ghost);
      d.ghost = ghost;
      document.body.classList.add("dragging-chat");
    }
    if (d.ghost) {
      d.ghost.style.left = `${e.clientX + 12}px`;
      d.ghost.style.top = `${e.clientY + 12}px`;
    }
    const hit = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>(".topic-row");
    const slug = hit?.dataset.slug ?? null;
    if (slug !== d.target) {
      clearTarget(d.target ? findRow(d.target) : null);
      d.target = slug;
      if (slug && slug !== topicSlug) hit?.classList.add("drop-target");
    }
  };

  const onPointerUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    d.ghost?.remove();
    document.body.classList.remove("dragging-chat");
    clearTarget(d.target ? findRow(d.target) : null);
    if (d.moved) suppressClick.current = true; // 这一次不当作「打开」
    if (d.moved && d.target && d.target !== topicSlug && chatId) {
      void moveChat(chatId, topicSlug, d.target);
    }
  };

  const startRename = () => {
    setDraft(title);
    setEditing(true);
  };

  const commitRename = () => {
    if (!chatId) return;
    setEditing(false);
    const next = draft.trim();
    // 名字没动就不打扰后端（改名会写文件）
    if (next === title.trim()) return;
    void renameChat(chatId, next, topicSlug);
  };

  /** 从这条对话分叉：复制成新对话，原对话不动 */
  const doFork = () => {
    if (!chatId) return;
    void forkChat(chatId, topicSlug);
  };

  return (
    <>
      <div
        className={"chat-row" + (active ? " active" : "") + (archived ? " archived" : "")}
        style={{ paddingLeft: 9 + depth * 14 }}
        title={
          chatId
            ? `${title || "新对话"}（按住拖到别的主题可以搬过去）`
            : title
        }
        onClick={() => {
          if (editing) return;
          if (suppressClick.current) {
            suppressClick.current = false;
            return;
          }
          onClick();
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        {editing ? (
          <input
            className="chat-rename"
            autoFocus
            value={draft}
            placeholder="对话名字（留空＝用第一句话）"
            onChange={(e) => setDraft(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            onClick={(e) => e.stopPropagation()}
            onPointerDown={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitRename();
              if (e.key === "Escape") setEditing(false);
            }}
            onBlur={commitRename}
          />
        ) : (
          <>
            {pinned && (
              <Icon name="target" size={11} className="chat-pin" />
            )}
            <span className="chat-title">{title || "新对话"}</span>
            {time && <span className="chat-time">{relTime(time)}</span>}
            {/* 悬停才出现的「⋯」：重命名 / 置顶 / 分叉 / 归档 / 删除 */}
            {chatId && (
              <span className="row-actions chat-actions" onClick={(e) => e.stopPropagation()}>
                <Dropdown
                  trigger={() => (
                    <button className="icon-btn" title="更多">
                      <Icon name="more" size={14} />
                    </button>
                  )}
                >
                  {(close) => (
                    <>
                      <MenuLabel>{title || "新对话"}</MenuLabel>
                      <MenuSep />
                      <MenuItem
                        onClick={() => {
                          startRename();
                          close();
                        }}
                      >
                        <Icon name="pencil" size={13} /> 重命名
                      </MenuItem>
                      {customTitle && (
                        <MenuItem
                          onClick={() => {
                            void renameChat(chatId, "", topicSlug);
                            close();
                          }}
                        >
                          <Icon name="refresh" size={13} /> 恢复自动标题
                        </MenuItem>
                      )}
                      <MenuItem
                        onClick={() => {
                          void pinChat(chatId, !pinned, topicSlug);
                          close();
                        }}
                      >
                        <Icon name="target" size={13} /> {pinned ? "取消置顶" : "置顶"}
                      </MenuItem>
                      <MenuItem
                        onClick={() => {
                          doFork();
                          close();
                        }}
                      >
                        <Icon name="layers" size={13} /> 分叉出新对话
                      </MenuItem>
                      <MenuItem
                        onClick={() => {
                          void archiveChat(chatId, !archived, topicSlug);
                          close();
                        }}
                      >
                        <Icon name="box" size={13} /> {archived ? "取消归档" : "归档"}
                      </MenuItem>
                      <MenuSep />
                      <MenuItem
                        danger
                        onClick={() => {
                          setConfirming(true);
                          close();
                        }}
                      >
                        <Icon name="trash" size={13} /> 删除
                      </MenuItem>
                    </>
                  )}
                </Dropdown>
              </span>
            )}
          </>
        )}
      </div>

      {confirming && (
        <Modal
          title="删除这条对话？"
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
                  void deleteChat(chatId!, topicSlug);
                  setConfirming(false);
                }}
              >
                移入回收站
              </button>
            </>
          }
        >
          <div className="sub">
            「{title || "新对话"}」会被移动到{" "}
            <code className="mono">工作区/.hub/trash/</code>，不会真删。
            只是想让它别占地方的话，用「归档」更合适——随时能翻回来。
          </div>
        </Modal>
      )}
    </>
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
  depth,
  childCount,
  chatCount,
  expandable,
  collapsed,
  parentSlug,
  onToggle,
  onNewSubtopic,
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
  /** 缩进层级：0 = 顶层主题，1+ = 章/节 */
  depth: number;
  childCount: number;
  /** 这个主题下有几条对话（展开后列在它下面） */
  chatCount: number;
  /** 有子主题或对话时才有折叠箭头 */
  expandable: boolean;
  collapsed: boolean;
  parentSlug: string | null;
  onToggle: () => void;
  onNewSubtopic: () => void;
  onClick: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const updateTopic = useApp((s) => s.updateTopic);
  const deleteTopic = useApp((s) => s.deleteTopic);
  const parentName = useApp((s) =>
    parentSlug ? (s.topics.find((t) => t.slug === parentSlug)?.meta.name ?? parentSlug) : null,
  );

  return (
    <>
      <div
        data-slug={slug}
        className={"topic-row" + (active ? " active" : "") + (depth > 0 ? " child" : "")}
        style={depth > 0 ? { paddingLeft: 9 + depth * 14 } : undefined}
        onClick={onClick}
        title={name}
      >
        {/* 折叠箭头：展开后能看到子主题和对话 */}
        {expandable ? (
          <button
            className="icon-btn topic-chev"
            title={collapsed ? `展开（${childCount} 个子主题 / ${chatCount} 条对话）` : "收起"}
            onClick={(e) => {
              e.stopPropagation();
              onToggle();
            }}
          >
            <Icon name={collapsed ? "chevron-right" : "chevron-down"} size={12} />
          </button>
        ) : (
          <span className="topic-chev-space" />
        )}
        <span style={{ flex: "none", fontSize: 13 }}>{emoji ?? (depth > 0 ? "📄" : "📘")}</span>
        <span className="topic-name">{name}</span>

        <span className="topic-badges">
          {depth === 0 && childCount > 0 && (
            <span className="badge-dot" title={`${childCount} 个子主题（章节）`}>
              <Icon name="layers" size={11} />
              {childCount}
            </span>
          )}
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
                <MenuLabel>
                  {STAGE_LABEL[stage]}阶段 · {notes} 笔记 / {cards} 卡片
                </MenuLabel>
                <MenuSep />
                <MenuItem
                  onClick={() => {
                    void (async () => {
                      const store = useApp.getState();
                      if (store.topic?.slug !== slug) await store.openTopic(slug);
                      await useApp.getState().newChat();
                    })();
                    close();
                  }}
                >
                  <Icon name="chat" size={13} /> 在这里开新对话
                </MenuItem>
                <MenuItem
                  onClick={() => {
                    onNewSubtopic();
                    close();
                  }}
                >
                  <Icon name="plus" size={13} /> 新建子主题（只学其中一章）
                </MenuItem>
                {parentName && (
                  <MenuItem
                    onClick={() => {
                      void useApp.getState().setTopicParent(slug, null);
                      close();
                    }}
                  >
                    <Icon name="arrow-up" size={13} /> 移出「{parentName}」
                  </MenuItem>
                )}
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
          {childCount > 0 ? (
            <>
              这个主题下有 <b>{childCount}</b> 个子主题（章节），它们会一起被移入回收站。
              <div className="sub" style={{ marginTop: 6 }}>
                不会真删。整个目录会被移动到 <code className="mono">工作区/.hub/trash/</code>，
                需要时手动搬回来即可（父子关系记在各自的 topic.json 里，搬回来还在）。
              </div>
            </>
          ) : (
            <div className="sub">
              不会真删。整个目录会被移动到 <code className="mono">工作区/.hub/trash/</code>，
              需要时手动搬回来即可。
            </div>
          )}
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

  // 子主题在搜索结果里带上父主题名，免得「第三章」这类名字看不出是哪门课的
  const byId = new Map(topics.map((t) => [t.meta.id, t]));
  const withParent = (slug: string, name: string) => {
    const t = topics.find((x) => x.slug === slug);
    const parent = t?.meta.parent ? byId.get(t.meta.parent) : undefined;
    return parent ? `${parent.meta.name} › ${name}` : name;
  };

  const list = matches.length > 0
    ? matches.map((m) => ({ ...m, name: withParent(m.slug, m.name) }))
    : topics.map((t) => ({
        slug: t.slug,
        name: withParent(t.slug, t.meta.name),
        hits: [relTime(t.meta.lastOpenedAt ?? t.meta.updatedAt)],
      }));

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
