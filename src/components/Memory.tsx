// 侧栏的「记忆」面板：查看与编辑 agent 记住的长期信息。
//
// 设计意图：记忆必须是**用户能看懂、能改、能删**的东西。
// 所以这里不做任何隐藏状态——agent 用 memory_write 记下的每一条，
// 都会出现在这个面板里，和用户手写的完全一样；反过来用户在这里改的，
// 下一轮对话就会生效（每轮都重新读盘）。
//
// 两级作用域与技能 / MCP 保持一致：
// - **全局**：跨主题都成立（称呼、作息、通用偏好）
// - **本主题**：只在这个主题生效；父主题的记忆会继承下来（只读展示）

import { useCallback, useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import {
  MEMORY_SCOPE_LABEL,
  type MemoryItem,
  type MemoryKind,
  type MemoryKindInfo,
  type MemoryOverview,
  type MemoryPaths,
  type MemoryScope,
} from "../lib/types";
import { useApp } from "../store/app";
import { Field, Icon, Modal, Spinner, Switch } from "./ui";

/** 兜底的中文名：后端 memory_kinds 拿不到时界面也不至于显示英文 id */
const FALLBACK_KINDS: MemoryKindInfo[] = [
  { id: "fact", label: "情况", hint: "关于用户本人的情况" },
  { id: "preference", label: "偏好", hint: "学习方式的偏好" },
  { id: "goal", label: "目标", hint: "要达成的目标" },
  { id: "pitfall", label: "注意", hint: "以后需要注意的地方" },
  { id: "style", label: "讲法", hint: "输出约定" },
  { id: "gap", label: "缺口", hint: "还没掌握的前置" },
];

const KIND_COLOR: Record<MemoryKind, string> = {
  fact: "var(--text-dim)",
  preference: "var(--accent)",
  goal: "var(--warn)",
  pitfall: "var(--danger)",
  style: "var(--ok)",
  gap: "var(--warn)",
};

function kindsOf(list: MemoryKindInfo[], id: MemoryKind): MemoryKindInfo | undefined {
  return list.find((k) => k.id === id);
}

function shortDate(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function MemoryPanel({ onClose }: { onClose: () => void }) {
  const slug = useApp((s) => s.topic?.slug ?? null);
  const toast = useApp((s) => s.toast);
  // agent 在对话里记下 / 删掉记忆时 +1：面板开着就跟着刷新
  const tick = useApp((s) => s.memoryTick);
  const [data, setData] = useState<MemoryOverview | null>(null);
  const [kinds, setKinds] = useState<MemoryKindInfo[]>(FALLBACK_KINDS);
  const [paths, setPaths] = useState<MemoryPaths | null>(null);
  const [editing, setEditing] = useState<MemoryItem | null>(null);
  const [creating, setCreating] = useState(false);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setData(await api.memoryOverview(slug));
    } catch (e) {
      toast("error", errText(e));
    }
  }, [slug, toast]);

  useEffect(() => {
    void refresh();
    void api.memoryKinds().then(setKinds).catch(() => setKinds(FALLBACK_KINDS));
    void api.memoryPaths(slug).then(setPaths).catch(() => setPaths(null));
  }, [refresh, slug, tick]);

  /** 记忆面板自己发起的改动，改完顺手把总览刷新一遍 */
  const after = useCallback(
    (next: MemoryOverview) => {
      setData(next);
      setEditing(null);
      setCreating(false);
    },
    [],
  );

  async function togglePin(m: MemoryItem) {
    try {
      after(await api.memorySetPinned(m.id, !m.pinned, m.scope, m.topicSlug ?? slug));
    } catch (e) {
      toast("error", errText(e));
    }
  }

  async function remove(m: MemoryItem) {
    try {
      after(await api.memoryDelete(m.id, m.scope, m.topicSlug ?? slug));
      toast("info", "已删除这条记忆");
    } catch (e) {
      toast("error", errText(e));
    }
  }

  async function clear(scope: MemoryScope) {
    const n = scope === "global" ? data?.global.length ?? 0 : data?.topic.length ?? 0;
    if (n === 0) return;
    if (!window.confirm(`确定清空${MEMORY_SCOPE_LABEL[scope]}的 ${n} 条记忆吗？此操作不可撤销。`)) return;
    setBusy(true);
    try {
      const removed = await api.memoryClear(scope, slug);
      await refresh();
      toast("info", `已清空 ${removed} 条记忆`);
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  const active = data?.activeCount ?? 0;
  const total = (data?.global.length ?? 0) + (data?.topic.length ?? 0) + (data?.inherited.length ?? 0);

  return (
    <Modal
      title="记忆"
      icon="sparkle"
      wide
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            {paths ? `全局记忆文件：${paths.global}` : ""}
          </span>
          <button
            className="btn"
            disabled={busy}
            onClick={async () => {
              try {
                const p = await api.revealInExplorer();
                toast("info", `已在文件管理器中打开：${p}`);
              } catch (e) {
                toast("error", errText(e));
              }
            }}
          >
            <Icon name="folder" size={13} /> 打开目录
          </button>
          <button
            className="btn primary"
            onClick={() => {
              setEditing(null);
              setCreating(true);
            }}
          >
            <Icon name="plus" size={13} /> 添加记忆
          </button>
        </>
      }
    >
      {data ? (
        <>
          <div className="row" style={{ gap: 10 }}>
            <Switch
              checked={data.enabled}
              onChange={async (v) => {
                try {
                  setData(await api.memorySetEnabled(v, slug));
                  toast("info", v ? "记忆已开启：每轮都会带上相关记忆" : "记忆已关闭：不再注入提示词");
                } catch (e) {
                  toast("error", errText(e));
                }
              }}
              label="启用长期记忆"
            />
            <div className="grow" />
            <span className="muted" style={{ fontSize: 11.5, textAlign: "right" }}>
              共 {total} 条，当前上下文会带上 {active} 条（约 {data.digestChars} 字）。
              <br />
              每轮对话开始时重新读盘，改完立刻生效。
            </span>
          </div>

          <div className="card-box" style={{ background: "var(--bg-sub)", gap: 4 }}>
            <div style={{ fontSize: 12.5, lineHeight: 1.7 }} className="muted">
              agent 会在对话里自己判断该记什么（你的基础、偏好、反复出错的点、还缺的前置），
              也可以由你在这里手写。钉住的条目永远优先注入；「用过几次」记录它被带进提示词的次数——
              一直是 0 说明这条记了没派上用场，可以删掉。
            </div>
          </div>

          {(creating || editing) && (
            <MemoryForm
              kinds={kinds}
              initial={editing}
              hasTopic={!!slug}
              busy={busy}
              onCancel={() => {
                setCreating(false);
                setEditing(null);
              }}
              onSubmit={async (input, scope) => {
                setBusy(true);
                try {
                  after(await api.memoryUpsert(input, scope, slug));
                  toast("success", editing ? "记忆已更新" : "已记下");
                } catch (e) {
                  toast("error", errText(e));
                } finally {
                  setBusy(false);
                }
              }}
            />
          )}

          <Group
            title={`全局（${data.global.length}）`}
            hint="所有主题都成立：称呼、作息、通用偏好"
            empty="还没有全局记忆"
            items={data.global}
            kinds={kinds}
            onEdit={(m) => {
              setCreating(false);
              setEditing(m);
            }}
            onPin={togglePin}
            onDelete={remove}
            onClear={() => void clear("global")}
          />

          <Group
            title={data.topicName ? `本主题：${data.topicName}（${data.topic.length}）` : "本主题"}
            hint={data.topicName ? "只在这个主题里生效" : "先打开一个主题，才能添加只属于它的记忆"}
            empty={data.topicName ? "这个主题还没有自己的记忆" : "（未打开主题）"}
            items={data.topic}
            kinds={kinds}
            onEdit={(m) => {
              setCreating(false);
              setEditing(m);
            }}
            onPin={togglePin}
            onDelete={remove}
            onClear={() => void clear("topic")}
          />

          {data.inherited.length > 0 && (
            <Group
              title={`继承自父主题（${data.inherited.length}）`}
              hint="同一门课里记下的事，学这一章时同样生效；要改请去父主题"
              items={data.inherited}
              kinds={kinds}
              readOnly
            />
          )}
        </>
      ) : (
        <div className="row" style={{ justifyContent: "center", padding: 20 }}>
          <Spinner />
        </div>
      )}
    </Modal>
  );
}

