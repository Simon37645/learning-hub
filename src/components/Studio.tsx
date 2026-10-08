// 工坊：独立于学习的一个 agent 模式——照着内置规范造技能（Skill）与 MCP 服务器。
//
// 为什么要有这个模式（而不是把造能力的活塞进学习对话里）：
// 造技能/服务器需要的是「读规范 → 写文件 → 发布 → 看连接状态」这套循环，
// 和学习一门课用的工具、提示词、上下文都不一样；混在一起两边都会被拖累。
// 界面上它就是「另一条对话线」：自己的对话清单（挂在侧栏「工坊」下面）、
// 自己的练习目录、自己的工具表。

import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { useApp } from "../store/app";
import { Chat, Markdown } from "./Chat";
import { Icon, Modal, Spinner } from "./ui";
import type { StudioInfo } from "../lib/types";

export function Studio() {
  const studio = useApp((s) => s.studio);
  const loadStudio = useApp((s) => s.loadStudio);
  const toast = useApp((s) => s.toast);
  /** 打开的浮层：规范文档或系统提示词 */
  const [sheet, setSheet] = useState<{ title: string; doc?: string; text?: string } | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    void loadStudio();
  }, [loadStudio]);

  const openDoc = async (doc: string, title: string) => {
    setSheet({ title, doc });
  };

  const previewPrompt = async () => {
    setLoading(true);
    try {
      setSheet({ title: "系统提示词（工坊模式）", text: await api.promptPreview(null, "studio") });
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setLoading(false);
    }
  };

  const openDir = async () => {
    try {
      const dir = await api.studioOpenDir();
      toast("info", `已在文件管理器中打开：${dir}`);
    } catch (e) {
      toast("error", errText(e));
    }
  };

  return (
    <div className="studio">
      <div className="studio-bar">
        <span className="studio-badge">
          <Icon name="hammer" size={13} /> 工坊
        </span>
        <span className="muted" style={{ fontSize: 11.5 }}>
          不跟任何主题挂钩：读规范、写文件、发布，产出直接装进这个应用
        </span>
        <div className="spacer" />
        {(studio?.specs ?? []).map((s) => (
          <button
            key={s.doc}
            className="pill-select"
            title={`${s.hint}（约 ${s.chars} 字）`}
            onClick={() => void openDoc(s.doc, s.title)}
          >
            <Icon name="book" size={13} />
            <span className="ellip">{s.title}</span>
          </button>
        ))}
        <button className="pill-select" title="打开工坊的练习目录（造到一半的东西都在这儿）" onClick={openDir}>
          <Icon name="folder" size={13} />
          <span className="ellip">练习目录</span>
        </button>
        <button className="pill-select" title="看看 agent 在这个模式下拿到了什么提示词与工具" onClick={previewPrompt}>
          {loading ? <Spinner /> : <Icon name="eye" size={13} />}
          <span className="ellip">提示词</span>
        </button>
      </div>

      <Chat mode="studio" welcome={<StudioWelcome studio={studio} onOpenDoc={openDoc} />} />

      {sheet && <SheetModal sheet={sheet} onClose={() => setSheet(null)} />}
    </div>
  );
}

