// 侧栏的「技能」与「MCP」两个面板。
//
// 两个作用域：
// - **全局**：写进 config.json，所有主题都能用
// - **本主题**：只属于当前主题（技能放在主题的 .hub/skills/，MCP 记在 topic.json）
//
// 开关是分开的：全局定义的东西可以在某个主题里单独关掉——
// 比如「文献检索」技能只在写论文的主题里开，别的时候不占模型的上下文。

import { useCallback, useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { SCOPE_LABEL, type McpEntryView, type McpOverview, type Scope, type SkillEntry, type SkillDirs, type SkillsOverview } from "../lib/types";
import { useApp } from "../store/app";
import { Field, Icon, Modal, Segmented, Spinner, Switch } from "./ui";

// ---------------------------------------------------------------- 技能

export function SkillsDialog({ onClose }: { onClose: () => void }) {
  const slug = useApp((s) => s.topic?.slug ?? null);
  const toast = useApp((s) => s.toast);
  const [data, setData] = useState<SkillsOverview | null>(null);
  const [dirs, setDirs] = useState<SkillDirs | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [form, setForm] = useState({ id: "", name: "", description: "", body: "", scope: "global" as Scope });

  // 会被写进系统提示词的技能数（面板上要能一眼看到「agent 到底拿到几个」）
  const onCount = data ? [...data.global, ...data.topic].filter((s) => s.enabled).length : 0;

  const refresh = useCallback(async () => {
    try {
      setData(await api.skillsOverview(slug));
    } catch (e) {
      toast("error", errText(e));
    }
  }, [slug, toast]);

  useEffect(() => {
    void refresh();
    void api.skillsDirs(slug).then(setDirs).catch(() => setDirs(null));
  }, [refresh, slug]);

  async function toggle(entry: SkillEntry, enabled: boolean, scope: Scope) {
    try {
      await api.skillSetEnabled(entry.id, scope, enabled, slug);
      await refresh();
    } catch (e) {
      toast("error", errText(e));
    }
  }

  return (
    <Modal
      title="技能"
      icon="puzzle"
      wide
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            {dirs ? `全局技能目录：${dirs.workspace}` : ""}
          </span>
          <button className="btn" onClick={() => void api.skillsReload().then((n) => (toast("success", `重新扫描到 ${n} 个全局技能`), void refresh()))}>
            <Icon name="refresh" size={13} /> 重新扫描
          </button>
          <button className="btn primary" onClick={() => setCreating((v) => !v)}>
            <Icon name="plus" size={13} /> 新建技能
          </button>
        </>
      }
    >
      {creating && (
        <div className="card-box" style={{ background: "var(--bg-sub)" }}>
          <div className="grid-2">
            <Field label="目录名" hint="英文/数字，作为技能标识">
              <input className="input mono" value={form.id} onChange={(e) => setForm({ ...form, id: e.target.value })} placeholder="paper-reading" />
            </Field>
            <Field label="显示名">
              <input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="论文精读" />
            </Field>
          </div>
          <Field label="什么时候用它" hint="模型靠这句判断要不要加载，写具体些">
            <input className="input" value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} placeholder="需要逐段拆解一篇论文时使用" />
          </Field>
          <Field label="正文（Markdown）" hint="写清步骤；可以提到同目录下的附加文件">
            <textarea className="textarea" rows={6} value={form.body} onChange={(e) => setForm({ ...form, body: e.target.value })} placeholder={"## 步骤\n1. 先复述研究问题\n2. …"} />
          </Field>
          <div className="row">
            <Segmented
              value={form.scope}
              onChange={(v) => setForm({ ...form, scope: v })}
              options={[
                { id: "global", label: "全局" },
                { id: "topic", label: "仅本主题" },
              ]}
            />
            <button
              className="btn primary"
              disabled={!form.id.trim() || !form.name.trim() || (form.scope === "topic" && !slug)}
              onClick={async () => {
                try {
                  await api.skillSave(
                    { id: form.id, name: form.name, description: form.description, body: form.body },
                    form.scope,
                    slug,
                  );
                  setCreating(false);
                  setForm({ id: "", name: "", description: "", body: "", scope: "global" });
                  await refresh();
                  toast("success", "技能已保存");
                } catch (e) {
                  toast("error", errText(e));
                }
              }}
            >
              保存
            </button>
            <button className="btn" onClick={() => setCreating(false)}>
              取消
            </button>
          </div>
        </div>
      )}

      {data ? (
        <>
          <div className="row">
            <Switch
              checked={data.enabled}
              onChange={async (v) => {
                await api.skillsSetEnabled(v);
                await refresh();
              }}
              label="启用技能系统（总开关）"
            />
            <div className="grow" />
            <span className="muted" style={{ fontSize: 11.5 }}>
              写进系统提示词的只有开着的那些（共 {data.global.length + data.topic.length} 个，
              当前开着 {onCount} 个）。提示词里只列名字与适用场景，模型需要时才读正文。
            </span>
          </div>

          <div className="row" style={{ gap: 6 }}>
            <span className="muted" style={{ fontSize: 11.5 }}>
              全局技能一键开关：
            </span>
            <button
              className="btn sm"
              onClick={async () => {
                const n = await api.skillsSetAll(false);
                await refresh();
                toast("info", `已关掉 ${n} 个技能（不再写进提示词）`);
              }}
            >
              全部关掉
            </button>
            <button
              className="btn sm"
              onClick={async () => {
                const n = await api.skillsSetAll(true);
                await refresh();
                toast("info", `已打开 ${n} 个技能`);
              }}
            >
              全部打开
            </button>
            <span className="muted" style={{ fontSize: 11.5 }}>
              （按当前扫到的技能来，以后新加的也会被这两个按钮管到）
            </span>
          </div>

          <SkillGroup
            title={`全局（${data.global.length}）`}
            hint="左边开关控制全局；右边开关只影响当前主题"
            entries={data.global}
            expanded={expanded}
            setExpanded={setExpanded}
            onToggle={toggle}
            showGlobalToggle
            onOpenDir={(dir) => void api.revealInExplorer().then(() => toast("info", dir)).catch(() => {})}
          />

          <SkillGroup
            title={data.topicName ? `本主题：${data.topicName}（${data.topic.length}）` : "本主题"}
            hint={
              data.topicName
                ? `只在这个主题里生效。目录：${dirs?.topic ?? "（主题/.hub/skills）"}`
                : "先打开一个主题，才能添加只属于它的技能"
            }
            entries={data.topic}
            expanded={expanded}
            setExpanded={setExpanded}
            onToggle={toggle}
            onOpenDir={(dir) => void api.revealInExplorer().then(() => toast("info", dir)).catch(() => {})}
            empty={data.topicName ? "这个主题还没有自己的技能——也可以用上边全局技能的「本主题」开关" : "（未打开主题）"}
          />
        </>
      ) : (
        <div className="row" style={{ justifyContent: "center", padding: 20 }}>
          <Spinner />
        </div>
      )}
    </Modal>
  );
}