/** 一组记忆（全局 / 本主题 / 继承） */
function Group({
  title,
  hint,
  empty,
  items,
  kinds,
  onEdit,
  onPin,
  onDelete,
  onClear,
  readOnly,
}: {
  title: string;
  hint: string;
  empty?: string;
  items: MemoryItem[];
  kinds: MemoryKindInfo[];
  onEdit?: (m: MemoryItem) => void;
  onPin?: (m: MemoryItem) => void;
  onDelete?: (m: MemoryItem) => void;
  onClear?: () => void;
  readOnly?: boolean;
}) {
  return (
    <div className="col" style={{ gap: 6 }}>
      <div className="row" style={{ gap: 8 }}>
        <span style={{ fontWeight: 500, fontSize: 13 }}>{title}</span>
        <span className="muted" style={{ fontSize: 11.5 }}>
          {hint}
        </span>
        <div className="grow" />
        {onClear && items.length > 0 && (
          <button className="btn sm ghost" onClick={onClear} title="清空这一级的全部记忆">
            清空
          </button>
        )}
      </div>
      {items.length === 0 ? (
        <div className="muted" style={{ fontSize: 12, padding: "4px 2px" }}>
          {empty ?? "还没有"}
        </div>
      ) : (
        items.map((m) => {
          const kind = kindsOf(kinds, m.kind);
          return (
            <div key={`${m.scope}-${m.topicSlug ?? ""}-${m.id}`} className="card-box mem-row">
              <div className="row" style={{ gap: 8, alignItems: "flex-start" }}>
                <span
                  className="tag"
                  style={{ color: KIND_COLOR[m.kind], borderColor: "currentColor", flex: "none" }}
                  title={kind?.hint}
                >
                  {kind?.label ?? m.kind}
                </span>
                <span className="grow" style={{ fontSize: 13, lineHeight: 1.6 }}>
                  {m.pinned && <Icon name="target" size={12} style={{ marginRight: 4, color: "var(--accent)" }} />}
                  {m.content}
                </span>
                {!readOnly && (
                  <>
                    <button
                      className="icon-btn"
                      title={m.pinned ? "取消钉住" : "钉住（永远优先注入）"}
                      style={m.pinned ? { color: "var(--accent)" } : { opacity: 0.55 }}
                      onClick={() => onPin?.(m)}
                    >
                      <Icon name="target" size={13} />
                    </button>
                    <button className="icon-btn" title="编辑" onClick={() => onEdit?.(m)}>
                      <Icon name="pencil" size={13} />
                    </button>
                    <button className="icon-btn" title="删除" onClick={() => onDelete?.(m)}>
                      <Icon name="trash" size={13} />
                    </button>
                  </>
                )}
              </div>
              {m.note && (
                <div className="muted" style={{ fontSize: 12 }}>
                  备注：{m.note}
                </div>
              )}
              <div className="muted mono" style={{ fontSize: 10.5 }}>
                {m.source ? `来源：${m.source}｜` : ""}
                {shortDate(m.updatedAt ?? m.createdAt)} 记录
                {m.useCount > 0 ? `｜用过 ${m.useCount} 次（最近 ${shortDate(m.lastUsed)}）` : "｜还没被用到过"}
              </div>
            </div>
          );
        })
      )}
    </div>
  );
}

