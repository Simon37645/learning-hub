// 设置页：左边菜单，右边面板。
//
// v0.6.1 起改成两栏：原来是一条长页（用户 / 外观 / 工作区 / 模型档案 / Agent 行为 /
// 内置浏览器 / 沙箱 / 长期记忆 / 技能与 MCP / 版本与更新 / 关于 一路往下排），
// 想改一个字段得先滚半天，也看不出到底有哪些设置。现在按「基础设置 / Agent 能力 / 其它」
// 分三组列在左边，右边一次只显示一节，标题在顶上。
//
// 约定：每一节是一个组件，**只返回卡片**，页标题与说明由下面的 NAV 表提供——
// 别在组件里再写一遍 <h2>，那会和页头重复。

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errText } from "../lib/api";
import {
  PERMISSION_LABEL,
  STYLE_LABEL,
  type CustomTheme,
  type PermissionMode,
  type ProfileInput,
  type ReasoningStyle,
  type ProviderKind,
  type UpdateInfo,
  type UpdateProgressEvent,
} from "../lib/types";
import { clampSearchCount, COUNT_MAX, COUNT_MIN } from "../lib/websearch";
import { useApp } from "../store/app";
import { Field, FieldRow, Icon, Modal, Segmented, Spinner, Switch, type IconName } from "./ui";

/** 本地草稿输入框：失焦 / 回车才提交。
 *
 * 直接把 config 当受控值、每个按键 patch 一次的话，打字快了会被
 * 往返中的旧值吞字，几个并发 patch 还可能乱序落盘把最后几个字弄丢。 */
function DraftInput({
  value,
  onCommit,
  ...rest
}: { value: string; onCommit: (v: string) => void } & Omit<React.InputHTMLAttributes<HTMLInputElement>, "value" | "onChange">) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <input
      {...rest}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => draft !== value && onCommit(draft)}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.nativeEvent.isComposing && draft !== value) {
          (e.target as HTMLInputElement).blur();
        }
      }}
    />
  );
}

/** 同 DraftInput 的 textarea 版（额外系统指令这类要写长句的字段） */
function DraftTextarea({
  value,
  onCommit,
  ...rest
}: { value: string; onCommit: (v: string) => void } & Omit<React.TextareaHTMLAttributes<HTMLTextAreaElement>, "value" | "onChange">) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <textarea
      {...rest}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => draft !== value && onCommit(draft)}
    />
  );
}

// ---------------------------------------------------------------- 菜单

type SectionId =
  | "general"
  | "appearance"
  | "models"
  | "viewer"
  | "agent"
  | "sandbox"
  | "memory"
  | "skills"
  | "update"
  | "about";

interface NavItem {
  id: SectionId;
  label: string;
  icon: IconName;
  /** 右侧页头的大标题与说明（菜单上放不下，所以写在这里） */
  title: string;
  sub: string;
}

const NAV: { group: string; items: NavItem[] }[] = [
  {
    group: "基础设置",
    items: [
      { id: "general", label: "常规", icon: "user", title: "常规", sub: "你的名字与工作区根目录" },
      {
        id: "appearance",
        label: "外观",
        icon: "palette",
        title: "外观",
        sub: "明亮 / 深色 / 跟随系统，或者自己写一套配色",
      },
      { id: "models", label: "模型设置", icon: "hub", title: "模型设置", sub: "可以有多个档案，随时切换" },
      {
        id: "viewer",
        label: "浏览器控制",
        icon: "globe",
        title: "浏览器控制",
        sub: "agent 和你共用同一个浏览窗口",
      },
    ],
  },
  {
    group: "Agent 能力",
    items: [
      {
        id: "agent",
        label: "Agent 行为",
        icon: "sparkle",
        title: "Agent 行为",
        sub: "决定它多主动、能碰什么",
      },
      {
        id: "sandbox",
        label: "工作区沙箱",
        icon: "shield",
        title: "工作区沙箱",
        sub: "agent 默认只能碰工作区里的文件",
      },
      {
        id: "memory",
        label: "长期记忆",
        icon: "book",
        title: "长期记忆",
        sub: "记住你的特点与「以后要注意的地方」",
      },
      {
        id: "skills",
        label: "技能与 MCP",
        icon: "puzzle",
        title: "技能与 MCP",
        sub: "开关在侧栏，造新的去工坊",
      },
    ],
  },
  {
    group: "其它",
    items: [
      {
        id: "update",
        label: "版本与更新",
        icon: "download",
        title: "版本与更新",
        sub: "新版本发布在 GitHub，应用内直接装",
      },
      { id: "about", label: "关于", icon: "info", title: "关于", sub: "版本、工具清单、系统提示词" },
    ],
  },
];