/** 技能没生效的原因 → 界面文案（后端算，界面只负责显示） */
const LABELS: Record<string, string> = {
  topic: "本主题已关",
  parent: "父主题已关",
  global: "全局已关",
  总开关: "总开关已关",
};

function SkillGroup({
  title,
  hint,
  entries,
  expanded,
  setExpanded,
  onToggle,
  empty,
  showGlobalToggle
}: {
  title: string;
  hint: string;
  entries: SkillEntry[];
  expanded: string | null;
  setExpanded: (v: string | null) => void;
  /** 开关回调：传 scope 决定改哪一级 */
  onToggle: (e: SkillEntry, enabled: boolean, scope: Scope) => void;
  onOpenDir?: (dir: string) => void;
  empty?: string;
  /** 全局组里也显示「本主题」开关（关掉某条全局技能在本主题里的使用） */
  showGlobalToggle?: boolean;
}) {
  const topicOpen = !!useApp((st) => st.topic);
  return (
    <div className="col" style={{ gap: 6 }}>
      <div className="row" style={{ gap: 8 }}>
        <span style={{ fontWeight: 500, fontSize: 13 }}>{title}</span>
        <span className="muted" style={{ fontSize: 11.5 }}>
          {hint}
        </span>
      </div>
      {entries.length === 0 ? (
        <div className="muted" style={{ fontSize: 12, padding: "4px 2px" }}>
          {empty ?? "还没有"}
        </div>
      ) : (
        entries.map((s) => (
          <div key={`${s.scope}-${s.id}`} className="card-box" style={{ padding: 12, gap: 6 }}>
            <div className="row" style={{ gap: 8 }}>
              <Icon name="puzzle" size={13} style={{ opacity: s.enabled ? 1 : 0.4 }} />
              <span style={{ fontWeight: 500 }}>{s.name}</span>
              <span className="tag">{s.source}</span>
              {s.disabledBy && (
                <span className="tag" style={{ color: "var(--warn)" }}>
                  {LABELS[s.disabledBy] ?? s.disabledBy}
                </span>
              )}
              <div className="grow" />
              <button className="btn sm ghost" onClick={() => setExpanded(expanded === s.id ? null : s.id)}>
                {expanded === s.id ? "收起" : "看正文"}
              </button>
              <ScopeSwitch
                scope="global"
                checked={!["global", "总开关", "default"].includes(s.disabledBy ?? "")}
                disabled={s.disabledBy === "总开关"}
                onChange={(v) => onToggle(s, v, "global")}
              />
              {showGlobalToggle && (
                <ScopeSwitch
                  scope="topic"
                  checked={!["topic", "parent"].includes(s.disabledBy ?? "")}
                  disabled={!topicOpen}
                  onChange={(v) => onToggle(s, v, "topic")}
                />
              )}
            </div>
            <div className="sub" style={{ fontSize: 12.5 }}>
              {s.description}
            </div>
            {expanded === s.id && (
              <div className="muted mono" style={{ fontSize: 11 }}>
                目录：{s.dir}
                {s.files.length > 0 && `　附带文件 ${s.files.length} 个`}
              </div>
            )}
          </div>
        ))
      )}
    </div>
  );
}

