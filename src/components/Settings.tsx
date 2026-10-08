// 设置页：用户 / 外观（含自定义主题）/ 工作区 / 模型档案 / Agent 行为 / 内置浏览器 / 关于。

import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import {
  PERMISSION_LABEL,
  STYLE_LABEL,
  type CustomTheme,
  type PermissionMode,
  type ProfileInput,
  type ReasoningStyle,
  type ProviderKind,
} from "../lib/types";
import { useApp } from "../store/app";
import { Field, Icon, Modal, Segmented, Spinner, Switch } from "./ui";


export function Settings() {
  const config = useApp((s) => s.config);
  const patchConfig = useApp((s) => s.patchConfig);
  const setView = useApp((s) => s.setView);
  const toast = useApp((s) => s.toast);
  const tools = useApp((s) => s.tools);
  const version = useApp((s) => s.version);

  const [ws, setWs] = useState<{ root: string; topicCount: number; diskUsageText: string } | null>(null);
  const [name, setName] = useState(config?.userName ?? "");
  const [toolsOpen, setToolsOpen] = useState(false);

  useEffect(() => {
    void api.workspaceInfo().then(setWs).catch(() => setWs(null));
  }, []);

  if (!config) return null;

  const agent = config.agent;

  return (
    <div className="settings">
      <div className="settings-inner">
        <div className="row">
          <h2>
            <Icon name="settings" size={16} /> 设置
          </h2>
          <div className="grow" />
          <button className="btn sm" onClick={() => setView("home")}>
            <Icon name="arrow-left" size={13} /> 返回
          </button>
        </div>

        {/* ---------------- 用户 ---------------- */}
        <section className="col">
          <h2>
            用户 <span className="sub">主界面问候语里的名字</span>
          </h2>
          <div className="card-box">
            <div className="row">
              <input
                className="input"
                value={name}
                placeholder="你的名字"
                onChange={(e) => setName(e.target.value)}
                onBlur={() => name.trim() && name !== config.userName && void patchConfig({ userName: name.trim() })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && name.trim()) void patchConfig({ userName: name.trim() });
                }}
              />
              <button
                className="btn"
                disabled={!name.trim() || name === config.userName}
                onClick={() => void patchConfig({ userName: name.trim() })}
              >
                保存
              </button>
            </div>
          </div>
        </section>

        {/* ---------------- 外观 ---------------- */}
        <AppearanceSection />

        {/* ---------------- 工作区 ---------------- */}
        <section className="col">
          <h2>
            工作区 <span className="sub">所有主题目录的根</span>
          </h2>
          <div className="card-box">
            <div className="row">
              <input className="input mono" value={config.workspaceRoot} onChange={(e) => void patchConfig({ workspaceRoot: e.target.value })} />
              <button
                className="btn"
                onClick={async () => {
                  const { open } = await import("@tauri-apps/plugin-dialog");
                  const dir = await open({ directory: true, title: "选择工作区目录" });
                  if (typeof dir === "string") void patchConfig({ workspaceRoot: dir });
                }}
              >
                <Icon name="folder" size={13} /> 选择
              </button>
              <button
                className="btn"
                onClick={() => void api.revealInExplorer().catch((e) => toast("error", errText(e)))}
              >
                打开
              </button>
            </div>
            {ws && (
              <div className="muted" style={{ fontSize: 12 }}>
                当前：{ws.topicCount} 个主题 · 占用 {ws.diskUsageText}
                <br />
                每个主题就是一个目录，里面是纯 Markdown / PDF / JSON，可以直接用编辑器改。
              </div>
            )}
          </div>
        </section>

        {/* ---------------- 模型档案 ---------------- */}
        <ProfilesSection />

        {/* ---------------- Agent ---------------- */}
        <section className="col">
          <h2>
            Agent 行为 <span className="sub">决定它多主动、能碰什么</span>
          </h2>
          <div className="card-box">
            <Field label="工具权限" hint="读操作永远直接执行；这里调的是「写」和「删」的尺度">
              <div className="row wrap">
                {(["ask", "auto_edit", "full"] as PermissionMode[]).map((m) => (
                  <button
                    key={m}
                    className={"chip" + (agent.permissionMode === m ? " primary" : "")}
                    onClick={() => void patchConfig({ permissionMode: m })}
                  >
                    {PERMISSION_LABEL[m]}
                  </button>
                ))}
              </div>
            </Field>

            <div className="grid-2">
              <Field label="单轮最多工具往返次数" hint="防止 agent 反复调用工具停不下来">
                <input
                  className="input"
                  type="number"
                  min={1}
                  max={100}
                  value={agent.maxIterations}
                  onChange={(e) => void patchConfig({ maxIterations: Number(e.target.value) })}
                />
              </Field>
              <Field label="单次请求超时（秒）">
                <input
                  className="input"
                  type="number"
                  min={30}
                  max={1800}
                  value={agent.requestTimeoutSecs}
                  onChange={(e) => void patchConfig({ requestTimeoutSecs: Number(e.target.value) })}
                />
              </Field>
            </div>

            <Field label="上下文预算（字符）" hint="发给模型的对话历史上限，超出会从最早的完整轮次开始丢弃">
              <input
                className="input"
                type="number"
                min={2000}
                max={500000}
                step={1000}
                value={agent.contextBudgetChars}
                onChange={(e) => void patchConfig({ contextBudgetChars: Number(e.target.value) })}
              />
            </Field>

            <Switch
              checked={agent.allowWeb}
              onChange={(v) => void patchConfig({ allowWeb: v })}
              label="允许 agent 联网（web_fetch / web_search）"
            />

            <Field label="额外系统指令" hint="追加到系统提示词末尾，优先级最高。例如「多举数学例子」「解释时先用中文再给英文术语」">
              <textarea
                className="textarea"
                rows={4}
                value={agent.systemPromptExtra}
                placeholder="想让 agent 怎么带你？写在这里。"
                onChange={(e) => void patchConfig({ systemPromptExtra: e.target.value })}
              />
            </Field>
          </div>
        </section>

        {/* ---------------- 内置浏览器 ---------------- */}
        <section className="col">
          <h2>
            内置浏览器 <span className="sub">agent 和你共用同一个浏览窗口</span>
          </h2>
          <div className="card-box">
            <div className="grid-2">
              <Field label="主页">
                <input
                  className="input"
                  value={config.viewer.homeUrl}
                  onChange={(e) => void patchConfig({ homeUrl: e.target.value })}
                />
              </Field>
              <Field label="联网搜索默认引擎">
                <select
                  className="select"
                  value={config.viewer.searchEngine}
                  onChange={(e) => void patchConfig({ searchEngine: e.target.value })}
                >
                  <option value="duckduckgo">DuckDuckGo</option>
                  <option value="bing">Bing</option>
                </select>
              </Field>
            </div>
            <div className="muted" style={{ fontSize: 12 }}>
              PDF 用 pdf.js 渲染（agent 能读每一页的文字、能翻页）；
              网页用内嵌窗口打开，遇到禁止嵌入的站点可以切「阅读模式」看提取后的正文。
            </div>
          </div>
        </section>

        {/* ---------------- 沙箱 ---------------- */}
        <section className="col">
          <h2>
            工作区沙箱 <span className="sub">agent 默认只能碰工作区里的文件</span>
          </h2>
          <div className="card-box">
            <Switch
              checked={agent.sandbox}
              onChange={(v) => void patchConfig({ sandbox: v })}
              label="开启沙箱：访问工作区之外的文件前必须向你申请"
            />
            <div className="muted" style={{ fontSize: 12 }}>
              关掉之后 agent 可以自由读写本机任意路径，只建议在完全信任的场景下使用。
            </div>
            <div className="col" style={{ gap: 6 }}>
              <div style={{ fontSize: 12.5, color: "var(--text-sub)" }}>已授权的目录</div>
              {agent.approvedRoots.length === 0 ? (
                <div className="muted" style={{ fontSize: 12 }}>
                  还没有。agent 想读工作区外的讲义时会弹窗申请，你同意后目录会出现在这里。
                </div>
              ) : (
                agent.approvedRoots.map((r) => (
                  <div key={r} className="row" style={{ gap: 8 }}>
                    <code className="mono grow" style={{ fontSize: 11.5 }}>
                      {r}
                    </code>
                    <button className="btn sm" onClick={() => void patchConfig({ revokeRoot: r })}>
                      撤销
                    </button>
                  </div>
                ))
              )}
              {agent.approvedRoots.length > 0 && (
                <button className="btn sm" style={{ alignSelf: "flex-start" }} onClick={() => void patchConfig({ clearApprovedRoots: true })}>
                  全部撤销
                </button>
              )}
            </div>
          </div>
        </section>

        {/* ---------------- 长期记忆 ---------------- */}
        <section className="col">
          <h2>
            长期记忆 <span className="sub">记住你的特点与「以后要注意的地方」</span>
          </h2>
          <div className="card-box">
            <Switch
              checked={agent.memoryEnabled}
              onChange={(v) => void patchConfig({ memoryEnabled: v })}
              label="启用长期记忆：每轮对话都把相关记忆交给 agent"
            />
            <div className="muted" style={{ fontSize: 12 }}>
              agent 会在对话里主动记下值得长期保留的信息（你的基础、偏好、反复出错的点、还缺的前置），
              记在 <code className="mono">.hub/memory/memories.jsonl</code> 里——
              普通文本，可以直接用编辑器改。关掉开关只是不再注入提示词，已有记忆仍可查看。
            </div>
            <div className="row">
              <span className="muted" style={{ fontSize: 12 }}>
                查看、编辑、钉住或删除记忆：侧栏顶部的「记忆」。
              </span>
            </div>
          </div>
        </section>

        {/* ---------------- 技能与 MCP 的入口 ---------------- */}
        <section className="col">
          <h2>
            技能与 MCP <span className="sub">开关在侧栏，造新的去工坊</span>
          </h2>
          <div className="card-box">
            <div className="sub" style={{ fontSize: 12.5 }}>
              侧栏顶部的「技能」与「MCP 服务器」两个入口里，可以按**全局**或**本主题**分别开关。
              全局的写进配置文件（所有主题可见），本主题的只影响当前主题。
            </div>
            <div className="row">
              <button className="btn sm" onClick={() => void useApp.getState().openStudio()}>
                <Icon name="hammer" size={13} /> 去工坊造一个技能 / MCP 服务器
              </button>
            </div>
          </div>
        </section>

        {/* ---------------- 关于 ---------------- */}
        <section className="col">
          <h2>
            关于 <span className="sub">学习中枢 v{version}</span>
          </h2>
          <div className="card-box">
            <div className="row">
              <span className="sub" style={{ fontSize: 12.5 }}>
                Tauri + Rust 后端，React 前端。数据全部在你自己的磁盘上，没有账号、没有云端。
              </span>
            </div>
            <div className="row">
              <button className="btn sm" onClick={() => setToolsOpen(true)}>
                <Icon name="sparkle" size={13} /> 查看 agent 的 {tools.length} 个工具
              </button>
              <button
                className="btn sm"
                onClick={() =>
                  void api
                    .promptPreview(useApp.getState().topic?.slug ?? null)
                    .then((t) => {
                      setPromptTextForView(t);
                    })
                    .catch((e) => toast("error", errText(e)))
                }
              >
                <Icon name="eye" size={13} /> 预览系统提示词
              </button>
            </div>
          </div>
        </section>
      </div>

      {toolsOpen && (
        <Modal title={`内置工具（${tools.length}）`} icon="sparkle" wide onClose={() => setToolsOpen(false)}>
          <div className="col" style={{ gap: 10 }}>
            {tools.map((t) => (
              <div key={t.name} className="card-box" style={{ gap: 6, padding: 12 }}>
                <div className="row">
                  <span className="mono" style={{ fontWeight: 600 }}>
                    {t.name}
                  </span>
                </div>
                <div className="sub" style={{ fontSize: 12.5, whiteSpace: "pre-wrap" }}>
                  {t.description}
                </div>
              </div>
            ))}
          </div>
        </Modal>
      )}

      <PromptPreviewModal />
    </div>
  );
}

