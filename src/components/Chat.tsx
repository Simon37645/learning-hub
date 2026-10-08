// 对话面板：消息流、工具卡片、审批弹窗、输入框。

import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useApp, type ToolActivity } from "../store/app";
import { api, errText } from "../lib/api";
import { highlightWithin, linkifyCitations, renderMarkdown, renderMermaidIn } from "../lib/markdown";
import { clampText, fmtClock, fmtTokens, hotkey } from "../lib/format";
import { imageFilesFrom, prepareImage } from "../lib/images";
import { contextShares, summarizeCache } from "../lib/usage";
import { LessonSteps } from "./Lesson";
import {
  EFFORT_HINT,
  EFFORT_LABEL,
  PERMISSION_LABEL,
  RISK_LABEL,
  STYLE_LABEL,
  SANDBOX_MODE_LABEL,
  STAGE_LABEL,
  type AgentMode,
  type ChatMessage,
  type ContentBlock,
  type ImageUpload,
  type PermissionMode,
  type ReasoningEffort,
  type ReasoningStyle,
  type Risk,
  type StudyStage,
  type ToolOutcomeView,
} from "../lib/types";
import { Dropdown, Icon, MenuItem, MenuLabel, MenuSep, Modal, Spinner, AutoTextarea } from "./ui";
import { TopicWelcome, Welcome } from "./Home";

/** 一条消息最多带几张图（再多就该想想是不是该做成资料了）。 */
const MAX_IMAGES = 8;

/** 附件（工作区相对路径）→ 能直接塞进 <img> 的地址。 */
export function assetUrl(workspaceRoot: string, rel: string): string {
  const sep = workspaceRoot.includes("\\") ? "\\" : "/";
  return convertFileSrc(workspaceRoot.replace(/[\\/]+$/, "") + sep + rel.replace(/^[\\/]+/, ""));
}

