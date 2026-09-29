// 主题工作台：概览 / 笔记 / 资料 / 卡片 / 计划 / 会话。
//
// 这里是「学习资产」的管理界面——agent 在对话里产出的东西，都在这里被查看、编辑、复习。

import { Fragment, useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errText } from "../lib/api";
import { dueLabel, fmtDate, fmtDateTime, humanBytes, relTime } from "../lib/format";
import {
  CARD_KIND_LABEL,
  STAGE_LABEL,
  type Card,
  type CardKind,
  type Grade,
  type MaterialItem,
  type StudyStage,
  type TopicSummary,
} from "../lib/types";
import { useApp } from "../store/app";
import { Dropdown, Empty, Field, Icon, MenuItem, MenuSep, Modal, Segmented, Spinner, Switch } from "./ui";
import { Markdown } from "./Chat";
import { NoteEditor } from "./NoteEditor";
import { QuizPane } from "./Quiz";
import { LessonCard } from "./Lesson";

type PaneKey = "overview" | "notes" | "materials" | "cards" | "quiz" | "plan" | "sessions";

const PANES: { key: PaneKey; label: string }[] = [
  { key: "overview", label: "概览" },
  { key: "notes", label: "笔记" },
  { key: "materials", label: "资料" },
  { key: "cards", label: "卡片" },
  { key: "quiz", label: "测验" },
  { key: "plan", label: "计划" },
  { key: "sessions", label: "会话" },
];

