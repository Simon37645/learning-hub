// 设置页：用户 / 工作区 / 模型档案 / Agent 行为 / 内置浏览器 / 关于。

import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import {
  PERMISSION_LABEL,
  STYLE_LABEL,
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
        <section className="col">
          <h2>
            外观 <span className="sub">明亮 / 深色 / 跟随系统</span>
          </h2>
          <div className="card-box">
            <div className="row">
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
          </div>
        </section>

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

        {/* ---------------- 技能与 MCP 的入口 ---------------- */}
        <section className="col">
          <h2>
            技能与 MCP <span className="sub">已挪到侧栏顶部，和「新建主题」放在一起</span>
          </h2>
          <div className="card-box">
            <div className="sub" style={{ fontSize: 12.5 }}>
              侧栏顶部的「技能」与「MCP 服务器」两个入口里，可以按**全局**或**本主题**分别开关。
              全局的写进配置文件（所有主题可见），本主题的只影响当前主题。
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