const NAV_ITEMS: NavItem[] = NAV.flatMap((g) => g.items);

export function Settings() {
  const setView = useApp((s) => s.setView);
  const [active, setActive] = useState<SectionId>("general");
  const item = NAV_ITEMS.find((i) => i.id === active) ?? NAV_ITEMS[0];

  return (
    <div className="settings">
      <nav className="settings-nav">
        <button
          className="btn sm settings-back"
          onClick={() => {
            // 与侧栏「首页」按钮同一条路（leaveTopic + setView）：光 setView("home")
            // 的话，上一条对话的身份还挂着——从工坊进设置再返回，首页就挂着工坊
            // 对话（消息列表、chatMode 全是工坊的），在这里发消息会继续写进那条
            // 工坊对话，而它并不出现在首页的对话清单里。leaveTopic 把对话身份
            // 一并清掉，回首页就是「新起一条」。
            useApp.getState().leaveTopic();
            setView("home");
          }}
        >
          <Icon name="arrow-left" size={13} /> 返回工作区
        </button>
        {NAV.map((g) => (
          <div key={g.group} className="settings-nav-group">
            <div className="settings-nav-label">{g.group}</div>
            {g.items.map((it) => (
              <button
                key={it.id}
                type="button"
                className={"settings-nav-item" + (it.id === active ? " on" : "")}
                onClick={() => setActive(it.id)}
              >
                <Icon name={it.icon} size={14} />
                <span className="grow">{it.label}</span>
              </button>
            ))}
          </div>
        ))}
      </nav>

      <div className="settings-body">
        <div className="settings-inner">
          <header className="settings-head">
            <h1>{item.title}</h1>
            <p>{item.sub}</p>
          </header>

          {active === "general" && <GeneralSection />}
          {active === "appearance" && <AppearanceSection />}
          {active === "models" && <ProfilesSection />}
          {active === "viewer" && <ViewerSection />}
          {active === "agent" && <AgentSection />}
          {active === "sandbox" && <SandboxSection />}
          {active === "memory" && <MemorySection />}
          {active === "skills" && <SkillsSection />}
          {active === "update" && <UpdateSection />}
          {active === "about" && <AboutSection />}
        </div>
      </div>

      <PromptPreviewModal />
    </div>
  );
}

// ---------------------------------------------------------------- 常规