export function Workbench() {
  const topic = useApp((s) => s.topic)!;
  const topics = useApp((s) => s.topics);
  const openTopic = useApp((s) => s.openTopic);
  const [pane, setPane] = useState<PaneKey>("overview");
  const inheritedCount = topic.materials.filter((m) => m.inherited).length;

  // 父主题链：子主题（一章）要能一眼看出属于哪门课，并且点回去
  const ancestors = useMemo(() => {
    const byId = new Map(topics.map((t) => [t.meta.id, t]));
    const out: TopicSummary[] = [];
    let cursor = topic.meta.parent ?? null;
    while (cursor && out.length < 8) {
      const parent = byId.get(cursor);
      if (!parent) break;
      out.unshift(parent);
      cursor = parent.meta.parent ?? null;
    }
    return out;
  }, [topics, topic.meta.parent]);

  return (
    <div className="workbench">
      <div className="wb-head">
        <div className="wb-title-row">
          <div className="wb-title">
            <span>{topic.meta.emoji ?? (topic.meta.parent ? "📄" : "📘")}</span>
            {ancestors.map((a) => (
              <Fragment key={a.slug}>
                <button className="crumb" onClick={() => void openTopic(a.slug)} title={`打开 ${a.meta.name}`}>
                  {a.meta.name}
                </button>
                <span className="crumb-sep">›</span>
              </Fragment>
            ))}
            <span>{topic.meta.name}</span>
          </div>
          <span className="tag">{STAGE_LABEL[topic.meta.stage]}</span>
          <div className="grow" />
          <button
            className="btn sm"
            onClick={() => void api.revealInExplorer(topic.slug).catch((e) => useApp.getState().toast("error", errText(e)))}
          >
            <Icon name="folder" size={13} /> 打开目录
          </button>
        </div>
        <div className="stat-inline">
          <span>
            笔记 <b>{topic.stats.notes}</b>
          </span>
          <span>
            资料 <b>{topic.stats.materials}</b>
            {inheritedCount > 0 && <span className="muted">（+父主题 {inheritedCount}）</span>}
          </span>
          <span>
            卡片 <b>{topic.stats.cards}</b>
            {topic.stats.cardsDue > 0 && <span style={{ color: "var(--accent)" }}>（到期 {topic.stats.cardsDue}）</span>}
          </span>
          <span>
            未完成任务 <b>{topic.stats.tasksOpen}</b>
          </span>
          <span>
            会话 <b>{topic.stats.sessions}</b>
          </span>
          <span className="mono" style={{ fontSize: 11 }}>
            {topic.path}
          </span>
        </div>
      </div>

      <div className="wb-tabs">
        {PANES.map((p) => (
          <button key={p.key} className={"wb-tab" + (pane === p.key ? " on" : "")} onClick={() => setPane(p.key)}>
            {p.label}
            {p.key === "cards" && topic.stats.cardsDue > 0 && <span className="cnt">{topic.stats.cardsDue}</span>}
            {p.key === "plan" && topic.stats.tasksOpen > 0 && <span className="cnt">{topic.stats.tasksOpen}</span>}
          </button>
        ))}
      </div>

      <div className="wb-body">
        {pane === "overview" && <OverviewPane />}
        {pane === "notes" && <NotesPane />}
        {pane === "materials" && <MaterialsPane />}
        {pane === "cards" && <CardsPane />}
        {pane === "quiz" && <QuizPane />}
        {pane === "plan" && <PlanPane />}
        {pane === "sessions" && <SessionsPane />}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 概览

function OverviewPane() {
  const topic = useApp((s) => s.topic)!;
  const updateTopic = useApp((s) => s.updateTopic);
  const setStage = useApp((s) => s.setStage);
  const session = topic.currentSession;
  const toast = useApp((s) => s.toast);

  const [desc, setDesc] = useState(topic.meta.description);
  const [tags, setTags] = useState(topic.meta.tags.join(", "));
  const [emoji, setEmoji] = useState(topic.meta.emoji ?? "");
  const [prompt, setPrompt] = useState<string | null>(null);
  const dirty = desc !== topic.meta.description || tags !== topic.meta.tags.join(", ") || emoji !== (topic.meta.emoji ?? "");

  return (
    <div className="wb-pane">
      <LessonCard />

      {session && (
        <div className="panel">
          <div className="panel-head">
            <Icon name="play" size={13} />
            <span className="title">正在进行的学习会话</span>
            <span className="grow" />
            <span className="mono muted">{fmtDateTime(session.startedAt)}</span>
          </div>
          <div className="panel-body col">
            <div style={{ fontWeight: 500 }}>{session.title}</div>
            {session.goals.length > 0 && (
              <div className="sub" style={{ fontSize: 12.5 }}>
                目标：{session.goals.join("；")}
              </div>
            )}
            <div className="row">
              <button
                className="btn sm"
                onClick={() => void useApp.getState().finishSession("（手动结束，未填写总结）")}
              >
                结束会话
              </button>
              <span className="muted" style={{ fontSize: 11.5 }}>
                已产出 {session.cardsCreated} 张卡片、{session.notesCreated} 篇笔记
              </span>
            </div>
          </div>
        </div>
      )}

      <div className="card-box">
        <div className="row">
          <h2 style={{ margin: 0, fontSize: 14 }}>主题信息</h2>
          <span className="grow" />
          {dirty && (
            <button
              className="btn primary sm"
              onClick={() => {
                void updateTopic({
                  description: desc,
                  emoji,
                  tags: tags
                    .split(/[,，\s]+/)
                    .map((t) => t.trim())
                    .filter(Boolean),
                });
              }}
            >
              保存
            </button>
          )}
        </div>
        <div className="grid-2">
          <Field label="图标（一个 emoji）">
            <input className="input" value={emoji} placeholder="📘" onChange={(e) => setEmoji(e.target.value)} maxLength={4} />
          </Field>
          <Field label="标签（逗号分隔）">
            <input className="input" value={tags} placeholder="线代, 期末" onChange={(e) => setTags(e.target.value)} />
          </Field>
        </div>
        <Field label="一句话说明" hint="agent 每次对话都会读到它，写清「学它做什么」最有帮助">
          <textarea className="textarea" value={desc} onChange={(e) => setDesc(e.target.value)} rows={3} />
        </Field>
        <div className="row">
          <span className="muted" style={{ fontSize: 12 }}>
            当前阶段
          </span>
          {(Object.keys(STAGE_LABEL) as StudyStage[]).map((s) => (
            <button
              key={s}
              className={"chip" + (topic.meta.stage === s ? " primary" : "")}
              onClick={() => void setStage(s)}
            >
              {STAGE_LABEL[s]}
            </button>
          ))}
        </div>
      </div>

      <div className="card-box">
        <div className="row">
          <h2 style={{ margin: 0, fontSize: 14 }}>快捷操作</h2>
        </div>
        <div className="row wrap">
          <button className="btn sm" onClick={() => void useApp.getState().startSession(`${topic.meta.name} 学习`)}>
            <Icon name="play" size={13} /> 开始学习会话
          </button>
          <button
            className="btn sm"
            onClick={async () => {
              try {
                setPrompt(await api.promptPreview(topic.slug));
              } catch (e) {
                toast("error", errText(e));
              }
            }}
          >
            <Icon name="eye" size={13} /> 看看 agent 眼中的我
          </button>
        </div>
      </div>

      {prompt !== null && (
        <Modal title="系统提示词（agent 眼中的世界）" icon="eye" wide onClose={() => setPrompt(null)}>
          <pre
            style={{
              margin: 0,
              whiteSpace: "pre-wrap",
              fontFamily: "var(--font-mono)",
              fontSize: 12,
              lineHeight: 1.7,
              background: "var(--bg-code)",
              border: "1px solid var(--border)",
              borderRadius: 8,
              padding: 12,
              maxHeight: "60vh",
              overflow: "auto",
            }}
          >
            {prompt}
          </pre>
        </Modal>
      )}

      {topic.readme && (
        <div className="panel">
          <div className="panel-head">
            <Icon name="file" size={13} />
            <span className="title">README.md（主题背景资料）</span>
          </div>
          <div className="panel-body">
            <Markdown source={topic.readme} onLink={() => {}} />
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 笔记

function NotesPane() {
  const topic = useApp((s) => s.topic)!;
  const [q, setQ] = useState("");
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");

  const notes = useMemo(() => {
    const query = q.trim().toLowerCase();
    if (!query) return topic.notes;
    return topic.notes.filter(
      (n) =>
        n.title.toLowerCase().includes(query) ||
        n.tags.some((t) => t.toLowerCase().includes(query)) ||
        n.excerpt.toLowerCase().includes(query),
    );
  }, [topic.notes, q]);

  if (openPath) {
    return (
      <div className="note-surface">
        <div className="row" style={{ padding: "6px 12px", gap: 8, borderBottom: "1px solid var(--border)" }}>
          <button className="icon-btn" title="返回笔记本列表" onClick={() => setOpenPath(null)}>
            <Icon name="arrow-left" />
          </button>
          <span style={{ fontWeight: 500 }}>{openPath.split("/").pop()}</span>
          <div className="grow" />
          <span className="muted" style={{ fontSize: 11.5 }}>
            所见即所得 · Ctrl+S 保存 · Ctrl+/ 切源码
          </span>
        </div>
        <NoteEditor
          slug={topic.slug}
          topicDir={topic.path}
          path={openPath}
          onSaved={() =>
            void api.topicGet(topic.slug).then((d) => useApp.setState({ topic: d, sessions: d.sessions }))
          }
        />
      </div>
    );
  }

  return (
    <div className="wb-pane">
      <div className="row">
        <input
          className="input"
          placeholder="搜索笔记标题 / 标签 / 摘要"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <button className="btn" onClick={() => setCreating(true)}>
          <Icon name="plus" size={13} /> 新建笔记
        </button>
      </div>

      {notes.length === 0 ? (
        <Empty icon="file">
          还没有笔记。
          <br />
          在对话里让 agent「把这个总结写成笔记」，或者点上方新建。
        </Empty>
      ) : (
        <div className="col" style={{ gap: 4 }}>
          {notes.map((n) => (
            <div key={n.path} className="list-row" onClick={() => setOpenPath(n.path)}>
              <Icon name="file" size={14} />
              <div className="li-main">
                <div className="li-title">{n.title}</div>
                <div className="li-sub">{n.excerpt || n.path}</div>
              </div>
              {n.tags.slice(0, 3).map((t) => (
                <span key={t} className="tag">
                  {t}
                </span>
              ))}
              <span className="muted mono" style={{ fontSize: 11 }}>
                {humanBytes(n.size)}
              </span>
              <span className="muted" style={{ fontSize: 11 }}>
                {relTime(n.updatedAt)}
              </span>
            </div>
          ))}
        </div>
      )}

      {creating && (
        <Modal
          title="新建笔记"
          icon="file"
          onClose={() => setCreating(false)}
          footer={
            <>
              <button className="btn" onClick={() => setCreating(false)}>
                取消
              </button>
              <button
                className="btn primary"
                disabled={!newTitle.trim()}
                onClick={async () => {
                  const note = await useApp.getState().createNote(newTitle.trim());
                  setCreating(false);
                  setNewTitle("");
                  if (note) setOpenPath(note.path);
                }}
              >
                创建并编辑
              </button>
            </>
          }
        >
          <Field label="笔记标题" hint="会成为文件名，例如「特征值与特征向量」">
            <input
              className="input"
              autoFocus
              value={newTitle}
              onChange={(e) => setNewTitle(e.target.value)}
              onKeyDown={async (e) => {
                if (e.key === "Enter" && newTitle.trim()) {
                  const note = await useApp.getState().createNote(newTitle.trim());
                  setCreating(false);
                  setNewTitle("");
                  if (note) setOpenPath(note.path);
                }
              }}
            />
          </Field>
        </Modal>
      )}

      <div className="muted" style={{ fontSize: 11.5 }}>
        笔记就是 <code className="mono">notes/</code> 下的 Markdown 文件，可以直接用 VS Code / Typora 编辑，
        应用每次打开都会重新扫描。
      </div>
    </div>
  );
}


// ---------------------------------------------------------------- 资料

function MaterialsPane() {
  const topic = useApp((s) => s.topic)!;
  const openFile = useApp((s) => s.openFile);
  const toast = useApp((s) => s.toast);
  const [busy, setBusy] = useState(false);

  // 本主题的资料 + 从父主题继承来的（父主题的只读，分组显示）
  const own = topic.materials.filter((m) => !m.inherited);
  const inheritedBy = useMemo(() => {
    const groups = new Map<string, MaterialItem[]>();
    for (const m of topic.materials) {
      if (!m.inherited) continue;
      const key = m.origin ?? "父主题";
      groups.set(key, [...(groups.get(key) ?? []), m]);
    }
    return [...groups.entries()];
  }, [topic.materials]);

  async function importFiles() {
    try {
      const picked = await openDialog({ multiple: true, title: "选择资料（会复制到 materials/）" });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      setBusy(true);
      await useApp.getState().importMaterials(paths as string[]);
      setBusy(false);
    } catch (e) {
      setBusy(false);
      toast("error", errText(e));
    }
  }

  /** 点开一份资料：继承来的要用它自己所属主题去开（浏览器标签也是那个主题的） */
  const open = (m: MaterialItem) => void openFile(m.path, m.name, undefined, m.topic);

  const row = (m: MaterialItem) => (
    <div key={`${m.topic}/${m.path}`} className="list-row" onClick={() => open(m)}>
      <Icon name={m.kind === "pdf" || m.kind === "md" ? "file" : "layers"} size={14} />
      <div className="li-main">
        <div className="li-title">{m.name}</div>
        <div className="li-sub mono">{m.path}</div>
      </div>
      <span className="tag">{m.kind || "文件"}</span>
      <span className="muted mono" style={{ fontSize: 11 }}>
        {m.sizeText}
      </span>
      <span className="muted" style={{ fontSize: 11 }}>
        {relTime(m.modifiedAt)}
      </span>
    </div>
  );

  return (
    <div className="wb-pane">
      <div className="row">
        <span className="sub" style={{ fontSize: 12.5 }}>
          资料保存在主题的 <code className="mono">materials/</code> 目录，agent 可以直接读 PDF 的文字。
          {inheritedBy.length > 0 && " 带「继承」标记的来自父主题，点开就能看，原件不用复制过来。"}
        </span>
        <div className="grow" />
        <button
          className="btn"
          title="把 materials/ 与 kb/ 里的讲义重新抽成可检索文本。PDF 的页码来自内置浏览器读过的分页文本，所以引用能定位到具体页"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              const r = await api.kbRebuild(topic.slug);
              toast("success", `索引已重建：${r.files} 份文件、${r.chunks} 个片段（带页码的 ${r.paged} 份）`);
            } catch (e) {
              toast("error", errText(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          <Icon name="refresh" size={13} /> 重建索引
        </button>
        <button className="btn primary" onClick={importFiles} disabled={busy}>
          {busy ? <Spinner /> : <Icon name="download" size={13} />} 导入资料
        </button>
      </div>

      {topic.materials.length === 0 ? (
        <div className="drop-hint" onClick={importFiles}>
          <Icon name="download" size={22} />
          <div style={{ fontSize: 13.5, color: "var(--text-sub)" }}>
            把课件、论文、截图拖进窗口，或点这里选择文件
          </div>
          <div className="muted" style={{ fontSize: 12 }}>
            会复制进 <code>materials/</code>，之后 agent 就能读 PDF 文字、按页码引用
          </div>
        </div>
      ) : (
        <div className="col" style={{ gap: 10 }}>
          {own.length > 0 && <div className="col" style={{ gap: 4 }}>{own.map(row)}</div>}
          {inheritedBy.map(([source, items]) => (
            <div key={source} className="col" style={{ gap: 4 }}>
              <div className="group-head">
                <Icon name="layers" size={12} /> 继承自「{source}」
                <span className="muted">（只读）</span>
              </div>
              {items.map(row)}
            </div>
          ))}
          {own.length === 0 && (
            <div className="muted" style={{ fontSize: 12 }}>
              本主题还没有自己的资料——上面这些是父主题的，想单独放就点「导入资料」。
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 卡片

/** 复习间隔的显示文案：10 分钟 / 3 小时 / 12 天 / 2.5 个月 */
function intervalText(secs: number): string {
  const minutes = secs / 60;
  if (minutes < 60) return `${Math.max(1, Math.round(minutes))} 分钟`;
  const hours = minutes / 60;
  if (hours < 24) return `${Math.round(hours)} 小时`;
  const days = hours / 24;
  if (days < 31) return `${Math.round(days)} 天`;
  const months = days / 30.4;
  if (months < 12) return `${months.toFixed(1)} 个月`;
  return `${(days / 365).toFixed(1)} 年`;
}

function CardsPane() {
  const topic = useApp((s) => s.topic)!;
  const cards = useApp((s) => s.cards);
  const loadCards = useApp((s) => s.loadCards);
  const reviewCard = useApp((s) => s.reviewCard);

  const [dueOnly, setDueOnly] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [showBack, setShowBack] = useState(false);
  const [adding, setAdding] = useState(false);
  /// 复习队列：进入复习时固定下来，评完一张就前进一张
  const [queue, setQueue] = useState<Card[]>([]);

  useEffect(() => {
    void loadCards({ dueOnly });
  }, [dueOnly, loadCards]);

  const current = reviewing ? queue[0] : undefined;

  function startReview() {
    const due = cards.filter((c) => new Date(c.srs.due) <= new Date());
    const picked = due.length > 0 ? due : cards;
    setQueue(picked);
    setShowBack(false);
    setReviewing(true);
  }

  async function grade(g: Grade) {
    if (!current) return;
    await reviewCard(current.id, g);
    setShowBack(false);
    setQueue((q) => q.slice(1));
  }

  async function exitReview() {
    setReviewing(false);
    setQueue([]);
    setShowBack(false);
    void loadCards({ dueOnly });
    // 复习结束后把主题统计与总览刷新一下（角标、待复习数都会变）
    await useApp.getState().refreshTopics();
    await useApp.getState().refreshBrief();
    try {
      const detail = await api.topicGet(topic.slug);
      useApp.setState({ topic: detail, sessions: detail.sessions });
    } catch {
      /* 忽略：统计刷新失败不影响使用 */
    }
  }

  // 键盘操作：空格/回车翻面，1~4 打分，Esc 结束（打字时让开）
  useEffect(() => {
    if (!reviewing) return;
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        setShowBack(true);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        void exitReview();
        return;
      }
      const map: Record<string, Grade> = { "1": "again", "2": "hard", "3": "good", "4": "easy" };
      const g = map[e.key];
      if (showBack && g) {
        e.preventDefault();
        void grade(g);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reviewing, showBack, current?.id]);

  /** 把后端算好的间隔秒数写成「10 分钟 / 3 小时 / 12 天 / 2 个月」 */
  const label = (secs?: number, fallback = "") => (secs && secs > 0 ? intervalText(secs) : fallback);

  return (
    <div className="wb-pane">
      <div className="row">
        <Switch checked={dueOnly} onChange={setDueOnly} label="只看今天到期" />
        <span className="muted" style={{ fontSize: 12 }}>
          共 {topic.stats.cards} 张，到期 {topic.stats.cardsDue} 张
        </span>
        <div className="grow" />
        <button className={"btn" + (reviewing ? "" : " primary")} onClick={() => (reviewing ? void exitReview() : startReview())}>
          <Icon name={reviewing ? "close" : "play"} size={13} /> {reviewing ? "结束复习" : "开始复习"}
        </button>
        <button className="btn" onClick={() => setAdding(true)}>
          <Icon name="plus" size={13} /> 新建卡片
        </button>
      </div>

      {reviewing ? (
        current ? (
          <div className="col" style={{ gap: 12 }}>
            <div className="review-card" onClick={() => setShowBack(true)}>
              <div className="muted" style={{ fontSize: 11.5 }}>
                还剩 {queue.length} 张 ·{" "}
                {current.srs.reviews === 0 ? "新卡" : `间隔 ${current.srs.intervalDays.toFixed(1)} 天`}
              </div>
              <div className="review-front">
                <Markdown source={current.front} onLink={() => {}} />
              </div>
              {showBack ? (
                <div className="review-back">
                  <Markdown source={current.back} onLink={() => {}} />
                  {current.source && (
                    <div className="muted mono" style={{ fontSize: 11, marginTop: 8 }}>
                      出处：{current.source}
                    </div>
                  )}
                </div>
              ) : (
                <div className="muted" style={{ fontSize: 12 }}>
                  先自己回想一遍，然后点这里看答案（或按空格）
                </div>
              )}
            </div>

            {showBack ? (
              <div className="grade-row">
                <button className="grade-btn" onClick={() => void grade("again")}>
                  忘了 <small>1 · {label(current.preview?.again, "10 分钟")}</small>
                </button>
                <button className="grade-btn" onClick={() => void grade("hard")}>
                  吃力 <small>2 · {label(current.preview?.hard, "缩短间隔")}</small>
                </button>
                <button className="grade-btn" onClick={() => void grade("good")}>
                  记得 <small>3 · {label(current.preview?.good, "正常推进")}</small>
                </button>
                <button className="grade-btn" onClick={() => void grade("easy")}>
                  太简单 <small>4 · {label(current.preview?.easy, "拉长间隔")}</small>
                </button>
              </div>
            ) : (
              <button className="btn primary" onClick={() => setShowBack(true)}>
                显示答案
              </button>
            )}

            <div className="row">
              <button
                className="btn ghost sm"
                onClick={async () => {
                  await useApp.getState().deleteCard(current.id);
                  setQueue((q) => q.slice(1));
                  setShowBack(false);
                }}
              >
                <Icon name="trash" size={12} /> 删掉这张
              </button>
            </div>
          </div>
        ) : (
          <Empty icon="check">这一轮复习完了 🎉</Empty>
        )
      ) : cards.length === 0 ? (
        <Empty icon="layers">
          还没有卡片。
          <br />
          让 agent 在讲完之后「把要点存成卡片」，之后就在这里按间隔重复复习
          （SM-2 调度就在本应用内完成）。
        </Empty>
      ) : (
        <div className="col" style={{ gap: 4 }}>
          {cards.map((c) => (
            <CardRow key={c.id} card={c} />
          ))}
        </div>
      )}

      {adding && <AddCardDialog onClose={() => setAdding(false)} />}
    </div>
  );
}

function CardRow({ card }: { card: Card }) {
  const [open, setOpen] = useState(false);
  const due = new Date(card.srs.due);
  const isDue = due <= new Date();
  return (
    <div className="list-row" style={{ alignItems: "flex-start" }} onClick={() => setOpen((v) => !v)}>
      <Icon name={isDue ? "target" : "layers"} size={14} style={{ marginTop: 3 }} />
      <div className="li-main">
        <div className="li-title">{card.front}</div>
        {open && card.back && (
          <div className="sub" style={{ fontSize: 12.5, marginTop: 4, whiteSpace: "pre-wrap" }}>
            {card.back}
          </div>
        )}
        <div className="li-sub" style={{ marginTop: 2 }}>
          {card.module ? `${card.module}｜` : ""}
          {card.srs.reviews === 0 ? "新卡" : `复习 ${card.srs.reviews} 次｜难度 ${card.srs.ease.toFixed(2)}`}
          {card.source ? `｜${card.source}` : ""}
        </div>
      </div>
      {card.kind !== "basic" && <span className="tag">{CARD_KIND_LABEL[card.kind]}</span>}
      <span className={"tag" + (isDue ? " accent" : "")}>{isDue ? "待复习" : dueLabel(card.srs.due)}</span>
      <button
        className="icon-btn"
        title="删除"
        onClick={(e) => {
          e.stopPropagation();
          void useApp.getState().deleteCard(card.id);
        }}
      >
        <Icon name="trash" size={13} />
      </button>
    </div>
  );
}

function AddCardDialog({ onClose }: { onClose: () => void }) {
  const [front, setFront] = useState("");
  const [back, setBack] = useState("");
  const [tags, setTags] = useState("");
  const [kind, setKind] = useState<CardKind>("basic");
  const create = useApp((s) => s.createCard);
  const isCloze = kind === "cloze";
  return (
    <Modal
      title="新建卡片"
      icon="plus"
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            一张卡只考一个点
          </span>
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button
            className="btn primary"
            disabled={!front.trim() || (!isCloze && !back.trim())}
            onClick={async () => {
              await create({
                front: front.trim(),
                kind,
                back: isCloze ? "" : back.trim(),
                tags: tags.split(/[,，\s]+/).filter(Boolean),
              });
              onClose();
            }}
          >
            保存
          </button>
        </>
      }
    >
      <Field label="卡片类型" hint={isCloze ? "完形卡：用 {{c1::}} 标出要挖空的部分，可以写多个 {{c2::}}、{{c3::}}" : "基础 = 正问反答；反向 = 两面都能问"}>
        <Segmented
          value={kind}
          onChange={(v) => setKind(v)}
          options={[
            { id: "basic", label: "基础" },
            { id: "reversed", label: "反向" },
            { id: "cloze", label: "完形" },
          ]}
        />
      </Field>
      <Field label={isCloze ? "正文（含 {{c1::…}} 标记）" : "正面（问题）"}>
        <textarea
          className="textarea"
          autoFocus
          rows={3}
          value={front}
          placeholder={isCloze ? "毛细血管壁由 {{c1::单层内皮细胞}} 和 {{c2::基膜}} 构成" : ""}
          onChange={(e) => setFront(e.target.value)}
        />
      </Field>
      {!isCloze && (
        <Field label="背面（答案）">
          <textarea className="textarea" rows={3} value={back} onChange={(e) => setBack(e.target.value)} />
        </Field>
      )}
      <Field label="标签">
        <input className="input" value={tags} onChange={(e) => setTags(e.target.value)} placeholder="线代, 定义" />
      </Field>
    </Modal>
  );
}

// ---------------------------------------------------------------- 计划

function PlanPane() {
  const topic = useApp((s) => s.topic)!;
  const tasks = useApp((s) => s.tasks).filter((t) => t.topicSlug === topic.slug);
  const loadTasks = useApp((s) => s.loadTasks);
  const updateTask = useApp((s) => s.updateTask);
  const [title, setTitle] = useState("");
  const [due, setDue] = useState("");

  useEffect(() => {
    void loadTasks();
  }, [loadTasks]);

  const open = tasks.filter((t) => t.task.status === "todo" || t.task.status === "doing");
  const done = tasks.filter((t) => t.task.status === "done" || t.task.status === "archived");

  return (
    <div className="wb-pane">
      <div className="row">
        <input
          className="input"
          placeholder="加一条学习计划，例如「读完第 3 章并做完习题」"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={async (e) => {
            if (e.key === "Enter" && title.trim()) {
              await useApp.getState().createTask({ title: title.trim(), due: due.trim() || null });
              setTitle("");
            }
          }}
        />
        <input
          className="input"
          style={{ width: 140 }}
          placeholder="截止 如 明天 / 10-01"
          value={due}
          onChange={(e) => setDue(e.target.value)}
        />
        <button
          className="btn primary"
          disabled={!title.trim()}
          onClick={async () => {
            await useApp.getState().createTask({ title: title.trim(), due: due.trim() || null });
            setTitle("");
            setDue("");
          }}
        >
          <Icon name="plus" size={13} /> 添加
        </button>
      </div>

      {open.length === 0 && done.length === 0 ? (
        <Empty icon="calendar">还没有计划。给自己排一个「今天读完第 1 章」也行。</Empty>
      ) : (
        <>
          <div className="col" style={{ gap: 5 }}>
            {open.map(({ task, overdue }) => (
              <div key={task.id} className="task-row">
                <button
                  className="check"
                  title="标记完成"
                  onClick={() => void updateTask(task.id, { status: "done" })}
                />
                <div className="grow">
                  <div style={{ fontWeight: 450 }}>{task.title}</div>
                  {task.detail && <div className="muted" style={{ fontSize: 12 }}>{task.detail}</div>}
                </div>
                {task.priority === 3 && <span className="tag accent">高优先</span>}
                {task.due && <span className={"tag" + (overdue ? " accent" : "")}>{dueLabel(task.due)}</span>}
                <Dropdown
                  trigger={() => (
                    <button className="icon-btn">
                      <Icon name="more" size={14} />
                    </button>
                  )}
                >
                  {(close) => (
                    <>
                      <MenuItem
                        onClick={() => {
                          void updateTask(task.id, { status: "doing" });
                          close();
                        }}
                      >
                        <Icon name="play" size={13} /> 标记进行中
                      </MenuItem>
                      <MenuItem
                        onClick={() => {
                          void updateTask(task.id, { priority: task.priority === 3 ? 2 : 3 });
                          close();
                        }}
                      >
                        <Icon name="target" size={13} /> {task.priority === 3 ? "取消高优先" : "标为高优先"}
                      </MenuItem>
                      <MenuSep />
                      <MenuItem
                        danger
                        onClick={() => {
                          void useApp.getState().deleteTask(task.id);
                          close();
                        }}
                      >
                        <Icon name="trash" size={13} /> 删除
                      </MenuItem>
                    </>
                  )}
                </Dropdown>
              </div>
            ))}
          </div>

          {done.length > 0 && (
            <div className="col" style={{ gap: 5, marginTop: 10 }}>
              <div className="muted" style={{ fontSize: 12 }}>
                已完成（{done.length}）
              </div>
              {done.slice(0, 20).map(({ task }) => (
                <div key={task.id} className="task-row done">
                  <button
                    className="check on"
                    title="重新打开"
                    onClick={() => void updateTask(task.id, { status: "todo" })}
                  >
                    <Icon name="check" size={10} />
                  </button>
                  <div className="grow" style={{ textDecoration: "line-through" }}>
                    {task.title}
                  </div>
                  <span className="muted" style={{ fontSize: 11 }}>
                    {task.doneAt ? fmtDate(task.doneAt) : ""}
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 会话

function SessionsPane() {
  const topic = useApp((s) => s.topic)!;
  const sessions = topic.sessions;

  return (
    <div className="wb-pane">
      <div className="row">
        <span className="sub" style={{ fontSize: 12.5 }}>
          每次「预习 / 学习 / 复习 / 测验」都会记成一次会话，包括目标、产出与遗留问题。
        </span>
        <div className="grow" />
        <button className="btn" onClick={() => void useApp.getState().startSession(`${topic.meta.name} 学习`)}>
          <Icon name="play" size={13} /> 开始新会话
        </button>
      </div>

      {sessions.length === 0 ? (
        <Empty icon="clock">还没有会话记录。在对话里说「我们开始学 X」就会自动开始一次。</Empty>
      ) : (
        <div className="col" style={{ gap: 8 }}>
          {sessions.map((s) => (
            <div key={s.id} className="card-box" style={{ gap: 8 }}>
              <div className="row">
                <span className="tag accent">{STAGE_LABEL[s.stage]}</span>
                <span style={{ fontWeight: 500 }}>{s.title}</span>
                <div className="grow" />
                <span className="muted mono" style={{ fontSize: 11 }}>
                  {fmtDateTime(s.startedAt)} · {s.endedAt ? `${Math.max(1, Math.round((new Date(s.endedAt).getTime() - new Date(s.startedAt).getTime()) / 60000))} 分钟` : "进行中"}
                </span>
              </div>
              {s.goals.length > 0 && <div className="muted" style={{ fontSize: 12 }}>目标：{s.goals.join("；")}</div>}
              {s.summary && <div className="sub" style={{ fontSize: 12.5 }}>{s.summary}</div>}
              {s.highlights.length > 0 && (
                <div className="col" style={{ gap: 3 }}>
                  {s.highlights.map((h, i) => (
                    <div key={i} className="row" style={{ gap: 6, fontSize: 12.5 }}>
                      <Icon name="check" size={12} style={{ color: "var(--ok)" }} />
                      {h}
                    </div>
                  ))}
                </div>
              )}
              {s.openQuestions.length > 0 && (
                <div className="col" style={{ gap: 3 }}>
                  <div className="muted" style={{ fontSize: 11.5 }}>遗留问题</div>
                  {s.openQuestions.map((q, i) => (
                    <div key={i} className="row" style={{ gap: 6, fontSize: 12.5, color: "var(--warn)" }}>
                      <Icon name="alert" size={12} />
                      {q}
                    </div>
                  ))}
                </div>
              )}
              <div className="row muted" style={{ fontSize: 11.5 }}>
                <Icon name="layers" size={11} /> 卡片 {s.cardsCreated}
                <Icon name="file" size={11} /> 笔记 {s.notesCreated}
                {s.materials.length > 0 && <span>资料 {s.materials.join("、")}</span>}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