/** 一个带作用域标签的开关：一眼看出改的是哪一级 */
function ScopeSwitch({
  scope,
  checked,
  disabled,
  onChange,
}: {
  scope: Scope;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <span
      className="scope-switch"
      title={scope === "global" ? "全局：所有主题都受影响" : "只影响当前主题"}
      style={disabled ? { opacity: 0.45 } : undefined}
    >
      <span className="scope-label">{SCOPE_LABEL[scope]}</span>
      <Switch checked={checked} onChange={disabled ? () => {} : onChange} />
    </span>
  );
}

// ---------------------------------------------------------------- MCP

export function McpDialog({ onClose }: { onClose: () => void }) {
  const slug = useApp((s) => s.topic?.slug ?? null);
  const toast = useApp((s) => s.toast);
  const [data, setData] = useState<McpOverview | null>(null);
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [form, setForm] = useState({ name: "", command: "", args: "", env: "", scope: "global" as Scope });

  const refresh = useCallback(async () => {
    try {
      setData(await api.mcpOverview(slug));
    } catch (e) {
      toast("error", errText(e));
    }
  }, [slug, toast]);

  useEffect(() => {
    void refresh();
    // 启动时的连接是后台任务，晚一点再拉一次状态
    const t = setTimeout(() => void refresh(), 2500);
    return () => clearTimeout(t);
  }, [refresh]);

  async function setEnabled(entry: McpEntryView, enabled: boolean, scope: Scope) {
    setBusy(true);
    try {
      await api.mcpSetEnabled(entry.name, enabled, scope, slug);
      await refresh();
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    const args = form.args.split(/\s+/).map((a) => a.trim()).filter(Boolean);
    const env: Record<string, string> = {};
    for (const line of form.env.split("\n")) {
      const [k, ...rest] = line.split("=");
      if (k && k.trim() && rest.length > 0) env[k.trim()] = rest.join("=").trim();
    }
    setBusy(true);
    try {
      await api.mcpUpsert(
        { name: form.name.trim(), command: form.command.trim(), args, env, enabled: true },
        form.scope,
        slug,
      );
      setAdding(false);
      setForm({ name: "", command: "", args: "", env: "", scope: "global" });
      await refresh();
      toast("success", "已保存并尝试连接");
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title="MCP 服务器"
      icon="plug"
      wide
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            连上后它提供的工具会以 <code className="mono">mcp__服务器__工具</code> 出现在 agent 的工具列表里
          </span>
          <button
            className="btn"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api.mcpReload();
                await refresh();
                toast("success", "已重连");
              } catch (e) {
                toast("error", errText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? <Spinner /> : <Icon name="refresh" size={13} />} 重连全部
          </button>
          <button className="btn primary" onClick={() => setAdding((v) => !v)}>
            <Icon name="plus" size={13} /> 添加服务器
          </button>
        </>
      }
    >
      {adding && (
        <div className="card-box" style={{ background: "var(--bg-sub)" }}>
          <div className="grid-2">
            <Field label="名字" hint="工具名前缀用它，例如 filesystem">
              <input className="input mono" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="zotero" />
            </Field>
            <Field label="启动命令" hint="npx / uvx / python / 可执行文件">
              <input className="input mono" value={form.command} onChange={(e) => setForm({ ...form, command: e.target.value })} placeholder="npx" />
            </Field>
          </div>
          <Field label="参数（空格分隔）">
            <input
              className="input mono"
              value={form.args}
              onChange={(e) => setForm({ ...form, args: e.target.value })}
              placeholder="-y @modelcontextprotocol/server-filesystem D:\文献"
            />
          </Field>
          <Field label="环境变量（每行 KEY=VALUE）">
            <textarea className="textarea" rows={2} value={form.env} onChange={(e) => setForm({ ...form, env: e.target.value })} />
          </Field>
          <div className="row">
            <Segmented
              value={form.scope}
              onChange={(v) => setForm({ ...form, scope: v })}
              options={[
                { id: "global", label: "全局" },
                { id: "topic", label: "仅本主题" },
              ]}
            />
            <button className="btn primary" disabled={!form.name.trim() || !form.command.trim() || busy || (form.scope === "topic" && !slug)} onClick={() => void save()}>
              保存并连接
            </button>
            <button className="btn" onClick={() => setAdding(false)}>
              取消
            </button>
          </div>
        </div>
      )}

      {data ? (
        <>
          <div className="row" style={{ gap: 6 }}>
            <span className="muted" style={{ fontSize: 11.5 }}>
              全局服务器一键开关：
            </span>
            <button
              className="btn sm"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                try {
                  await api.mcpSetAll(false);
                  await refresh();
                  toast("info", "已关掉全部 MCP 服务器（工具不再暴露给 agent）");
                } catch (e) {
                  toast("error", errText(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              全部关掉
            </button>
            <button
              className="btn sm"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                try {
                  await api.mcpSetAll(true);
                  await refresh();
                  toast("info", "已打开全部 MCP 服务器");
                } catch (e) {
                  toast("error", errText(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              全部打开
            </button>
            <span className="muted" style={{ fontSize: 11.5 }}>
              （按当前配置里的服务器来，以后新加的也一起管）
            </span>
          </div>
          <McpGroup
            title={`全局（${data.global.length}）`}
            hint="左边开关控制全局；右边开关只影响当前主题"
            entries={data.global}
            busy={busy}
            showGlobalToggle
            onToggle={setEnabled}
            onDelete={async (e) => {
              setBusy(true);
              try {
                await api.mcpDelete(e.name, e.scope, slug);
                await refresh();
              } catch (err) {
                toast("error", errText(err));
              } finally {
                setBusy(false);
              }
            }}
          />
          <McpGroup
            title={data.topicName ? `本主题：${data.topicName}（${data.topic.length}）` : "本主题"}
            hint={data.topicName ? "只在这个主题里连接的服务器" : "先打开一个主题，才能添加它专用的服务器"}
            entries={data.topic}
            busy={busy}
            onToggle={setEnabled}
            onDelete={async (e) => {
              setBusy(true);
              try {
                await api.mcpDelete(e.name, e.scope, slug);
                await refresh();
              } catch (err) {
                toast("error", errText(err));
              } finally {
                setBusy(false);
              }
            }}
            empty={data.topicName ? "这个主题还没有专用服务器" : "（未打开主题）"}
          />
        </>
      ) : (
        <div className="row" style={{ justifyContent: "center", padding: 20 }}>
          <Spinner />
        </div>
      )}
    </Modal>
  );
}

function McpGroup({
  title,
  hint,
  entries,
  busy,
  onToggle,
  onDelete,
  empty,
  showGlobalToggle,
}: {
  title: string;
  hint: string;
  entries: McpEntryView[];
  busy: boolean;
  onToggle: (e: McpEntryView, enabled: boolean, scope: Scope) => void;
  onDelete: (e: McpEntryView) => void;
  empty?: string;
  showGlobalToggle?: boolean;
}) {
  const topicOpen = !!useApp((st) => st.topic);
  return (
    <div className="col" style={{ gap: 6 }}>
      <div className="row" style={{ gap: 8 }}>
        <span style={{ fontWeight: 500, fontSize: 13 }}>{title}</span>
        <span className="muted" style={{ fontSize: 11.5 }}>
          {hint}
        </span>
      </div>
      {entries.length === 0 ? (
        <div className="muted" style={{ fontSize: 12, padding: "4px 2px" }}>
          {empty ?? "还没有"}
        </div>
      ) : (
        entries.map((s) => (
          <div key={`${s.scope}-${s.name}`} className="card-box" style={{ padding: 12, gap: 6 }}>
            <div className="row" style={{ gap: 8 }}>
              <Icon
                name={s.connected ? "check" : "alert"}
                size={13}
                style={{ color: s.connected ? "var(--ok)" : "var(--danger)" }}
              />
              <span style={{ fontWeight: 500 }}>{s.name}</span>
              {s.connected ? (
                <span className="tag ok">{s.toolCount} 个工具</span>
              ) : (
                <span className="tag">{s.enabled ? "未连接" : "已停用"}</span>
              )}
              {!s.enabledHere && <span className="tag" style={{ color: "var(--warn)" }}>本主题已关</span>}
              <div className="grow" />
              <ScopeSwitch
                scope="global"
                checked={s.enabled}
                onChange={(v) => onToggle(s, v, "global")}
              />
              {showGlobalToggle && (
                <ScopeSwitch
                  scope="topic"
                  checked={s.enabledHere}
                  disabled={!topicOpen}
                  onChange={(v) => onToggle(s, v, "topic")}
                />
              )}
              <button className="icon-btn" title="删除" disabled={busy} onClick={() => onDelete(s)}>
                <Icon name="trash" size={13} />
              </button>
            </div>
            <div className="mono muted" style={{ fontSize: 11 }}>
              {s.command}
            </div>
            {s.error && <div style={{ fontSize: 12, color: "var(--danger)" }}>{s.error}</div>}
            {s.connected && s.tools.length > 0 && (
              <div className="row wrap" style={{ gap: 4 }}>
                {s.tools.map((t) => (
                  <span key={t} className="tag mono" style={{ fontSize: 10.5 }}>
                    {t}
                  </span>
                ))}
              </div>
            )}
          </div>
        ))
      )}
    </div>
  );
}
