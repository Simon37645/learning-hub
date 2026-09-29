// 讲解方案卡片：显示当前的讲解进度，并驱动「一步一停」的讲解节奏。
//
// agent 用 lesson_plan 出方案、lesson_step 标记进度；用户在这里看到全景，
// 也能手动推进（比如「这步我懂了」直接跳到下一步），或者让 agent 接着讲。

import { useCallback, useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { STEP_STATUS_LABEL, type LessonPlan, type StepStatus } from "../lib/types";
import { useApp } from "../store/app";
import { Icon, Spinner } from "./ui";

/**
 * 对话里的「讲解进度」卡片：像待办清单一样列着方案里的步骤，可展开/收起。
 *
 * 为什么要有它：方案卡片原来只在工作台「概览」里，聊天时看不到自己走到哪了——
 * 讲解模式本来就是一步一停，进度必须跟对话在同一屏上（用户要求的样式）。
 * 折叠状态记在本地，展开时每一步的状态由 agent 的 lesson_step 驱动。
 */
export function LessonSteps() {
  const topic = useApp((s) => s.topic);
  const tick = useApp((s) => s.lessonTick);
  const [plan, setPlan] = useState<LessonPlan | null>(null);
  const [open, setOpen] = useState(() => localStorage.getItem("hub.lessonSteps.open") !== "0");

  useEffect(() => {
    if (!topic) {
      setPlan(null);
      return;
    }
    let alive = true;
    api
      .lessonList(topic.slug)
      .then((ps) => {
        if (alive) setPlan(ps.find((p) => !p.finished) ?? null);
      })
      .catch(() => alive && setPlan(null));
    return () => {
      alive = false;
    };
  }, [topic?.slug, tick]);

  if (!plan || plan.steps.length === 0) return null;

  const done = plan.steps.filter((s) => s.status === "done" || s.status === "skipped").length;
  const current = plan.steps.find((s) => s.status === "doing") ?? plan.steps.find((s) => s.status === "todo");

  const toggle = () => {
    setOpen((v) => {
      try {
        localStorage.setItem("hub.lessonSteps.open", v ? "0" : "1");
      } catch {
        /* 隐私模式下写不了，不影响使用 */
      }
      return !v;
    });
  };

  return (
    <div className={"lesson-steps" + (open ? " open" : "")}>
      <button className="ls-head" onClick={toggle} title={open ? "收起步骤" : "展开步骤"}>
        <Icon name="list" size={13} />
        <span className="ls-count mono">
          {done}/{plan.steps.length}
        </span>
        <span className="ls-now">{current ? `第 ${current.index} 步 · ${current.title}` : "方案已走完"}</span>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={13} />
      </button>

      {open && (
        <div className="ls-body">
          {plan.title && <div className="ls-title">{plan.title}</div>}
          {plan.steps.map((s) => {
            const isCurrent = current?.index === s.index;
            const isDone = s.status === "done" || s.status === "skipped";
            return (
              <div key={s.index} className={"ls-step" + (isCurrent ? " on" : "") + (isDone ? " done" : "")}>
                <span className="ls-mark">
                  {isDone ? <Icon name="check" size={11} /> : isCurrent ? <span className="ls-dot" /> : <span className="ls-ring" />}
                </span>
                <div className="ls-main">
                  <div className="ls-step-title">
                    {s.index}. {s.title}
                  </div>
                  {(isCurrent || !isDone) && s.check && <div className="ls-check">检验：{s.check}</div>}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function LessonCard() {
  const topic = useApp((s) => s.topic)!;
  const send = useApp((s) => s.send);
  const toast = useApp((s) => s.toast);
  const [plans, setPlans] = useState<LessonPlan[] | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setPlans(await api.lessonList(topic.slug));
    } catch {
      setPlans([]);
    }
  }, [topic.slug]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 优先显示进行中的那一份
  const active = plans?.find((p) => !p.finished) ?? null;
  const finished = (plans ?? []).filter((p) => p.finished);

  async function setStep(plan: LessonPlan, index: number, status: StepStatus) {
    setBusy(true);
    try {
      const updated = await api.lessonSetStep(topic.slug, plan.id, index, status);
      setPlans((ps) => (ps ?? []).map((p) => (p.id === updated.id ? updated : p)));
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  if (plans === null) {
    return (
      <div className="panel">
        <div className="panel-head">
          <Icon name="play" size={13} />
          <span className="title">讲解方案</span>
          <span className="grow" />
          <Spinner />
        </div>
      </div>
    );
  }

  if (!active) {
    return (
      <div className="panel">
        <div className="panel-head">
          <Icon name="play" size={13} />
          <span className="title">讲解方案</span>
          <span className="grow" />
          {finished.length > 0 && (
            <span className="muted mono" style={{ fontSize: 11 }}>
              已讲完 {finished.length} 份
            </span>
          )}
        </div>
        <div className="panel-body col">
          <div className="sub" style={{ fontSize: 12.5 }}>
            还没有进行中的讲解方案。预习阶段让 agent 先出一份方案，它会定好「讲几步、每步怎么检验听懂了」，
            再一步步带你走。
          </div>
          <div className="row">
            <button
              className="btn primary sm"
              onClick={() =>
                void send(
                  "请先给我出一份讲解方案（用 lesson_plan）：把这次要学的内容拆成 4~8 个模块，说明每个模块的难点、以及你打算怎么检验我听懂了；如果有需要我先补的前置知识，也请一并指出。",
                )
              }
            >
              <Icon name="sparkle" size={13} /> 让 agent 出讲解方案
            </button>
          </div>
        </div>
      </div>
    );
  }

  const done = active.steps.filter((s) => s.status === "done" || s.status === "skipped").length;
  const current = active.steps.find((s) => s.status === "doing") ?? active.steps.find((s) => s.status === "todo");

  return (
    <div className="panel">
      <div className="panel-head">
        <Icon name="play" size={13} />
        <span className="title">{active.title}</span>
        <span className="grow" />
        <span className="mono muted" style={{ fontSize: 11 }}>
          {done}/{active.steps.length}
        </span>
      </div>
      <div className="panel-body col" style={{ gap: 10 }}>
        {active.goal && <div style={{ fontSize: 13 }}>目标：{active.goal}</div>}

        {active.prereqs.length > 0 && (
          <div className="col" style={{ gap: 3 }}>
            <span className="muted" style={{ fontSize: 11.5 }}>前置知识</span>
            {active.prereqs.map((p, i) => (
              <div key={i} className="row" style={{ gap: 6, fontSize: 12.5 }}>
                <Icon name="layers" size={12} style={{ color: "var(--text-faint)" }} />
                {p}
              </div>
            ))}
          </div>
        )}

        <div className="col" style={{ gap: 3 }}>
          {active.steps.map((s) => {
            const isCurrent = current?.index === s.index;
            return (
              <div
                key={s.index}
                className="row"
                style={{
                  gap: 8,
                  alignItems: "flex-start",
                  padding: "5px 8px",
                  borderRadius: 8,
                  background: isCurrent ? "var(--accent-soft)" : undefined,
                }}
              >
                <button
                  className={"check" + (s.status === "done" ? " on" : "")}
                  title={s.status === "done" ? "标记为未讲" : "标记已讲完"}
                  disabled={busy}
                  onClick={() => void setStep(active, s.index, s.status === "done" ? "todo" : "done")}
                  style={{ marginTop: 2 }}
                >
                  {s.status === "done" && <Icon name="check" size={10} />}
                </button>
                <div className="grow">
                  <div className="row" style={{ gap: 6 }}>
                    <span style={{ fontWeight: isCurrent ? 600 : 400, fontSize: 13 }}>
                      {s.index}. {s.title}
                    </span>
                    <span className={"tag" + (s.status === "doing" ? " accent" : "")}>
                      {STEP_STATUS_LABEL[s.status]}
                    </span>
                    {s.demoHtml && (
                      <button
                        className="tag"
                        title="打开这一步的 HTML 演示页"
                        onClick={() => void useApp.getState().openFile(s.demoHtml!)}
                      >
                        演示页
                      </button>
                    )}
                  </div>
                  {s.focus && (
                    <div className="muted" style={{ fontSize: 12, marginTop: 2 }}>
                      {s.focus}
                    </div>
                  )}
                  {s.check && (
                    <div className="muted" style={{ fontSize: 11.5 }}>
                      检验：{s.check}
                    </div>
                  )}
                  {s.sources.length > 0 && (
                    <div className="row wrap" style={{ gap: 4, marginTop: 3 }}>
                      {s.sources.map((src, i) => (
                        <span key={i} className="cite-chip" title={src}>
                          {src}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>

        <div className="row">
          <button
            className="btn sm primary"
            onClick={() =>
              void send(
                current
                  ? `继续讲第 ${current.index} 步「${current.title}」。讲完请按方案里的检验问题问我，等我回答再往下。`
                  : "方案里的步骤都走完了，请带我做个收尾总结。",
              )
            }
          >
            <Icon name="play" size={12} /> {current ? `继续讲第 ${current.index} 步` : "收尾总结"}
          </button>
          <button
            className="btn sm"
            disabled={busy}
            onClick={() => void setStep(active, current?.index ?? 1, "done")}
          >
            这一步我会了
          </button>
          <div className="grow" />
          <span className="muted" style={{ fontSize: 11.5 }}>
            agent 每讲完一步会自动标记进度
          </span>
        </div>
      </div>
    </div>
  );
}