/** 名字与工作区根目录：两件「应用长什么样」的事，都很少改，所以合在一节。 */
function GeneralSection() {
  const config = useApp((s) => s.config)!;
  const patchConfig = useApp((s) => s.patchConfig);
  const toast = useApp((s) => s.toast);
  const [ws, setWs] = useState<{ root: string; topicCount: number; diskUsageText: string } | null>(null);
  const [name, setName] = useState(config.userName ?? "");

  useEffect(() => {
    void api.workspaceInfo().then(setWs).catch(() => setWs(null));
  }, []);

  return (
    <>
      <div className="card-box">
        <FieldRow label="你的名字" hint="主界面问候语里用（回车或点别处即保存）">
          <input
            className="input"
            value={name}
            placeholder="你的名字"
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() && name !== config.userName && void patchConfig({ userName: name.trim() })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing && name.trim()) void patchConfig({ userName: name.trim() });
            }}
          />
          <button
            className="btn sm"
            disabled={!name.trim() || name === config.userName}
            onClick={() => void patchConfig({ userName: name.trim() })}
          >
            保存
          </button>
        </FieldRow>
      </div>

      <div className="card-box">
        <FieldRow label="工作区根目录" hint="所有主题目录的根；每个主题就是一个目录，里面是纯 Markdown / PDF / JSON，可以直接用编辑器改">
          <DraftInput
            className="input mono"
            value={config.workspaceRoot}
            onCommit={(v) => v.trim() && void patchConfig({ workspaceRoot: v.trim() })}
          />
          <button
            className="btn sm"
            onClick={async () => {
              const { open } = await import("@tauri-apps/plugin-dialog");
              const dir = await open({ directory: true, title: "选择工作区目录" });
              if (typeof dir === "string") void patchConfig({ workspaceRoot: dir });
            }}
          >
            <Icon name="folder" size={13} /> 选择
          </button>
          <button className="btn sm" onClick={() => void api.revealInExplorer().catch((e) => toast("error", errText(e)))}>
            打开
          </button>
        </FieldRow>
        {ws && (
          <div className="hint">
            当前：{ws.topicCount} 个主题 · 占用 {ws.diskUsageText}
          </div>
        )}
      </div>
    </>
  );
}

// ---------------------------------------------------------------- 内置浏览器