/** 输入框里还没发出去的一张图（`preview` 就是它的 base64，直接当缩略图用）。 */
interface PendingImage {
  upload: ImageUpload;
  preview: string;
}

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
  onImage,
}: {
  msg: ChatMessage;
  results: Map<string, ToolOutcomeView | { content: string; isError: boolean }>;
  onLink: (href: string) => void;
  onImage: (block: ContentBlock) => void;
}) {
  const user = msg.role === "user";
  const userName = useApp((s) => s.config?.userName ?? "我");
  const workspaceRoot = useApp((s) => s.config?.workspaceRoot ?? "");

  const blocks: ContentBlock[] = msg.blocks ?? [];
  const thinking = blocks.find((b) => b.type === "thinking") as { type: "thinking"; text: string } | undefined;
  const text = blocks.find((b) => b.type === "text") as { type: "text"; text: string } | undefined;
  const images = blocks.filter((b) => b.type === "image");
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

        {images.length > 0 && (
          <div className={"msg-imgs" + (user ? " user" : "")}>
            {images.map((b, i) =>
              b.type === "image" ? (
                <button
                  key={b.path + i}
                  className="msg-img"
                  title={`${b.name || "图片"}（点击放大）`}
                  onClick={() => onImage(b)}
                >
                  <img src={assetUrl(workspaceRoot, b.path)} alt={b.name || "图片"} loading="lazy" />
                </button>
              ) : null,
            )}
          </div>
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

/** 点开一张图看大图。图片是用户自己贴上来的，所以这里不需要缩放以外的操作。 */
function ImageLightbox({ block, onClose }: { block: ContentBlock; onClose: () => void }) {
  const workspaceRoot = useApp((s) => s.config?.workspaceRoot ?? "");
  const [zoom, setZoom] = useState(1);
  if (block.type !== "image") return null;

  return (
    <div className="img-lightbox" onClick={onClose}>
      <div className="img-lightbox-inner" onClick={(e) => e.stopPropagation()}>
        <div className="img-lightbox-bar">
          <span className="ellip">{block.name || "图片"}</span>
          <span className="muted">
            {block.width && block.height ? `${block.width}×${block.height} · ` : ""}
            {block.bytes > 0 ? fmtBytes(block.bytes) : ""}
          </span>
          <div className="spacer" />
          <button className="icon-btn" title="缩小" onClick={() => setZoom((z) => Math.max(0.2, z - 0.25))}>
            <Icon name="zoom-out" size={14} />
          </button>
          <span className="muted mono" style={{ fontSize: 11 }}>
            {Math.round(zoom * 100)}%
          </span>
          <button className="icon-btn" title="放大" onClick={() => setZoom((z) => Math.min(4, z + 0.25))}>
            <Icon name="zoom-in" size={14} />
          </button>
          <button className="icon-btn" title="关闭（Esc）" onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <div className="img-lightbox-stage">
          <img
            src={assetUrl(workspaceRoot, block.path)}
            alt={block.name || "图片"}
            style={{ transform: `scale(${zoom})` }}
          />
        </div>
      </div>
    </div>
  );
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
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

export function Chat({ mode = "study", welcome }: { mode?: AgentMode; welcome?: React.ReactNode }) {
  const topic = useApp((s) => s.topic);
  const messages = useApp((s) => s.messages);
  const streaming = useApp((s) => s.streaming);
  const activities = useApp((s) => s.activities);
  const iteration = useApp((s) => s.iteration);
  const error = useApp((s) => s.chatError);
  const approval = useApp((s) => s.approval);
  const setView = useApp((s) => s.setView);
  const [lightbox, setLightbox] = useState<ContentBlock | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // 大图浮层：Esc 关掉（和其它浮层保持一致的手感）
  useEffect(() => {
    if (!lightbox) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setLightbox(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [lightbox]);

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
      <ChatHead mode={mode} />
      <LessonSteps />

      <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
        {visible.length === 0 && !streaming ? (
          welcome ?? (topic?.meta.name ? <TopicWelcome topicName={topic.meta.name} /> : <Welcome topicName={null} />)
        ) : (
          <div className="msg-wrap">
            {visible.map((m) => (
              <MessageView
                key={m.id}
                msg={m}
                results={results}
                onLink={onLink}
                onImage={setLightbox}
              />
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

      <Composer mode={mode} />
      {approval && <ApprovalDialog />}
      {lightbox && <ImageLightbox block={lightbox} onClose={() => setLightbox(null)} />}
    </div>
  );
}

function ToolCardForActivity({ a }: { a: ToolActivity }) {
  return <ToolCard name={a.name} summary={a.summary} risk={a.risk} result={a} running={a.running} />;
}

function ChatHead({ mode = "study" }: { mode?: AgentMode }) {
  const topic = useApp((s) => s.topic);
  const setStage = useApp((s) => s.setStage);
  const newChat = useApp((s) => s.newChat);
  const newStudioChat = useApp((s) => s.newStudioChat);
  const session = useApp((s) => s.topic?.currentSession ?? null);
  const updateTopic = useApp((s) => s.updateTopic);
  const studio = mode === "studio";

  return (
    <div className="chat-head">
      <div className="who">
        <Icon name={studio ? "hammer" : "chat"} size={14} />
        <span>{studio ? "工坊" : topic ? topic.meta.name : "日常问答"}</span>
        {studio ? (
          <span className="sub">· 造技能与 MCP 服务器，不跟任何主题挂钩</span>
        ) : (
          topic && <span className="sub">· {topic.path}</span>
        )}
      </div>
      <div className="spacer" />

      {session && (
        <span className="tag accent" title={`正在进行的学习会话：${session.title}`}>
          会话中 · {Math.max(1, Math.round((Date.now() - new Date(session.startedAt).getTime()) / 60000))} 分钟
        </span>
      )}

      {topic && !studio && (
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

      <button
        className="icon-btn"
        title={studio ? "新对话（工坊）" : "新对话"}
        onClick={() => void (studio ? newStudioChat() : newChat())}
      >
        <Icon name="plus" />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- 用量徽标

/** 对话整体缓存命中率：平时一枚小胶囊，光标移上去展开明细。
 *
 * 数字只认**服务商在流里报的真实用量**（provider 层解析的 usage）。服务商不报的轮次
 * 不计入，一轮都没有就整块不显示——宁可没有，也不给一个假的 0%。
 * 「整体」= 这条对话所有报过用量的轮次里，命中 token / 输入 token 的累计值。 */
export function CacheBadge() {
  const messages = useApp((s) => s.messages);
  const usage = useApp((s) => s.usage);
  const [open, setOpen] = useState(false);

  const sum = useMemo(() => summarizeCache(messages), [messages]);
  const parts = useMemo(() => contextShares(usage?.context ?? []), [usage]);

  if (sum.turns === 0) return null;

  return (
    <div
      className="cache-badge"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button className="pill-select" title="这条对话整体的缓存命中率（光标移上去看明细）">
        <Icon name="target" size={13} />
        <span className="ellip">缓存 {sum.rate.toFixed(1)}%</span>
      </button>
      {open && (
        <div className="cache-pop">
          <div className="cp-head">
            <span>上下文构成</span>
            <span className="muted">
              输入 {fmtTokens(usage?.input ?? 0)} · 输出 {fmtTokens(usage?.output ?? 0)}
            </span>
          </div>
          {parts.length > 0 && (
            <>
              <div className="cp-bar">
                {parts.map((p, i) => (
                  <i key={p.label} style={{ width: `${p.share}%`, background: partColor(i) }} />
                ))}
              </div>
              {parts.map((p, i) => (
                <div className="cp-row" key={p.label}>
                  <span className="cp-label">
                    <span className="dot" style={{ background: partColor(i) }} />
                    {p.label}
                  </span>
                  <span>{p.share.toFixed(1)}%</span>
                </div>
              ))}
            </>
          )}
          <div className="cp-sep" />
          <div className="cp-row cp-strong">
            <span>缓存命中率（本对话累计）</span>
            <span>{sum.rate.toFixed(1)}%</span>
          </div>
          <div className="cp-row">
            <span>
              命中 {fmtTokens(sum.cached)} / 输入 {fmtTokens(sum.input)}
            </span>
            <span>{sum.turns} 轮</span>
          </div>
          <div className="cp-hint">
            命中率由服务商返回的用量算出（OpenAI 需端点支持 stream_options）；
            「上下文构成」是按字符估的比例。
          </div>
        </div>
      )}
    </div>
  );
}

/** 上下文各块的颜色：同一支强调色按比例调深浅，不引入新色板 */
function partColor(i: number): string {
  return `color-mix(in srgb, var(--accent) ${Math.max(12, 100 - i * 18)}%, var(--border-strong))`;
}

// ---------------------------------------------------------------- 输入区

function Composer({ mode = "study" }: { mode?: AgentMode }) {
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
  const [images, setImages] = useState<PendingImage[]>([]);
  const [reading, setReading] = useState(false);

  const active = config?.profiles.find((p) => p.id === config.activeProfileId) ?? null;
  const profile = active;
  const permMode = config?.agent.permissionMode ?? "ask";
  const studio = mode === "studio";

  async function submit() {
    if ((!text.trim() && images.length === 0) || streaming) return;
    const t = text;
    const pics = images.map((i) => i.upload);
    setText("");
    setAttachments([]);
    setImages([]);
    await send(t, attachments, pics);
  }

  /**
   * 把若干张图收进待发送队列。
   *
   * 缩放与编码在 `lib/images.ts` 里做（长边 1568px 以上只会更贵、不会更清楚），
   * 这里只管数量上限与错误提示——一张图读不出来不该让另外几张也发不出去。
   */
  async function addImages(files: File[]) {
    const room = MAX_IMAGES - images.length;
    if (room <= 0) {
      toast("warn", `一条消息最多带 ${MAX_IMAGES} 张图`);
      return;
    }
    const take = files.slice(0, room);
    if (files.length > room) toast("warn", `一条消息最多带 ${MAX_IMAGES} 张图，多余的没有加`);
    setReading(true);
    const added: PendingImage[] = [];
    for (const file of take) {
      try {
        const p = await prepareImage(file, file.name || "粘贴的图片.png");
        added.push({ upload: p.upload, preview: p.preview });
      } catch (e) {
        toast("error", `${file.name || "这张图"}：${errText(e)}`);
      }
    }
    setReading(false);
    if (added.length > 0) setImages((prev) => [...prev, ...added]);
  }

  async function pickImages() {
    try {
      const picked = await openDialog({
        multiple: true,
        title: "选择图片（会随消息发给模型）",
        filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const files: File[] = [];
      for (const p of paths as string[]) {
        try {
          // 文件选择框给的是路径，字节要经后端读一遍（沙箱只认主题目录，读不了任意路径）
          const payload = await api.agentImageLoad(p);
          const bytes = Uint8Array.from(atob(payload.data), (c) => c.charCodeAt(0));
          files.push(new File([bytes], payload.name, { type: payload.mediaType }));
        } catch (e) {
          toast("error", errText(e));
        }
      }
      if (files.length > 0) await addImages(files);
    } catch (e) {
      toast("error", errText(e));
    }
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
        {images.length > 0 && (
          <div className="composer-imgs">
            {images.map((img, i) => (
              <div className="composer-img" key={img.upload.name + i}>
                <img src={img.preview} alt={img.upload.name} />
                <button
                  className="img-x"
                  title="移除这张图"
                  onClick={() => setImages((prev) => prev.filter((_, j) => j !== i))}
                >
                  <Icon name="close" size={10} />
                </button>
              </div>
            ))}
            <div className="composer-imgs-note muted">
              {reading ? <Spinner /> : `${images.length} 张图会随这条消息发给模型`}
            </div>
          </div>
        )}

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
          onPaste={(e) => {
            // 截图直接粘进来是最常用的一条路（Win+Shift+S 之后 Ctrl+V）
            const files = imageFilesFrom(e.clipboardData?.items);
            if (files.length === 0) return;
            e.preventDefault();
            void addImages(files);
          }}
          placeholder={
            studio
              ? "想造什么？例如「做一个查英语词根的技能」（Enter 发送，Ctrl+V 贴图）"
              : topic
                ? `关于「${topic.meta.name}」，想问什么？（Enter 发送，Shift+Enter 换行，Ctrl+V 贴图）`
                : "说点什么，或者直接说要学什么（Enter 发送，Ctrl+V 贴图）"
          }
        />

        <div className="composer-bar">
          <button
            className="pill-select"
            title="贴图片：把截图 / 照片随消息发给模型（也可以直接 Ctrl+V 粘贴）"
            onClick={pickImages}
            disabled={reading}
          >
            {reading ? <Spinner /> : <Icon name="image" size={13} />}
            <span className="ellip">图片</span>
          </button>

          {!studio && (
            <button
              className="pill-select"
              title="导入资料：把课件 / 论文 / 讲义复制进主题的 materials/（直接拖进窗口也行）"
              onClick={pickFiles}
              disabled={importing}
            >
              {importing ? <Spinner /> : <Icon name="download" size={13} />}
              <span className="ellip">资料</span>
            </button>
          )}

          <Dropdown
            trigger={() => (
              <button className={"pill-select" + (permMode === "full" ? " warn" : "")} title="工具权限">
                <Icon name="eye" size={13} />
                <span className="ellip">{PERMISSION_LABEL[permMode]}</span>
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
                    selected={m === permMode}
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

          {/* 用量：这条对话整体缓存命中多少（光标移上去看上下文构成） */}
          <CacheBadge />

          {/* 思考强度：和模型选择放在一起，随时能调 */}
          <Dropdown
            trigger={() => (
              <button
                className="pill-select"
                title="思考强度：让模型想多久（各家写法的兼容性不同，可在设置里改发送方式）"
              >
                <Icon name="sparkle" size={13} />
                <span className="ellip">思考 {EFFORT_LABEL[profile?.reasoning?.effort ?? "off"]}</span>
                <Icon name="chevron-down" size={12} />
              </button>
            )}
          >
            {(close) => (
              <>
                <MenuLabel>思考强度</MenuLabel>
                {(["off", "low", "medium", "high", "max"] as ReasoningEffort[]).map((e) => (
                  <MenuItem
                    key={e}
                    selected={(profile?.reasoning?.effort ?? "off") === e}
                    onClick={() => {
                      void useApp.getState().setReasoning(e);
                      close();
                    }}
                  >
                    <span className="grow">
                      {EFFORT_LABEL[e]}
                      <span className="muted" style={{ marginLeft: 8, fontSize: 11 }}>
                        {EFFORT_HINT[e]}
                      </span>
                    </span>
                  </MenuItem>
                ))}
                <MenuSep />
                <MenuItem
                  onClick={async () => {
                    const cur = profile?.reasoning?.style ?? "auto";
                    const next: ReasoningStyle =
                      cur === "auto"
                        ? "openai_effort"
                        : cur === "openai_effort"
                          ? "qwen_thinking"
                          : cur === "qwen_thinking"
                            ? "anthropic_thinking"
                            : cur === "anthropic_thinking"
                              ? "none"
                              : "auto";
                    await useApp.getState().setReasoning(profile?.reasoning?.effort ?? "off", next);
                    close();
                  }}
                >
                  <Icon name="refresh" size={13} /> 发送方式：{STYLE_LABEL[profile?.reasoning?.style ?? "auto"]}
                </MenuItem>
                <MenuLabel>
                  有的服务商不认这些参数；报错就点上面把它切成「不发」
                </MenuLabel>
              </>
            )}
          </Dropdown>

          {active && !active.supportsTools && (
            <span className="muted" style={{ fontSize: 11.5 }} title="该模型未开启工具调用，agent 会把工具建议写进回复">
              工具降级模式
            </span>
          )}

          {active && !active.supportsVision && (
            <span
              className="muted"
              style={{ fontSize: 11.5 }}
              title="这个模型档案标着「不支持图片输入」，贴的图不会发给它（会换成一行文字说明）。可在「设置 → 模型档案」里打开"
            >
              图片不会发出
            </span>
          )}

          {streaming ? (
            <button className="send-btn stop" title="停止生成" onClick={() => void stop()}>
              <Icon name="square" size={12} />
            </button>
          ) : (
            <button
              className="send-btn"
              title={`发送 (${hotkey("Enter")})`}
              onClick={submit}
              disabled={!text.trim() && images.length === 0}
            >
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
