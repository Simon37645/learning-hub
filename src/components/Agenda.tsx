// 日程页：把「今天该做什么」摊开——到期卡片、各时间桶的任务、近 30 天活跃度。

import { useEffect, useMemo } from "react";
import { useApp } from "../store/app";
import { dueLabel, fmtDate } from "../lib/format";
import { Icon, Spinner } from "./ui";

export function Agenda() {
  const agenda = useApp((s) => s.agenda);
  const brief = useApp((s) => s.brief);
  const refreshAgenda = useApp((s) => s.refreshAgenda);
  const refreshBrief = useApp((s) => s.refreshBrief);
  const updateTask = useApp((s) => s.updateTask);
  const openTopic = useApp((s) => s.openTopic);
  const topics = useApp((s) => s.topics);

  useEffect(() => {
    void refreshAgenda();
    void refreshBrief();
  }, [refreshAgenda, refreshBrief]);

  const dueTopics = useMemo(() => topics.filter((t) => t.stats.cardsDue > 0), [topics]);

  const total = agenda.reduce((n, b) => n + b.tasks.length, 0);

  return (
    <div className="wb-body" style={{ flex: 1 }}>
      <div className="wb-pane" style={{ maxWidth: 860 }}>
        <div className="row">
          <h2 style={{ margin: 0, fontSize: 15 }}>
            {brief?.date ?? "今日"}要做什么
          </h2>
          <div className="grow" />
          <button
            className="btn sm"
            onClick={() => {
              void refreshAgenda();
              void refreshBrief();
            }}
          >
            <Icon name="refresh" size={13} /> 刷新
          </button>
        </div>

        <div className="brief-grid">
          <div className="brief-card">
            <div className={"num" + ((brief?.dueCards ?? 0) > 0 ? " hot" : "")}>{brief?.dueCards ?? 0}</div>
            <div className="lbl">卡片待复习</div>
          </div>
          <div className="brief-card">
            <div className={"num" + ((brief?.overdueTasks ?? 0) > 0 ? " hot" : "")}>{brief?.overdueTasks ?? 0}</div>
            <div className="lbl">逾期</div>
          </div>
          <div className="brief-card">
            <div className="num">{brief?.openTasks ?? 0}</div>
            <div className="lbl">未完成任务</div>
          </div>
          <div className="brief-card">
            <div className="num">{brief?.topicsTouchedToday ?? 0}</div>
            <div className="lbl">今天动过的主题</div>
          </div>
        </div>

        {dueTopics.length > 0 && (
          <div className="panel">
            <div className="panel-head">
              <Icon name="target" size={13} />
              <span className="title">待复习</span>
              <span className="grow" />
              <span className="muted mono" style={{ fontSize: 11 }}>
                共 {brief?.dueCards ?? 0} 张
              </span>
            </div>
            <div className="panel-body tight">
              {dueTopics.map((t) => (
                <div key={t.slug} className="list-row" onClick={() => void openTopic(t.slug)}>
                  <Icon name="target" size={14} style={{ color: "var(--accent)" }} />
                  <div className="li-main">
                    <div className="li-title">{t.meta.name}</div>
                    <div className="li-sub">
                      {t.stats.cardsDue} 张到期 · 共 {t.stats.cards} 张卡片
                      {t.stats.nextDue ? ` · 最近 ${dueLabel(t.stats.nextDue)}` : ""}
                    </div>
                  </div>
                  <span className="tag accent">去复习</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {total === 0 ? (
          <div className="empty" style={{ padding: "40px 20px" }}>
            <Icon name="calendar" size={22} style={{ opacity: 0.5 }} />
            <div>日程是空的。在主题的计划页里加一条「今天读完第 1 章」试试。</div>
          </div>
        ) : (
          <div className="agenda-grid">
            {agenda.map((b) => (
              <div key={b.key} className="bucket">
                <div className="bucket-head">
                  <span>{b.label}</span>
                  <span className="muted mono" style={{ fontSize: 11 }}>
                    {b.tasks.length}
                  </span>
                  <span className="line" />
                </div>
                <div className="col" style={{ gap: 5 }}>
                  {b.tasks.map(({ task, topicSlug, topicName }) => {
                    const done = task.status === "done";
                    return (
                      <div key={task.id} className={"task-row" + (done ? " done" : "")}>
                        <button
                          className={"check" + (done ? " on" : "")}
                          onClick={() =>
                            void updateTask(task.id, { status: done ? "todo" : "done" })
                          }
                          title={done ? "重新打开" : "标记完成"}
                        >
                          {done && <Icon name="check" size={10} />}
                        </button>
                        <div className="grow">
                          <div style={{ fontWeight: 450 }}>{task.title}</div>
                          {task.detail && (
                            <div className="muted" style={{ fontSize: 12 }}>
                              {task.detail}
                            </div>
                          )}
                        </div>
                        {task.priority === 3 && <span className="tag accent">高</span>}
                        {task.stage && <span className="tag">{task.stage === "preview" ? "预习" : task.stage === "learn" ? "学习" : task.stage === "review" ? "复习" : "测验"}</span>}
                        <button className="tag" onClick={() => void openTopic(topicSlug)} title="打开主题">
                          {topicName}
                        </button>
                        {task.due && (
                          <span className="muted mono" style={{ fontSize: 11 }}>
                            {b.key === "overdue" ? dueLabel(task.due) : fmtDate(task.due)}
                          </span>
                        )}
                        {task.estimateMin ? (
                          <span className="muted" style={{ fontSize: 11 }}>
                            {task.estimateMin} 分钟
                          </span>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        )}

        <div className="panel">
          <div className="panel-head">
            <Icon name="clock" size={13} />
            <span className="title">近 30 天</span>
          </div>
          <div className="panel-body">
            <div className="heatmap" style={{ height: 40 }}>
              {(brief?.heatmap ?? []).map((c) => (
                <div
                  key={c.date}
                  className={"heat-cell" + (c.level > 0 ? ` l${c.level}` : "")}
                  title={`${c.date}：${c.count} 次会话`}
                  style={{ height: `${Math.max(8, Math.min(40, 8 + c.count * 8))}px` }}
                />
              ))}
              {!brief && <Spinner />}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