function ViewerSection() {
  const config = useApp((s) => s.config)!;
  const patchConfig = useApp((s) => s.patchConfig);
  return (
    <div className="card-box">
      <FieldRow label="主页" hint="内置浏览器新标签打开的那个页面">
        <DraftInput className="input mono" value={config.viewer.homeUrl} onCommit={(v) => void patchConfig({ homeUrl: v })} />
      </FieldRow>
      <FieldRow label="联网搜索默认引擎">
        <select
          className="select"
          value={config.viewer.searchEngine}
          onChange={(e) => void patchConfig({ searchEngine: e.target.value })}
        >
          <option value="duckduckgo">DuckDuckGo</option>
          <option value="bing">Bing</option>
        </select>
      </FieldRow>
      <div className="hint">
        PDF 用 pdf.js 渲染（agent 能读每一页的文字、能翻页）；网页用内嵌窗口打开，
        遇到禁止嵌入的站点可以切「阅读模式」看提取后的正文。
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Agent 行为

function AgentSection() {
  const config = useApp((s) => s.config)!;
  const patchConfig = useApp((s) => s.patchConfig);
  const agent = config.agent;
  const webSearch = config.webSearch;

  /** 数字字段统一走这里：只在是合法数字且不小于 min 时才写回配置 */
  const commitNumber = (key: "maxIterations" | "requestTimeoutSecs" | "reconnectMaxAttempts" | "contextBudgetChars", min: number) => (
    v: string,
  ) => {
    const n = Number(v);
    if (Number.isFinite(n) && n >= min) void patchConfig({ [key]: n });
  };

  return (
    <div className="card-box">
      <FieldRow label="工具权限" hint="读操作永远直接执行；这里调的是「写」和「删」的尺度">
        <div className="row wrap" style={{ gap: 6 }}>
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
      </FieldRow>

      <FieldRow label="单轮最多工具往返次数" hint="防止 agent 反复调用工具停不下来">
        <DraftInput
          className="input num"
          type="number"
          min={1}
          max={100}
          value={String(agent.maxIterations)}
          onCommit={commitNumber("maxIterations", 1)}
        />
      </FieldRow>

      <FieldRow label="单次请求超时（秒）">
        <DraftInput
          className="input num"
          type="number"
          min={30}
          max={1800}
          value={String(agent.requestTimeoutSecs)}
          onCommit={commitNumber("requestTimeoutSecs", 30)}
        />
      </FieldRow>

      <FieldRow label="断线自动重连次数" hint="模型连接中断（超时 / 限流 / 服务端过载）后自动重试的上限，0 = 不重连">
        <DraftInput
          className="input num"
          type="number"
          min={0}
          max={50}
          value={String(agent.reconnectMaxAttempts)}
          onCommit={commitNumber("reconnectMaxAttempts", 0)}
        />
      </FieldRow>

      <FieldRow label="上下文预算（字符）" hint="发给模型的对话历史上限，超出会从最早的完整轮次开始丢弃">
        <DraftInput
          className="input num"
          type="number"
          min={2000}
          max={500000}
          step={1000}
          value={String(agent.contextBudgetChars)}
          onCommit={commitNumber("contextBudgetChars", 2000)}
        />
      </FieldRow>

      <FieldRow label="允许 agent 联网" hint="放开 web_fetch / web_search 两个工具">
        <Switch checked={agent.allowWeb} onChange={(v) => void patchConfig({ allowWeb: v })} />
      </FieldRow>

      <FieldRow
        label="联网搜索"
        hint="agent 遇到拿不准的事实、时效性问题先搜再答（DeepSeek Search / Fetch，与模型 API 同一把 Key）。按次计费，默认 8 条结果、不抓全文"
      >
        <Switch
          checked={webSearch.enabled}
          onChange={(v) => void patchConfig({ webSearchEnabled: v })}
        />
      </FieldRow>

      <FieldRow
        label="DeepSeek API Key"
        hint={
          webSearch.hasKey
            ? "已保存（明文只存在本机 config.json，不会上传）。留空表示不修改；想清除就提交一个空值"
            : "还没配置。与 DeepSeek 模型 API 是同一把 Key；搜索与抓取按次计费"
        }
      >
        <input
          className="input mono"
          type="password"
          placeholder={webSearch.hasKey ? "••••••（不改就留空）" : "sk-在此填入你的Key"}
          onBlur={(e) => {
            const v = e.target.value.trim();
            // 只在用户真的输入了内容时才写回： onBlur 空值不覆盖已有 Key
            //（想清除就先粘贴一个空格再清空——编辑框里按原样提交）
            if (v) {
              void patchConfig({ deepseekApiKey: v });
              e.target.value = "";
            }
          }}
        />
      </FieldRow>

      <FieldRow
        label="单次搜索条数"
        hint="1-50，默认 8。条数越多单次返回越多，但结果都是按同一次请求计费的"
      >
        <DraftInput
          className="input num"
          type="number"
          min={COUNT_MIN}
          max={COUNT_MAX}
          value={String(webSearch.count)}
          onCommit={(v) => void patchConfig({ webSearchCount: clampSearchCount(v) })}
        />
      </FieldRow>

      <FieldRow
        label="额外系统指令"
        hint="追加到系统提示词末尾，优先级最高（失焦后保存）。例如「多举数学例子」「解释时先用中文再给英文术语」"
      >
        <DraftTextarea
          className="textarea"
          rows={4}
          value={agent.systemPromptExtra}
          placeholder="想让 agent 怎么带你？写在这里。"
          onCommit={(v) => void patchConfig({ systemPromptExtra: v })}
        />
      </FieldRow>
    </div>
  );
}

// ---------------------------------------------------------------- 沙箱

function SandboxSection() {
  const config = useApp((s) => s.config)!;
  const patchConfig = useApp((s) => s.patchConfig);
  const agent = config.agent;
  return (
    <div className="card-box">
      <FieldRow
        label="开启沙箱"
        hint="访问工作区之外的文件前必须向你申请；关掉之后 agent 可以自由读写本机任意路径，只建议在完全信任的场景下使用"
      >
        <Switch checked={agent.sandbox} onChange={(v) => void patchConfig({ sandbox: v })} />
      </FieldRow>

      <div className="field">
        <label>已授权的目录</label>
        {agent.approvedRoots.length === 0 ? (
          <div className="hint">
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
          <button
            className="btn sm"
            style={{ alignSelf: "flex-start" }}
            onClick={() => void patchConfig({ clearApprovedRoots: true })}
          >
            全部撤销
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 长期记忆

function MemorySection() {
  const config = useApp((s) => s.config)!;
  const patchConfig = useApp((s) => s.patchConfig);
  return (
    <div className="card-box">
      <FieldRow
        label="启用长期记忆"
        hint="每轮对话都把相关记忆交给 agent；关掉开关只是不再注入提示词，已有记忆仍可查看"
      >
        <Switch checked={config.agent.memoryEnabled} onChange={(v) => void patchConfig({ memoryEnabled: v })} />
      </FieldRow>
      <div className="hint">
        agent 会在对话里主动记下值得长期保留的信息（你的基础、偏好、反复出错的点、还缺的前置），
        记在 <code className="mono">.hub/memory/memories.jsonl</code> 里——普通文本，可以直接用编辑器改。
        查看、编辑、钉住或删除记忆：侧栏顶部的「记忆」。
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 技能与 MCP

function SkillsSection() {
  return (
    <div className="card-box">
      <div className="hint">
        侧栏顶部的「技能」与「MCP 服务器」两个入口里，可以按<b>全局</b>或<b>本主题</b>分别开关。
        全局的写进配置文件（所有主题可见），本主题的只影响当前主题。
      </div>
      <FieldRow label="造一个新的" hint="工坊是一个独立于学习的 agent 模式：读规范 → 在练习目录里写 → 发布即重连">
        <button className="btn sm" onClick={() => void useApp.getState().openStudio()}>
          <Icon name="hammer" size={13} /> 去工坊
        </button>
      </FieldRow>
    </div>
  );
}

// ---------------------------------------------------------------- 关于

function AboutSection() {
  const tools = useApp((s) => s.tools);
  const version = useApp((s) => s.version);
  const toast = useApp((s) => s.toast);
  const [toolsOpen, setToolsOpen] = useState(false);

  return (
    <>
      <div className="card-box">
        <FieldRow
          label={`学习中枢 v${version || "…"}`}
          hint="Tauri + Rust 后端，React 前端。数据全部在你自己的磁盘上，没有账号、没有云端。"
        >
          <button className="btn sm" onClick={() => setToolsOpen(true)}>
            <Icon name="sparkle" size={13} /> {tools.length} 个内置工具
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
        </FieldRow>
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
    </>
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
    <>
      <div className="card-box">
        <FieldRow label="界面缩放" hint="放大整个界面（等效浏览器缩放），100% 为默认。内置浏览器里的网页视图会跟着对齐，布局不会乱">
          <Segmented
            value={String(config.appearance.uiScale ?? 1)}
            onChange={(v) => void useApp.getState().setUiScale(Number(v))}
            options={[
              { id: "0.9", label: "90%" },
              { id: "1", label: "100%" },
              { id: "1.1", label: "110%" },
              { id: "1.25", label: "125%" },
              { id: "1.5", label: "150%" },
            ]}
          />
        </FieldRow>
        <FieldRow label="配色" hint="强制明亮适合长时间读讲义；侧栏底部也有一个快捷切换按钮">
          <Segmented
            value={config.appearance.theme}
            onChange={(v) => void useApp.getState().setTheme(v)}
            options={[
              { id: "system", label: "跟随系统" },
              { id: "light", label: "明亮" },
              { id: "dark", label: "深色" },
            ]}
          />
        </FieldRow>

        {active && (
          <div className="hint">
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
            <div className="hint">
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
    </>
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

// ---------------------------------------------------------------- 版本与更新

/**
 * 一键更新：检测 GitHub release → 确认 → 下载（进度条）→ 静默安装并自动重启。
 *
 * 只在用户点按钮时才去问 GitHub（无认证限额 60 次/小时），挂载时的那一次
 * 是为了把「当前版本 + 有没有新版」摆出来；不做任何自动轮询。
 * 安装是「分离启动 NSIS 安装器」：**安装器自己会关掉本应用**（Restart Manager，
 * 见 `updater::run_installer` 的说明），装完它再拉起新版。
 * 万一它没能关掉本应用（那意味着它已经静默放弃安装了），界面必须说清楚，
 * 所以这里备了一条「卡住」提示 —— 不能只留一句「正在启动安装程序…」让用户干等。
 */
function UpdateSection() {
  const toast = useApp((s) => s.toast);
  const version = useApp((s) => s.version);
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  /** idle → downloading → installing（停在 installing 免得按钮闪回） */
  const [phase, setPhase] = useState<"idle" | "downloading" | "installing">("idle");
  const [progress, setProgress] = useState<{ received: number; total: number | null }>({ received: 0, total: null });
  const [confirming, setConfirming] = useState(false);
  /** 最近一次检查是否失败：失败要跟「还没检查过」分开说，别误导用户 */
  const [failed, setFailed] = useState(false);
  /** 下载好的安装包路径（卡住时告诉用户它在哪，可以直接双击装） */
  const [installer, setInstaller] = useState<string | null>(null);
  /** 启动安装器后迟迟没退出应用：安装器多半已经放弃了，得给用户一条出路 */
  const [stuck, setStuck] = useState(false);
  const stuckTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (stuckTimer.current !== null) window.clearTimeout(stuckTimer.current);
    },
    [],
  );

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setInfo(await api.updateCheck());
      setFailed(false);
    } catch (e) {
      setFailed(true);
      toast("error", errText(e));
    } finally {
      setChecking(false);
    }
  }, [toast]);

  // 卡片挂载时拉一次：常显当前版本，顺便知道有没有新版
  useEffect(() => {
    void check();
  }, [check]);

  // 下载进度。监听挂在组件里、卸载时真的退订——StrictMode 会挂两遍，
  // 不退订的话进度事件会被处理两次（这是一开始就把 init 做成幂等的同一个坑）
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<UpdateProgressEvent>("hub://update", (e) => {
      if (e.payload.kind === "progress") {
        setProgress({ received: e.payload.received, total: e.payload.total });
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const install = async () => {
    if (!info?.assetUrl || phase !== "idle") return;
    setConfirming(false);
    setPhase("downloading");
    setProgress({ received: 0, total: info.assetSize ?? null });
    try {
      const path = await api.updateDownload(info.assetUrl, info.assetSize ?? null);
      setInstaller(path);
      // 界面快照先落盘：接下来应用会**被安装器关掉**（Restart Manager，
      // 可能是强制终止），没有第二次机会保存。
      await useApp.getState().saveUiState();
      setPhase("installing");
      await api.updateRun(path);
      toast("info", "安装程序已启动，应用即将关闭并在安装完成后自动重启");
      // 这里不再自己关窗口：安装器起来后第一件事就是关掉本应用（见 updater.rs）。
      // 关不掉它就静默放弃安装——所以下面备一条「卡住」的提示，别让用户干等。
      setStuck(false);
      stuckTimer.current = window.setTimeout(() => setStuck(true), 12000);
    } catch (e) {
      setPhase("idle");
      toast("error", errText(e));
    }
  };

  const busy = phase !== "idle";
  const pct = progress.total ? Math.min(100, (progress.received / progress.total) * 100) : null;

  return (
    <>
      <div className="card-box">
        <div className="row">
          <span className="sub" style={{ fontSize: 12.5 }}>
            当前版本 v{info?.current ?? (version || "…")}
          </span>
          <div className="grow" />
          <button className="btn sm" disabled={checking || busy} onClick={() => void check()}>
            {checking ? <Spinner /> : <Icon name="refresh" size={13} />} 检查更新
          </button>
        </div>

        {info?.available && info.version ? (
          <div className="col" style={{ gap: 8 }}>
            <div className="row wrap" style={{ gap: 8 }}>
              <span style={{ fontWeight: 500 }}>有新版本：v{info.version}</span>
              {info.publishedAt && (
                <span className="muted" style={{ fontSize: 12 }}>
                  发布于 {formatPublished(info.publishedAt)}
                </span>
              )}
              {info.assetSize != null && (
                <span className="muted" style={{ fontSize: 12 }}>
                  安装包约 {mb(info.assetSize)} MB
                </span>
              )}
            </div>
            {info.notes && <pre className="update-notes">{info.notes}</pre>}
            {phase === "downloading" && (
              <div className="col" style={{ gap: 6 }}>
                <div className={"progress" + (pct === null ? " indeterminate" : "")}>
                  <div className="progress-fill" style={{ width: pct === null ? undefined : `${pct}%` }} />
                </div>
                <div className="muted" style={{ fontSize: 12 }}>
                  {pct === null
                    ? `已下载 ${mb(progress.received)} MB`
                    : `已下载 ${mb(progress.received)} / ${mb(progress.total!)} MB`}
                </div>
              </div>
            )}
            {phase === "installing" && (
              <div className="col" style={{ gap: 6 }}>
                <div className="muted" style={{ fontSize: 12 }}>
                  正在启动安装程序…
                </div>
                {stuck && (
                  <>
                    <div className="muted" style={{ fontSize: 12 }}>
                      应用本该在这时候自动关闭。它没有关闭，通常意味着安装程序没能接管
                      （最常见的原因是安装包被安全软件拦下了）。安装包已经下载好：
                    </div>
                    <div className="mono muted" style={{ fontSize: 11.5, wordBreak: "break-all" }}>
                      {installer}
                    </div>
                    <div className="muted" style={{ fontSize: 12 }}>
                      可以退出本应用后直接双击它安装；也可以先「重新启动安装程序」再试一次。
                    </div>
                    <div className="row" style={{ gap: 8 }}>
                      <button
                        className="btn sm"
                        onClick={() => {
                          if (stuckTimer.current !== null) window.clearTimeout(stuckTimer.current);
                          setStuck(false);
                          void (async () => {
                            try {
                              await api.updateRun(installer!);
                              stuckTimer.current = window.setTimeout(() => setStuck(true), 12000);
                            } catch (e) {
                              toast("error", errText(e));
                            }
                          })();
                        }}
                      >
                        <Icon name="refresh" size={13} /> 重新启动安装程序
                      </button>
                      <button className="btn sm" onClick={() => setPhase("idle")}>
                        稍后再说
                      </button>
                    </div>
                  </>
                )}
              </div>
            )}
            {phase === "idle" && (
              <div className="row">
                <button className="btn primary" disabled={!info.assetUrl} onClick={() => setConfirming(true)}>
                  <Icon name="download" size={13} /> 下载并安装
                </button>
                {!info.assetUrl && (
                  <span className="muted" style={{ fontSize: 12 }}>
                    这个版本没有找到安装包，可以去 GitHub 手动下载
                  </span>
                )}
              </div>
            )}
          </div>
        ) : (
          !checking && (
            <div className="muted" style={{ fontSize: 12 }}>
              {info
                ? "已是最新版本。"
                : failed
                  ? "这次没能查成（原因见提示），可以稍后再点一次「检查更新」。"
                  : "还没有检查过，点上面的「检查更新」试试。"}
            </div>
          )
        )}
      </div>

      {confirming && (
        <Modal
          title={`更新到 v${info?.version ?? ""}？`}
          icon="download"
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirming(false)}>
                取消
              </button>
              <button className="btn primary" onClick={() => void install()}>
                下载并安装
              </button>
            </>
          }
        >
          <div className="sub">
            会下载安装包（{info?.assetSize != null ? `约 ${mb(info.assetSize)} MB` : "大小未知"}）并静默安装，
            期间应用会自动关闭，安装完成后自动重新打开。
          </div>
        </Modal>
      )}
    </>
  );
}

/** GitHub 的 ISO 时间 → 本地时间短句；解析不了就原样返回。 */
function formatPublished(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

/** 字节 → MB 文本（进度条旁边的数字）。 */
function mb(bytes: number): string {
  return (bytes / 1024 / 1024).toFixed(1);
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
  );
}
