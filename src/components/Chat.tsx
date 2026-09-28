// 对话面板：消息流、工具卡片、审批弹窗、输入框。

import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useApp, type ToolActivity } from "../store/app";
import { api, errText } from "../lib/api";
import { highlightWithin, linkifyCitations, renderMarkdown, renderMermaidIn } from "../lib/markdown";
import { clampText, fmtClock, hotkey } from "../lib/format";
import {
  PERMISSION_LABEL,
  RISK_LABEL,
  SANDBOX_MODE_LABEL,
  STAGE_LABEL,
  type ChatMessage,
  type ContentBlock,
  type PermissionMode,
  type Risk,
  type StudyStage,
  type ToolOutcomeView,
} from "../lib/types";
import { Dropdown, Icon, MenuItem, MenuLabel, MenuSep, Modal, Spinner, AutoTextarea } from "./ui";
import { TopicWelcome, Welcome } from "./Home";

// ---------------------------------------------------------------- Markdown

export function Markdown({ source, onLink }: { source: string; onLink?: (href: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const deferred = useDeferredValue(source);
  const html = useMemo(() => renderMarkdown(deferred), [deferred]);

  useEffect(() => {
    highlightWithin(ref.current);
    // agent 写的【来源：…】标注在这里变成可点击的跳转（点了就在内置浏览器里打开对应页）
    linkifyCitations(ref.current, (cite) => {
      void useApp.getState().openFile(cite.path, undefined, cite.page ?? undefined);
    });
    // 思维导图 / 流程图等 mermaid 块在这里画成图（异步，失败时保留代码块）
    void renderMermaidIn(ref.current);
  }, [html]);

  return (
    <div
      ref={ref}
      className="md"
      onClick={(e) => {
        const a = (e.target as HTMLElement).closest("a");
        if (!a) return;
        const href = a.getAttribute("href") ?? "";
        if (!href) return;
        e.preventDefault();
        onLink?.(href);
      }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

// ---------------------------------------------------------------- 工具卡片

function ToolCard({
  name,
  summary,
  risk,
  result,
  running,
}: {
  name: string;
  summary: string;
  risk: Risk;
  result?: ToolOutcomeView | { content: string; isError: boolean };
  running?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ok = result ? ("ok" in result ? result.ok : !result.isError) : true;
  const preview = result ? ("preview" in result ? result.preview : result.content) : "";
  const denied = result && "denied" in result ? result.denied : false;
  const ms = result && "durationMs" in result ? result.durationMs : 0;

  return (
    <div className={"tool-card" + (running ? " running" : "") + (!ok ? " failed" : "")}>
      <div className="tool-head" onClick={() => setOpen((v) => !v)}>
        {running ? <Spinner /> : <Icon name={denied ? "close" : ok ? "check" : "alert"} size={13} />}
        <span className="tool-name">{name}</span>
        <span className="tool-sum">{summary || "（执行中）"}</span>
        {risk !== "read" && <span className={"risk-pill " + risk}>{risk === "destructive" ? "危险" : "写入"}</span>}
        {ms > 0 && <span className="tool-time">{ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`}</span>}
        <Icon name={open ? "chevron-down" : "chevron-right"} size={12} />
      </div>
      {open && preview && <div className="tool-body">{clampText(preview, 4000)}</div>}
    </div>
  );
}

// ---------------------------------------------------------------- 消息

function MessageView({
  msg,
  results,
  onLink,
}: {
  msg: ChatMessage;
  results: Map<string, ToolOutcomeView | { content: string; isError: boolean }>;
  onLink: (href: string) => void;
}) {
  const user = msg.role === "user";
  const userName = useApp((s) => s.config?.userName ?? "我");

  const blocks: ContentBlock[] = msg.blocks ?? [];
  const thinking = blocks.find((b) => b.type === "thinking") as { type: "thinking"; text: string } | undefined;
  const text = blocks.find((b) => b.type === "text") as { type: "text"; text: string } | undefined;
  const tools = blocks.filter((b) => b.type === "tool_use") as {
    type: "tool_use";
    id: string;
    name: string;
    input: unknown;
  }[];

  return (
    <div className={"msg " + (user ? "user" : "agent")}>
      <div className={"msg-avatar" + (user ? "" : " agent")}>
        {user ? userName.slice(0, 1) : <Icon name="sparkle" size={13} />}
      </div>
      <div className="msg-col">
        {thinking && thinking.text.trim() && (
          <details className="thinking">
            <summary>思考过程</summary>
            <div style={{ marginTop: 6 }}>{thinking.text}</div>
          </details>
        )}

        {text && text.text.trim() && (
          <div className="bubble">
            {user ? <div style={{ whiteSpace: "pre-wrap" }}>{text.text}</div> : <Markdown source={text.text} onLink={onLink} />}
          </div>
        )}

        {tools.map((t) => {
          const r = results.get(t.id);
          // 后端在执行时会算一句人话摘要；回看历史时没有它，用前端的启发式兜底
          const liveSummary = r && "summary" in r && r.summary ? r.summary : null;
          return (
            <ToolCard
              key={t.id}
              name={t.name}
              summary={liveSummary ?? summarizeInput(t.name, t.input)}
              risk={r && "risk" in r ? r.risk : "read"}
              result={r}
            />
          );
        })}

        {!user && (
          <div className="msg-meta">
            <span>{fmtClock(msg.createdAt)}</span>
            {msg.meta?.model && <span>{msg.meta.model}</span>}
            {msg.meta?.inputTokens ? <span>↑{msg.meta.inputTokens} ↓{msg.meta.outputTokens ?? 0}</span> : null}
          </div>
        )}
      </div>
    </div>
  );
}

/** 从工具名 + 入参里猜一句人话（历史消息没有后端摘要时用） */
function summarizeInput(name: string, input: unknown): string {
  const o = (input ?? {}) as Record<string, unknown>;
  const pick = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : undefined);
  switch (name) {
    case "fs_read":
    case "fs_write":
    case "fs_delete":
      return pick("path") ?? "";
    case "fs_list":
      return pick("path") ?? "主题根目录";
    case "fs_search":
      return `检索「${pick("query") ?? ""}」`;
    case "note_create":
      return `写笔记「${pick("title") ?? ""}」`;
    case "card_create":
      return clampText(pick("front") ?? "", 40);
    case "task_create":
      return pick("title") ?? "";
    case "viewer_open":
      return pick("path") ?? pick("url") ?? "";
    case "viewer_read":
      return "读取内置浏览器内容";
    case "viewer_goto":
      return o.page ? `翻到第 ${o.page} 页` : "定位";
    case "viewer_search":
      return `文档内检索「${pick("query") ?? ""}」`;
    case "web_fetch":
      return pick("url") ?? "";
    case "web_search":
      return `联网搜索「${pick("query") ?? ""}」`;
    case "topic_create":
      return `新建主题「${pick("name") ?? ""}」`;
    case "mindmap_create":
      return `思维导图「${pick("title") ?? ""}」`;
    case "quiz_create":
      return `出卷「${pick("title") ?? ""}」`;
    case "kb_build":
      return "刷新知识库";
    case "kb_search":
      return `查讲义「${pick("query") ?? ""}」`;
    case "lesson_plan":
      return `讲解方案「${pick("title") ?? ""}」`;
    case "lesson_step":
      return `更新讲解进度：第 ${o.index ?? "?"} 步`;
    case "material_import":
      return "导入资料到 materials/";
    case "skill_read":
      return `读技能「${pick("name") ?? ""}」`;
    case "skill_list":
      return "查看可用技能";
    case "card_review":
      return `记录复习：${pick("grade") ?? ""}`;
    case "task_update":
      return `更新任务 ${pick("status") ?? ""}`;
    case "session_start":
      return `开始学习会话「${pick("title") ?? ""}」`;
    default:
      return name;
  }
}

// ---------------------------------------------------------------- 主面板

export function Chat() {
  const topic = useApp((s) => s.topic);
  const messages = useApp((s) => s.messages);
  const streaming = useApp((s) => s.streaming);
  const activities = useApp((s) => s.activities);
  const iteration = useApp((s) => s.iteration);
  const error = useApp((s) => s.chatError);
  const approval = useApp((s) => s.approval);
  const setView = useApp((s) => s.setView);

  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // 结果索引：历史（tool 消息）+ 本轮实时（activities）
  const results = useMemo(() => {
    const map = new Map<string, ToolOutcomeView | { content: string; isError: boolean }>();
    for (const m of messages) {
      if (m.role !== "tool") continue;
      for (const b of m.blocks) {
        if (b.type === "tool_result") map.set(b.tool_use_id, { content: b.content, isError: b.is_error });
      }
    }
    for (const a of activities) map.set(a.callId, a);
    return map;
  }, [messages, activities]);

  const visible = useMemo(
    () =>
      messages.filter((m) => {
        if (m.role === "tool") return false;
        // 只调了工具、没有文本的助手消息不单独渲染（工具卡片挂在它上面）
        if (m.role === "assistant") {
          const hasText = m.blocks.some((b) => b.type === "text" && b.text.trim());
          const hasThinking = m.blocks.some((b) => b.type === "thinking" && b.text.trim());
          const hasTools = m.blocks.some((b) => b.type === "tool_use");
          return hasText || hasThinking || hasTools;
        }
        return true;
      }),
    [messages],
  );

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (pinned.current) el.scrollTop = el.scrollHeight;
  }, [messages, streaming?.text, streaming?.thinking, activities.length]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  const onLink = (href: string) => {
    if (/^https?:\/\//i.test(href)) void useApp.getState().openUrl(href);
    else if (!href.startsWith("#")) useApp.getState().toast("info", href);
  };

  return (
    <div className="chat">
      <ChatHead />

      <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
        {visible.length === 0 && !streaming ? (
          topic?.meta.name ? (
            <TopicWelcome topicName={topic.meta.name} />
          ) : (
            <Welcome topicName={null} />
          )
        ) : (
          <div className="msg-wrap">
            {visible.map((m) => (
              <MessageView key={m.id} msg={m} results={results} onLink={onLink} />
            ))}

            {/* 本轮工具活动（尚未落进消息的） */}
            {streaming &&
              activities
                .filter((a) => a.running)
                .map((a) => (
                  <div className="msg agent" key={"live-" + a.callId}>
                    <div className="msg-avatar agent">
                      <Icon name="sparkle" size={13} />
                    </div>
                    <div className="msg-col">
                      <ToolCardForActivity a={a} />
                    </div>
                  </div>
                ))}

            {streaming && (
              <div className="msg agent">
                <div className="msg-avatar agent">
                  <Icon name="sparkle" size={13} />
                </div>
                <div className="msg-col">
                  {streaming.thinking.trim() && (
                    <details className="thinking" open={!streaming.text.trim()}>
                      <summary>思考中…</summary>
                      <div style={{ marginTop: 6 }}>{streaming.thinking}</div>
                    </details>
                  )}
                  {streaming.text ? (
                    <div className="bubble">
                      <Markdown source={streaming.text} onLink={onLink} />
                      <span className="typing-caret" />
                    </div>
                  ) : (
                    !streaming.thinking && (
                      <div className="row" style={{ color: "var(--text-faint)", gap: 8 }}>
                        <Spinner />
                        <span style={{ fontSize: 12.5 }}>
                          {iteration && iteration.index > 1
                            ? `第 ${iteration.index}/${iteration.max} 轮工具循环…`
                            : "思考中…"}
                        </span>
                      </div>
                    )
                  )}
                </div>
              </div>
            )}

            {error && (
              <div className="msg agent">
                <div className="msg-avatar agent" style={{ background: "var(--danger-soft)", color: "var(--danger)", borderColor: "transparent" }}>
                  <Icon name="alert" size={13} />
                </div>
                <div className="msg-col">
                  <div className="tool-card failed">
                    <div className="tool-head" style={{ cursor: "default" }}>
                      <Icon name="alert" size={13} />
                      <span className="tool-sum">出错了：{error}</span>
                    </div>
                  </div>
                  <div className="row">
                    <button className="btn sm" onClick={() => setView("settings")}>
                      去检查模型设置
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      <Composer />
      {approval && <ApprovalDialog />}
    </div>
  );
}

function ToolCardForActivity({ a }: { a: ToolActivity }) {
  return <ToolCard name={a.name} summary={a.summary} risk={a.risk} result={a} running={a.running} />;
}

function ChatHead() {
  const topic = useApp((s) => s.topic);
  const setStage = useApp((s) => s.setStage);
  const newChat = useApp((s) => s.newChat);
  const session = useApp((s) => s.topic?.currentSession ?? null);
  const updateTopic = useApp((s) => s.updateTopic);

  return (
    <div className="chat-head">
      <div className="who">
        <Icon name="chat" size={14} />
        <span>{topic ? topic.meta.name : "日常问答"}</span>
        {topic && <span className="sub">· {topic.path}</span>}
      </div>
      <div className="spacer" />

      {session && (
        <span className="tag accent" title={`正在进行的学习会话：${session.title}`}>
          会话中 · {Math.max(1, Math.round((Date.now() - new Date(session.startedAt).getTime()) / 60000))} 分钟
        </span>
      )}

      {topic && (
        <Dropdown
          up={false}
          trigger={() => (
            <button className="pill-select" title="切换学习阶段（会改变 agent 的讲法）">
              <Icon name="target" size={13} />
              {STAGE_LABEL[topic.meta.stage]}阶段
              <Icon name="chevron-down" size={12} />
            </button>
          )}
        >
          {(close) => (
            <>
              <MenuLabel>学习阶段</MenuLabel>
              {(Object.keys(STAGE_LABEL) as StudyStage[]).map((s) => (
                <MenuItem
                  key={s}
                  selected={s === topic.meta.stage}
                  onClick={() => {
                    void setStage(s);
                    close();
                  }}
                >
                  {STAGE_LABEL[s]}
                </MenuItem>
              ))}
              <MenuSep />
              <MenuItem
                onClick={() => {
                  void updateTopic({});
                  close();
                }}
              >
                <Icon name="refresh" size={13} /> 刷新主题信息
              </MenuItem>
            </>
          )}
        </Dropdown>
      )}

      <button className="icon-btn" title="新对话" onClick={() => void newChat()}>
        <Icon name="plus" />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- 输入区

function Composer() {
  const send = useApp((s) => s.send);
  const stop = useApp((s) => s.stop);
  const streaming = useApp((s) => s.streaming);
  const config = useApp((s) => s.config);
  const patchConfig = useApp((s) => s.patchConfig);
  const topic = useApp((s) => s.topic);
  const toast = useApp((s) => s.toast);

  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<string[]>([]);
  const [importing, setImporting] = useState(false);

  const active = config?.profiles.find((p) => p.id === config.activeProfileId) ?? null;
  const mode = config?.agent.permissionMode ?? "ask";

  async function submit() {
    if (!text.trim() || streaming) return;
    const t = text;
    setText("");
    setAttachments([]);
    await send(t, attachments);
  }

  async function pickFiles() {
    if (!topic) {
      toast("warn", "先在左侧选一个主题，资料会导入到它的 materials/ 目录");
      return;
    }
    try {
      const picked = await openDialog({ multiple: true, title: "选择要加入主题的资料" });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      if (paths.length === 0) return;
      setImporting(true);
      const n = await useApp.getState().importMaterials(paths as string[]);
      setImporting(false);
      if (n > 0) {
        // 导入成功后按文件名挂到消息上（agent 看到的是相对路径提示）
        const detail = await api.topicGet(topic.slug);
        const latest = detail.materials.slice(0, n).map((m) => m.path);
        setAttachments((a) => [...new Set([...a, ...latest])]);
      }
    } catch (e) {
      setImporting(false);
      toast("error", errText(e));
    }
  }

  return (
    <div className="composer-wrap">
      <div className="composer">
        {attachments.length > 0 && (
          <div className="row wrap" style={{ padding: "8px 12px 0", gap: 6 }}>
            {attachments.map((a) => (
              <span key={a} className="tag" title={a}>
                <Icon name="file" size={11} /> {a.split("/").pop()}
                <button
                  className="icon-btn"
                  style={{ width: 16, height: 16, marginLeft: 4 }}
                  onClick={() => setAttachments((x) => x.filter((v) => v !== a))}
                >
                  <Icon name="close" size={10} />
                </button>
              </span>
            ))}
          </div>
        )}

        <AutoTextarea
          value={text}
          onChange={setText}
          onSend={submit}
          placeholder={
            topic
              ? `关于「${topic.meta.name}」，想问什么？（Enter 发送，Shift+Enter 换行）`
              : "说点什么，或者直接说要学什么（Enter 发送）"
          }
        />

        <div className="composer-bar">
          <button
            className="pill-select"
            title="导入资料：把课件 / 论文 / 截图复制进主题的 materials/（直接拖进窗口也行）"
            onClick={pickFiles}
            disabled={importing}
          >
            {importing ? <Spinner /> : <Icon name="download" size={13} />}
            <span className="ellip">资料</span>
          </button>

          <Dropdown
            trigger={() => (
              <button className={"pill-select" + (mode === "full" ? " warn" : "")} title="工具权限">
                <Icon name="eye" size={13} />
                <span className="ellip">{PERMISSION_LABEL[mode]}</span>
                <Icon name="chevron-down" size={12} />
              </button>
            )}
          >
            {(close) => (
              <>
                <MenuLabel>工具权限</MenuLabel>
                {(config?.agent ? (["ask", "auto_edit", "full"] as PermissionMode[]) : []).map((m) => (
                  <MenuItem
                    key={m}
                    selected={m === mode}
                    onClick={() => {
                      void patchConfig({ permissionMode: m });
                      close();
                    }}
                  >
                    {PERMISSION_LABEL[m]}
                    <span className="muted" style={{ marginLeft: 8, fontSize: 11 }}>
                      {m === "ask" ? "写入前先问" : m === "auto_edit" ? "只拦危险操作" : "不打断"}
                    </span>
                  </MenuItem>
                ))}
              </>
            )}
          </Dropdown>

          <Dropdown
            trigger={() => (
              <button className="pill-select" title="切换模型">
                <span className="ellip">{active ? `${active.name} / ${active.model}` : "未配置模型"}</span>
                <Icon name="chevron-down" size={12} />
              </button>
            )}
          >
            {(close) => (
              <>
                <MenuLabel>模型档案</MenuLabel>
                {(config?.profiles ?? []).map((p) => (
                  <MenuItem
                    key={p.id}
                    selected={p.id === config?.activeProfileId}
                    onClick={() => {
                      void patchConfig({ activeProfileId: p.id });
                      close();
                    }}
                  >
                    <span className="grow">
                      {p.name}
                      <span className="muted" style={{ marginLeft: 6, fontSize: 11 }}>
                        {p.model}
                      </span>
                    </span>
                    {!p.hasApiKey && <span className="tag">缺 Key</span>}
                  </MenuItem>
                ))}
                <MenuSep />
                <MenuItem
                  onClick={() => {
                    useApp.getState().setView("settings");
                    close();
                  }}
                >
                  <Icon name="settings" size={13} /> 管理模型档案…
                </MenuItem>
              </>
            )}
          </Dropdown>

          <div className="spacer" />

          {active && !active.supportsTools && (
            <span className="muted" style={{ fontSize: 11.5 }} title="该模型未开启工具调用，agent 会把工具建议写进回复">
              工具降级模式
            </span>
          )}

          {streaming ? (
            <button className="send-btn stop" title="停止生成" onClick={() => void stop()}>
              <Icon name="square" size={12} />
            </button>
          ) : (
            <button className="send-btn" title={`发送 (${hotkey("Enter")})`} onClick={submit} disabled={!text.trim()}>
              <Icon name="arrow-up" size={14} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 审批

function ApprovalDialog() {
  const pending = useApp((s) => s.approval);
  const approve = useApp((s) => s.approve);
  if (!pending) return null;

  // 越权申请：agent 想碰工作区之外的文件
  if (pending.kind === "sandbox") {
    const r = pending.request;
    return (
      <Modal
        title="agent 想访问工作区之外的文件"
        icon="alert"
        onClose={() => void approve(false, false)}
        footer={
          <>
            <span className="left muted" style={{ fontSize: 11.5 }}>
              批准后该目录长期有效，可在设置里撤销
            </span>
            <button className="btn" onClick={() => void approve(false, false)}>
              拒绝
            </button>
            <button className="btn primary" onClick={() => void approve(true, true)}>
              允许访问这个目录
            </button>
          </>
        }
      >
        <div className="col" style={{ gap: 10 }}>
          <div>
            agent 想{SANDBOX_MODE_LABEL[r.mode] ?? r.mode}：
            <code className="mono" style={{ marginLeft: 6 }}>
              {r.path}
            </code>
          </div>
          <div className="sub" style={{ fontSize: 12.5 }}>
            用途：{r.reason || "（未说明）"}
          </div>
          <div className="card-box" style={{ background: "var(--bg-sub)", padding: 12, gap: 6 }}>
            <div className="muted" style={{ fontSize: 12 }}>
              将被授权访问的目录（含其子目录）：
            </div>
            <code className="mono" style={{ fontSize: 12 }}>
              {r.root}
            </code>
          </div>
          <div className="muted" style={{ fontSize: 11.5 }}>
            如果你只是想让 agent 读某份讲义，更稳妥的做法是先把文件放进该主题的 materials/ 目录。
          </div>
        </div>
      </Modal>
    );
  }

  const call = pending.call;
  return (
    <Modal
      title={
        <span className="row" style={{ gap: 8 }}>
          agent 想要{call.riskLabel}：
          <span className="mono">{call.name}</span>
        </span>
      }
      icon="alert"
      onClose={() => void approve(false, false)}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            风险等级：{RISK_LABEL[call.risk]}
          </span>
          <button className="btn" onClick={() => void approve(false, true)}>
            拒绝
          </button>
          <button className="btn" onClick={() => void approve(true, false)}>
            允许一次
          </button>
          <button className="btn primary" onClick={() => void approve(true, true)}>
            本次运行内始终允许 {call.name}
          </button>
        </>
      }
    >
      <div style={{ fontSize: 13.5 }}>{call.summary}</div>
      <details>
        <summary className="muted" style={{ cursor: "pointer", fontSize: 12 }}>
          查看完整参数
        </summary>
        <pre className="tool-body" style={{ marginTop: 8, borderRadius: 8 }}>
          {JSON.stringify(call.input, null, 2)}
        </pre>
      </details>
    </Modal>
  );
}

export { toolSummaryOf };

function toolSummaryOf(name: string, input: unknown): string {
  return summarizeInput(name, input);
}