/** 新增 / 编辑一条记忆的表单 */
function MemoryForm({
  kinds,
  initial,
  hasTopic,
  busy,
  onCancel,
  onSubmit,
}: {
  kinds: MemoryKindInfo[];
  initial: MemoryItem | null;
  hasTopic: boolean;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (input: { id?: string | null; kind: MemoryKind; content: string; note?: string; source?: string; pinned?: boolean }, scope: MemoryScope) => void;
}) {
  const [kind, setKind] = useState<MemoryKind>(initial?.kind ?? "fact");
  const [content, setContent] = useState(initial?.content ?? "");
  const [note, setNote] = useState(initial?.note ?? "");
  const [source, setSource] = useState(initial?.source ?? "");
  const [pinned, setPinned] = useState(initial?.pinned ?? false);
  const [scope, setScope] = useState<MemoryScope>(initial?.scope ?? "topic");

  // 没有打开主题时只能写全局（不然没地方落）
  const effectiveScope: MemoryScope = hasTopic ? scope : "global";
  const current = kindsOf(kinds, kind);

  return (
    <div className="card-box" style={{ background: "var(--bg-sub)" }}>
      <div className="grid-2">
        <Field label="分类" hint={current?.hint}>
          <select className="select" value={kind} onChange={(e) => setKind(e.target.value as MemoryKind)}>
            {kinds.map((k) => (
              <option key={k.id} value={k.id}>
                {k.label}
              </option>
            ))}
          </select>
        </Field>
        <Field
          label="写到哪一级"
          hint={effectiveScope === "global" ? "所有主题都带上" : "只在当前主题（含它的子主题）生效"}
        >
          <select
            className="select"
            value={effectiveScope}
            disabled={!hasTopic}
            onChange={(e) => setScope(e.target.value as MemoryScope)}
          >
            <option value="topic" disabled={!hasTopic}>
              本主题
            </option>
            <option value="global">全局</option>
          </select>
        </Field>
      </div>
      <Field label="内容" hint="一句话说清，要能脱离上下文读懂。例如「他把特征值和特征向量搞混」">
        <textarea
          className="textarea"
          rows={2}
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="他习惯先看具体例子再接受定义"
        />
      </Field>
      <div className="grid-2">
        <Field label="备注（可选）">
          <input className="input" value={note} onChange={(e) => setNote(e.target.value)} placeholder="当时的上下文、证据" />
        </Field>
        <Field label="来源（可选）">
          <input className="input" value={source} onChange={(e) => setSource(e.target.value)} placeholder="2024-05 复习时的对话" />
        </Field>
      </div>
      <div className="row">
        <Switch checked={pinned} onChange={setPinned} label="钉住（永远优先注入）" />
        <div className="grow" />
        <button className="btn" onClick={onCancel}>
          取消
        </button>
        <button
          className="btn primary"
          disabled={!content.trim() || busy}
          onClick={() =>
            onSubmit(
              {
                id: initial?.id ?? null,
                kind,
                content: content.trim(),
                note: note.trim(),
                source: source.trim(),
                pinned,
              },
              effectiveScope,
            )
          }
        >
          {initial ? "保存" : "记下"}
        </button>
      </div>
    </div>
  );
}