// 供设置页触发的提示词预览
let promptSetter: ((t: string) => void) | null = null;
function setPromptTextForView(t: string) {
  promptSetter?.(t);
}

function PromptPreviewModal() {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    promptSetter = setText;
    return () => {
      promptSetter = null;
    };
  }, []);
  if (text === null) return null;
  return (
    <Modal title="系统提示词（agent 眼中的世界）" icon="eye" wide onClose={() => setText(null)}>
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
        {text}
      </pre>
    </Modal>
  );
}

// ---------------------------------------------------------------- 外观

/**
 * 外观：内置三档配色 + 用户自己写的主题。
 *
 * 自定义主题就是「一堆 CSS 变量的覆盖值」，所以这里给的编辑方式是一份 JSON 文本——
 * 与其做一个只能改颜色的表单（改不了字体、圆角、代码高亮），不如让用户直接改文件内容，
 * 同时提供「从当前配色复制一份」当起点，省得他去猜变量名。
 */
function AppearanceSection() {
  const config = useApp((s) => s.config)!;
  const themes = useApp((s) => s.themes);
  const loadThemes = useApp((s) => s.loadThemes);
  const setCustomTheme = useApp((s) => s.setCustomTheme);
  const deleteTheme = useApp((s) => s.deleteTheme);
  const toast = useApp((s) => s.toast);
  const [varsOpen, setVarsOpen] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    void loadThemes();
  }, [loadThemes]);

  const activeId = config.appearance.customTheme ?? null;
  const active = themes?.applied ?? null;
  const list = themes?.themes ?? [];
  const darkBase = (document.documentElement.dataset.theme ?? "light") === "dark";

  const openDir = async (user: boolean) => {
    try {
      const p = await api.themeOpenDir(user);
      toast("info", `已打开：${p}`);
    } catch (e) {
      toast("error", errText(e));
    }
  };

  return (
    <section className="col">
      <h2>
        外观 <span className="sub">明亮 / 深色 / 跟随系统，或者自己写一套配色</span>
      </h2>
      <div className="card-box">
        <div className="row wrap">
          <Segmented
            value={config.appearance.theme}
            onChange={(v) => void useApp.getState().setTheme(v)}
            options={[
              { id: "system", label: "跟随系统" },
              { id: "light", label: "明亮" },
              { id: "dark", label: "深色" },
            ]}
          />
          <span className="muted" style={{ fontSize: 12 }}>
            强制明亮适合长时间读讲义；侧栏底部也有一个快捷切换按钮
          </span>
        </div>

        {active && (
          <div className="muted" style={{ fontSize: 12 }}>
            当前用的是自定义主题「{active.name}」（自带{active.base === "dark" ? "深色" : "明亮"}底色）。
            它没覆盖的颜色跟随内置的{active.base === "dark" ? "深色" : "明亮"}配色；点上面那三档会退回内置配色。
          </div>
        )}
        {activeId && !active && (
          <div style={{ fontSize: 12, color: "var(--warn)" }}>
            配置里记着的主题「{activeId}」现在找不到（文件被删了或读不了），已经退回内置配色。
          </div>
        )}

        <div className="theme-list">
          {list.length === 0 ? (
            <div className="muted" style={{ fontSize: 12 }}>
              还没有自定义主题。可以「从当前配色新建一份」，也可以把自己写的 JSON 放进主题目录。
            </div>
          ) : (
            list.map((t) => (
              <div key={t.id} className={"theme-row" + (t.id === activeId ? " on" : "")}>
                <span className="theme-swatch" style={{ background: t.vars["--bg"] ?? "var(--bg-sub)" }}>
                  <i style={{ background: t.vars["--accent"] ?? "var(--accent)" }} />
                  <i style={{ background: t.vars["--text"] ?? "var(--text)" }} />
                </span>
                <div className="grow">
                  <div className="row" style={{ gap: 6 }}>
                    <span style={{ fontWeight: 500 }}>{t.name}</span>
                    <span className="muted mono" style={{ fontSize: 10.5 }}>
                      {t.id}
                    </span>
                  </div>
                  <div className="muted" style={{ fontSize: 11.5 }}>
                    {t.error ? (
                      <span style={{ color: "var(--danger)" }}>读不了：{t.error}</span>
                    ) : (
                      `${t.description || "（没有说明）"} · 来自${t.source} · ${Object.keys(t.vars).length} 个变量`
                    )}
                  </div>
                </div>
                <div className="row" style={{ gap: 4 }}>
                  <button
                    className="btn sm"
                    disabled={!!t.error}
                    onClick={() => void setCustomTheme(t.id === activeId ? null : t.id)}
                  >
                    {t.id === activeId ? "取消使用" : "使用"}
                  </button>
                  <button className="icon-btn" title="编辑这份 JSON" onClick={() => setEditing(themeJson(t))}>
                    <Icon name="pencil" size={13} />
                  </button>
                  {t.source === "工作区" && (
                    <button className="icon-btn" title="删除（进回收站）" onClick={() => void deleteTheme(t.id)}>
                      <Icon name="trash" size={13} />
                    </button>
                  )}
                </div>
              </div>
            ))
          )}
        </div>

        <div className="row wrap" style={{ gap: 6 }}>
          <button className="btn sm" onClick={() => setEditing(themeTemplate(themes?.vars ?? [], darkBase))}>
            <Icon name="plus" size={13} /> 从当前配色新建一份
          </button>
          <button className="btn sm" onClick={() => void openDir(false)}>
            <Icon name="folder" size={13} /> 主题目录
          </button>
          <button className="btn sm" onClick={() => void openDir(true)}>
            <Icon name="folder" size={13} /> 用户目录
          </button>
          <button className="btn sm" onClick={() => setVarsOpen((v) => !v)}>
            <Icon name="list" size={13} /> 可用的变量（{themes?.vars.length ?? 0}）
          </button>
        </div>

        {varsOpen && (
          <div className="theme-vars">
            <div className="muted" style={{ fontSize: 11.5 }}>
              <code className="mono">vars</code> 里的键要写成下面这些名字，值就是 CSS 里能用的任何写法
              （<code className="mono">#c9532a</code>、<code className="mono">oklch(0.7 0.1 40)</code> 都行）。
              没写的变量沿用内置配色——所以只改几个颜色也是完整可用的主题。
            </div>
            <div className="grid-2">
              {(themes?.vars ?? []).map((v) => (
                <div key={v.name} className="theme-var-row">
                  <code className="mono grow">{v.name}</code>
                  <span className="muted">{v.hint}</span>
                  <i className="theme-var-dot" style={{ background: `var(${v.name})` }} />
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {editing !== null && <ThemeEditor initial={editing} onClose={() => setEditing(null)} />}
    </section>
  );
}

/** 一份主题 → 可直接编辑的 JSON 文本。 */
function themeJson(t: CustomTheme): string {
  return JSON.stringify(
    {
      id: t.id,
      name: t.name,
      description: t.description,
      base: t.base,
      vars: t.vars,
    },
    null,
    2,
  );
}

/**
 * 新建时的模板：把**当前界面上真实的变量值**抄下来当起点。
 *
 * 这样用户拿到的是一份「现在就长这样」的完整主题，改哪几个颜色就是改哪几个，
 * 而不是面对一个空对象去猜变量名和取值。
 */
function themeTemplate(vars: { name: string; hint: string }[], dark: boolean): string {
  const cs = getComputedStyle(document.documentElement);
  const out: Record<string, string> = {};
  for (const v of vars) {
    const value = cs.getPropertyValue(v.name).trim();
    if (value) out[v.name] = value;
  }
  return JSON.stringify(
    {
      id: "my-theme",
      name: "我的主题",
      description: "从当前配色复制出来的，改几个颜色试试",
      base: dark ? "dark" : "light",
      vars: out,
    },
    null,
    2,
  );
}

/** 主题 JSON 编辑器。保存＝写进工作区主题目录并立刻启用。 */
function ThemeEditor({ initial, onClose }: { initial: string; onClose: () => void }) {
  const saveTheme = useApp((s) => s.saveTheme);
  const setCustomTheme = useApp((s) => s.setCustomTheme);
  const toast = useApp((s) => s.toast);
  const [text, setText] = useState(initial);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    let parsed: Partial<CustomTheme>;
    try {
      parsed = JSON.parse(text) as Partial<CustomTheme>;
    } catch (e) {
      toast("error", `JSON 有问题：${errText(e)}`);
      return;
    }
    if (!parsed || typeof parsed !== "object") {
      toast("error", "这里要是一份 JSON 对象");
      return;
    }
    const theme: CustomTheme = {
      id: String(parsed.id ?? "").trim(),
      name: String(parsed.name ?? "").trim(),
      description: String(parsed.description ?? ""),
      author: String(parsed.author ?? ""),
      base: parsed.base === "dark" ? "dark" : "light",
      vars: parsed.vars ?? {},
      source: "",
      path: "",
      error: null,
    };
    if (!theme.id) {
      toast("warn", "id 不能为空——它就是文件名（例如 my-theme → my-theme.json）");
      return;
    }
    if (Object.keys(theme.vars).length === 0) {
      toast("warn", "vars 里至少要写一个变量，否则这个主题什么也改不了");
      return;
    }
    setBusy(true);
    const ok = await saveTheme(theme);
    if (ok) await setCustomTheme(theme.id);
    setBusy(false);
    if (ok) onClose();
  };

  return (
    <Modal
      title="编辑主题"
      icon="pencil"
      wide
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            保存后会写进 <code className="mono">工作区/.hub/themes/</code> 并立刻启用
          </span>
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button className="btn primary" onClick={() => void save()} disabled={busy}>
            {busy ? "保存中…" : "保存并启用"}
          </button>
        </>
      }
    >
      <div className="col" style={{ gap: 8 }}>
        <div className="muted" style={{ fontSize: 11.5 }}>
          <code className="mono">vars</code> 里的键是 CSS 变量名（以 <code className="mono">--</code> 开头），
          值就是颜色、字体、圆角这类 CSS 取值。写错的键会被忽略，不影响其它变量。
        </div>
        <textarea
          className="textarea mono"
          style={{ minHeight: "46vh", fontSize: 12, lineHeight: 1.6 }}
          value={text}
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
        />
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- 模型档案

function ProfilesSection() {
  const config = useApp((s) => s.config)!;
  const upsert = useApp((s) => s.upsertProfile);
  const remove = useApp((s) => s.deleteProfile);
  const patchConfig = useApp((s) => s.patchConfig);
  const providerKinds = useApp((s) => s.providerKinds);

  const editingId = useState<string | null>(null);
  const [selected, setSelected] = editingId;
  const current = config.profiles.find((p) => p.id === selected) ?? null;

  const [form, setForm] = useState<ProfileInput | null>(null);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string; latencyMs: number } | null>(null);
  const [apiKey, setApiKey] = useState("");

  useEffect(() => {
    if (current) {
      setForm({
        id: current.id,
        name: current.name,
        kind: current.kind,
        baseUrl: current.baseUrl,
        model: current.model,
        temperature: current.temperature,
        maxTokens: current.maxTokens,
        supportsTools: current.supportsTools,
        supportsVision: current.supportsVision,
        headers: {},
        apiKey: null,
        reasoning: current.reasoning,
      });
      setApiKey("");
      setTestResult(null);
    } else {
      setForm(null);
    }
  }, [selected]); // eslint-disable-line react-hooks/exhaustive-deps

  function newProfile() {
    const kind: ProviderKind = "open_ai";
    const preset = providerKinds.find((k) => k.id === kind);
    setForm({
      name: "新模型",
      kind,
      baseUrl: preset?.defaultBaseUrl ?? "https://api.deepseek.com/v1",
      model: preset?.defaultModel ?? "deepseek-chat",
      temperature: 0.5,
      maxTokens: 8192,
      supportsTools: true,
      supportsVision: true,
      headers: {},
      apiKey: "",
      reasoning: { effort: "off", style: "auto" },
    });
    setSelected(null);
    setTestResult(null);
  }

  return (
    <section className="col">
      <h2>
        模型档案 <span className="sub">可以有多个，随时切换</span>
      </h2>
      <div className="card-box">
        <div className="col" style={{ gap: 6 }}>
          {config.profiles.map((p) => (
            <div
              key={p.id}
              className={"profile-row" + (p.id === config.activeProfileId ? " on" : "")}
              onClick={() => setSelected(p.id === selected ? null : p.id)}
            >
              <Icon name={p.id === config.activeProfileId ? "check" : "hub"} size={14} />
              <div className="grow">
                <div style={{ fontWeight: 500 }}>{p.name}</div>
                <div className="muted mono" style={{ fontSize: 11 }}>
                  {p.kindLabel} · {p.model} · {p.baseUrl}
                </div>
              </div>
              {!p.hasApiKey && <span className="tag">无 Key</span>}
              <span className="muted mono" style={{ fontSize: 10.5 }}>
                {p.keyHint}
              </span>
              <div className="row" style={{ gap: 4 }} onClick={(e) => e.stopPropagation()}>
                {p.id !== config.activeProfileId && (
                  <button className="btn sm" onClick={() => void patchConfig({ activeProfileId: p.id })}>
                    设为当前
                  </button>
                )}
                <button className="icon-btn" title="删除" onClick={() => void remove(p.id)}>
                  <Icon name="trash" size={13} />
                </button>
              </div>
            </div>
          ))}
        </div>

        <div className="row">
          <button className="btn" onClick={newProfile}>
            <Icon name="plus" size={13} /> 添加模型档案
          </button>
          <span className="muted" style={{ fontSize: 11.5 }}>
            支持任何 OpenAI 兼容接口（DeepSeek / Moonshot / 通义 / 硅基流动 / Ollama / vLLM）与 Anthropic
          </span>
        </div>

        {form && (
          <div className="card-box" style={{ background: "var(--bg-sub)" }}>
            <div className="grid-2">
              <Field label="显示名">
                <input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
              </Field>
              <Field label="协议">
                <select
                  className="select"
                  value={form.kind}
                  onChange={(e) => {
                    const kind = e.target.value as ProviderKind;
                    const preset = providerKinds.find((k) => k.id === kind);
                    setForm({
                      ...form,
                      kind,
                      baseUrl: preset?.defaultBaseUrl ?? form.baseUrl,
                      model: preset?.defaultModel ?? form.model,
                    });
                  }}
                >
                  {providerKinds.map((k) => (
                    <option key={k.id} value={k.id}>
                      {k.label}
                    </option>
                  ))}
                </select>
              </Field>
            </div>

            <Field label="接口地址" hint="OpenAI 兼容要带上 /v1；Anthropic 填到 /v1 即可，后面会自动补 /messages">
              <input
                className="input mono"
                value={form.baseUrl}
                onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
              />
            </Field>

            <div className="grid-2">
              <Field label="模型名">
                <input className="input mono" value={form.model} onChange={(e) => setForm({ ...form, model: e.target.value })} />
              </Field>
              <Field label="API Key" hint={current?.hasApiKey ? `已保存 ${current.keyHint}，留空表示不修改` : "明文存在本机配置里，不会上传"}>
                <input
                  className="input mono"
                  type="password"
                  value={apiKey}
                  placeholder={current?.hasApiKey ? "••••••（不改就留空）" : "sk-…"}
                  onChange={(e) => setApiKey(e.target.value)}
                />
              </Field>
            </div>

            <div className="grid-2">
              <Field label={`温度：${form.temperature.toFixed(2)}`} hint="越低越稳，学习讲解建议 0.3~0.7">
                <input
                  type="range"
                  min={0}
                  max={2}
                  step={0.05}
                  value={form.temperature}
                  onChange={(e) => setForm({ ...form, temperature: Number(e.target.value) })}
                  style={{ width: "100%" }}
                />
              </Field>
              <Field label="最大输出 tokens">
                <input
                  className="input"
                  type="number"
                  value={form.maxTokens}
                  onChange={(e) => setForm({ ...form, maxTokens: Number(e.target.value) })}
                />
              </Field>
            </div>

            <Switch
              checked={form.supportsTools}
              onChange={(v) => setForm({ ...form, supportsTools: v })}
              label="该模型支持原生工具调用（不支持时 agent 会降级成文字建议）"
            />

            <Switch
              checked={form.supportsVision ?? true}
              onChange={(v) => setForm({ ...form, supportsVision: v })}
              label="该模型支持图片输入（关掉后贴的图不会发出去，会换成一行文字说明）"
            />

            <Field
              label="思考强度参数的写法"
              hint="对话栏里模型旁边那颗「思考」芯片调的是强度，这里决定用哪种写法发出去。有的服务商不认这些扩展，报错就选「不发」。"
            >
              <select
                className="select"
                value={form.reasoning?.style ?? "auto"}
                onChange={(e) =>
                  setForm({
                    ...form,
                    reasoning: {
                      effort: form.reasoning?.effort ?? "off",
                      style: e.target.value as ReasoningStyle,
                    },
                  })
                }
              >
                {(Object.keys(STYLE_LABEL) as ReasoningStyle[]).map((k) => (
                  <option key={k} value={k}>
                    {STYLE_LABEL[k]}
                  </option>
                ))}
              </select>
            </Field>

            <div className="row">
              <button
                className="btn primary"
                onClick={() => {
                  void upsert({ ...form, apiKey: apiKey ? apiKey : undefined });
                  setSelected(form.id ?? null);
                  setApiKey("");
                }}
              >
                保存
              </button>
              {form.id && (
                <button
                  className="btn"
                  disabled={testing}
                  onClick={async () => {
                    setTesting(true);
                    setTestResult(null);
                    try {
                      const r = await api.profileTest(form.id!);
                      setTestResult(r);
                    } catch (e) {
                      setTestResult({ ok: false, message: errText(e), latencyMs: 0 });
                    } finally {
                      setTesting(false);
                    }
                  }}
                >
                  {testing ? <Spinner /> : <Icon name="play" size={13} />} 测试连接
                </button>
              )}
              {testResult && (
                <span
                  className="grow"
                  style={{ fontSize: 12, color: testResult.ok ? "var(--ok)" : "var(--danger)" }}
                >
                  {testResult.message}
                  {testResult.latencyMs > 0 && `（${testResult.latencyMs} ms）`}
                </span>
              )}
              {form.id && !testResult && (
                <span className="grow muted" style={{ fontSize: 11.5 }}>
                  保存后可以点「测试连接」发一次 ping
                </span>
              )}
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
