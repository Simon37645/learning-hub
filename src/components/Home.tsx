// 首页欢迎区：问候语 + 今日概览 + 快捷入口。
//
// 用户在需求里点名了这个界面：「Hi <名字>，想要学习关于 <项目名称> 的什么呢？」

import { useMemo } from "react";
import { useApp } from "../store/app";
import { AppMark, Icon, type IconName } from "./ui";

export function timeGreeting(): string {
  const h = new Date().getHours();
  if (h < 5) return "夜深了";
  if (h < 9) return "早上好";
  if (h < 12) return "上午好";
  if (h < 14) return "中午好";
  if (h < 18) return "下午好";
  if (h < 23) return "晚上好";
  return "夜深了";
}

export function Welcome({ topicName }: { topicName: string | null }) {
  const config = useApp((s) => s.config);
  const brief = useApp((s) => s.brief);
  const topics = useApp((s) => s.topics);
  const send = useApp((s) => s.send);
  const setView = useApp((s) => s.setView);
  const setPaletteOpen = useApp((s) => s.setPaletteOpen);
  const dueTopics = useMemo(() => topics.filter((t) => t.stats.cardsDue > 0), [topics]);

  const name = config?.userName ?? "旅行者";

  // 有主题时问候聚焦主题；没有主题时让用户直接说要学什么
  const question = topicName
    ? `想要学习关于「${topicName}」的什么呢？`
    : "想要学习关于什么的呢？";

  const quick: { label: string; icon: IconName; run: () => void }[] = topicName
    ? [
        { label: "先给我全局地图", icon: "hub", run: () => void send(`先别讲细节：请给「${topicName}」画一张全局地图——包含哪几块、彼此什么关系、按什么顺序学。`) },
        { label: "我该从哪开始", icon: "play", run: () => void send(`我想系统学「${topicName}」，请先问我 3 个问题判断基础，再给出第一步该做什么。`) },
        { label: "考考我的水平", icon: "target", run: () => void send(`请出 5 道由易到难的题测一下我在「${topicName}」上的现有水平，先不要给答案。`) },
        { label: "开始一次学习会话", icon: "clock", run: () => void useApp.getState().startSession(`${topicName} 学习`) },
      ]
    : [
        { label: "我想学点新东西", icon: "sparkle", run: () => void send("我想系统学一门新东西，帮我先理清楚：需要明确哪些信息，才能开始？") },
        { label: "今天该学什么", icon: "calendar", run: () => void setView("agenda") },
        { label: "找找以前的主题", icon: "search", run: () => setPaletteOpen(true) },
        { label: "新建一个主题", icon: "plus", run: () => void send("帮我建一个学习主题：我想学") },
        // 工坊：不学东西，造东西（技能 / MCP 服务器）
        { label: "去工坊造个技能", icon: "hammer", run: () => void useApp.getState().openStudio() },
      ];

  return (
    <div className="home" style={{ padding: "3vh 0 20px" }}>
      <div className="home-inner">
        <div className="home-mark">
          <AppMark size={104} color="var(--text-faint)" />
        </div>

        <h1 className="home-title">
          Hi {name}，{question}
        </h1>
        <div className="home-sub">{timeGreeting()}，{new Date().toLocaleDateString("zh-CN", { month: "long", day: "numeric", weekday: "long" })}</div>

        {(brief || dueTopics.length > 0) && (
          <>
            <div className="brief-grid">
              <div className="brief-card" onClick={() => setView("agenda")}>
                <div className={"num" + ((brief?.dueCards ?? 0) > 0 ? " hot" : "")}>{brief?.dueCards ?? 0}</div>
                <div className="lbl">卡片待复习</div>
              </div>
              <div className="brief-card" onClick={() => setView("agenda")}>
                <div className={"num" + ((brief?.overdueTasks ?? 0) > 0 ? " hot" : "")}>{brief?.overdueTasks ?? 0}</div>
                <div className="lbl">逾期任务</div>
              </div>
              <div className="brief-card" onClick={() => setView("agenda")}>
                <div className="num">{brief?.openTasks ?? 0}</div>
                <div className="lbl">未完成任务</div>
              </div>
              <div className="brief-card" onClick={() => setView("agenda")}>
                <div className="num">{brief?.topicsTouchedToday ?? 0}</div>
                <div className="lbl">今天动过的主题</div>
              </div>
            </div>

            <div className="panel">
              <div className="panel-head">
                <Icon name="clock" size={13} />
                <span className="title">近 30 天学习活跃度</span>
                <span className="grow" />
                <span className="mono muted" style={{ fontSize: 11 }}>
                  {brief?.heatmap.reduce((n, c) => n + c.count, 0) ?? 0} 次会话
                </span>
              </div>
              <div className="panel-body">
                <div className="heatmap">
                  {(brief?.heatmap ?? []).map((c) => (
                    <div
                      key={c.date}
                      className={"heat-cell" + (c.level > 0 ? ` l${c.level}` : "")}
                      title={`${c.date}：${c.count} 次会话`}
                      style={{ height: `${Math.max(8, Math.min(34, 8 + c.count * 7))}px` }}
                    />
                  ))}
                </div>
              </div>
            </div>
          </>
        )}

        <div className="quick-row">
          {quick.map((q) => (
            <button key={q.label} className="chip" onClick={q.run}>
              <Icon name={q.icon} size={13} />
              {q.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

/** 聊天区里的空状态：有主题时给更短的引导 */
export function TopicWelcome({ topicName }: { topicName: string }) {
  const send = useApp((s) => s.send);
  const quick = [
    { label: "先给我一张全局地图", text: `先别讲细节：请给「${topicName}」画一张全局地图——包含哪几块、彼此什么关系、按什么顺序学。` },
    { label: "我该从哪开始", text: `我想系统学「${topicName}」，请先问我 3 个问题判断基础，再给出第一步该做什么。` },
    { label: "考考我现在什么水平", text: `请出 5 道由易到难的题测一下我在「${topicName}」上的现有水平，先不要给答案。` },
    { label: "复习到期卡片", text: `看一下我在「${topicName}」下到期的卡片，开始抽查我。` },
  ];

  return (
    <div className="empty" style={{ padding: "26px 20px", gap: 16 }}>
      <div className="home-mark">
        <AppMark size={76} color="var(--text-faint)" />
      </div>
      <div style={{ fontSize: 14.5, color: "var(--text-sub)" }}>
        关于「{topicName}」，想从哪里开始？
      </div>
      <div className="col" style={{ gap: 8, width: "100%", maxWidth: 460 }}>
        {quick.map((q) => (
          <button
            key={q.label}
            className="btn"
            style={{ justifyContent: "flex-start" }}
            onClick={() => void send(q.text)}
          >
            <Icon name="sparkle" size={13} />
            {q.label}
          </button>
        ))}
      </div>
    </div>
  );
}
