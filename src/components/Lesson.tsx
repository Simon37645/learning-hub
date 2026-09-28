// 讲解方案卡片：显示当前的讲解进度，并驱动「一步一停」的讲解节奏。
//
// agent 用 lesson_plan 出方案、lesson_step 标记进度；用户在这里看到全景，
// 也能手动推进（比如「这步我懂了」直接跳到下一步），或者让 agent 接着讲。

import { useCallback, useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { STEP_STATUS_LABEL, type LessonPlan, type StepStatus } from "../lib/types";
import { useApp } from "../store/app";
import { Icon, Spinner } from "./ui";

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