/** 空对话时的开场：把「工坊能干什么、怎么用、产出装到哪」讲清楚。 */
function StudioWelcome({
  studio,
  onOpenDoc,
}: {
  studio: StudioInfo | null;
  onOpenDoc: (doc: string, title: string) => void;
}) {
  const examples = [
    "把我讲课时反复要问的那几个问题做成一个技能",
    "做一个查英语词根的 MCP 服务器：给它一个词，它给我词根拆解",
    "帮我做一个技能：每次读完讲义都按固定格式整理成问答卡片",
  ];
  return (
    <div className="empty" style={{ padding: "30px 24px", gap: 14, maxWidth: 760, margin: "0 auto" }}>
      <div className="home-mark">
        <Icon name="hammer" size={24} />
      </div>
      <h1 className="home-title" style={{ fontSize: 20 }}>
        工坊
      </h1>
      <div className="home-sub" style={{ maxWidth: 620 }}>
        这里不学具体课程。你说要什么，agent 会先读内置的规范文档，再把技能或 MCP 服务器写出来、
        发布进应用——发布之后侧栏的「技能」「MCP 服务器」面板里就能看到它们，学习对话里也会立刻用上。
      </div>

      <div className="grid-2" style={{ width: "100%", gap: 10, textAlign: "left" }}>
        <div className="panel">
          <div className="panel-head">
            <Icon name="puzzle" size={13} />
            <span className="title">技能</span>
          </div>
          <div className="panel-body" style={{ lineHeight: 1.7 }}>
            一份 <code className="mono">SKILL.md</code>：写清「某类任务该怎么做」。
            平时只把名字和一句话说明放进提示词，需要时 agent 再读正文，所以几乎不占上下文。
          </div>
        </div>
        <div className="panel">
          <div className="panel-head">
            <Icon name="plug" size={13} />
            <span className="title">MCP 服务器</span>
          </div>
          <div className="panel-body" style={{ lineHeight: 1.7 }}>
            一个本地小进程，用 JSON-RPC 说话。发布后它的工具会以{" "}
            <code className="mono">mcp__名字__工具</code> 出现在 agent 的工具表里。
          </div>
        </div>
      </div>

      <div className="panel" style={{ width: "100%", textAlign: "left" }}>
        <div className="panel-head">
          <Icon name="folder" size={13} />
          <span className="title">练习目录</span>
        </div>
        <div className="panel-body" style={{ lineHeight: 1.7 }}>
          agent 在这个模式下只能写{" "}
          <code className="mono">{studio?.dir ?? "工作区/.hub/workshop"}</code>，
          发布之后才会进正式的技能目录与配置——它试错弄不坏你已有的东西。
        </div>
      </div>

      <div className="row wrap" style={{ gap: 6, justifyContent: "center" }}>
        <span className="muted" style={{ fontSize: 12 }}>
          试试这么说：
        </span>
        {examples.map((e) => (
          <span key={e} className="chip" title="照抄到下面的输入框里">
            {e}
          </span>
        ))}
      </div>

      <div className="row" style={{ gap: 8 }}>
        <button className="btn sm" onClick={() => onOpenDoc("skill", "技能规范")}>
          <Icon name="book" size={13} /> 先看看技能规范
        </button>
        <button className="btn sm" onClick={() => onOpenDoc("mcp", "MCP 规范")}>
          <Icon name="book" size={13} /> 先看看 MCP 规范
        </button>
        {studio && (
          <span className="muted mono" style={{ fontSize: 11 }}>
            {studio.tools.length} 个工具可用
          </span>
        )}
      </div>
    </div>
  );
}

/** 规范文档 / 系统提示词的浮层。规范是 Markdown，直接按 Markdown 渲染。 */
function SheetModal({
  sheet,
  onClose,
}: {
  sheet: { title: string; doc?: string; text?: string };
  onClose: () => void;
}) {
  const toast = useApp((s) => s.toast);
  const [text, setText] = useState<string | null>(sheet.text ?? null);

  useEffect(() => {
    if (text !== null || !sheet.doc) return;
    let alive = true;
    void (async () => {
      try {
        const t = await api.studioSpec(sheet.doc!);
        if (alive) setText(t);
      } catch (e) {
        if (alive) toast("error", errText(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [sheet.doc, text, toast]);

  return (
    <Modal title={sheet.title} icon="book" wide onClose={onClose}>
      {text === null ? (
        <div className="row" style={{ gap: 8 }}>
          <Spinner />
          <span className="muted">正在读取…</span>
        </div>
      ) : sheet.doc ? (
        <div style={{ maxHeight: "62vh", overflow: "auto" }}>
          <Markdown source={text} />
        </div>
      ) : (
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
            maxHeight: "62vh",
            overflow: "auto",
          }}
        >
          {text}
        </pre>
      )}
    </Modal>
  );
}
