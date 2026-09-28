// 测验面板：「出题 → 作答 → 判分 → 复盘错题」的完整闭环。
//
// 判分分两段：客观题交卷即出分（本地比对）；主观题交给 agent 按采分点逐条对照，
// 结果会写进作答记录，所以刷新后还能看到评语与漏掉的要点。

import { useCallback, useEffect, useMemo, useState } from "react";
import { api, errText } from "../lib/api";
import {
  QUESTION_TYPE_LABEL,
  SUBJECTIVE_TYPES,
  type Attempt,
  type Question,
  type Quiz,
  type QuizAnswer,
  type QuizSummary,
  type WrongItem,
} from "../lib/types";
import { useApp } from "../store/app";
import { Empty, Icon, Modal, Spinner } from "./ui";

type View =
  | { kind: "list" }
  | { kind: "take"; quiz: Quiz }
  | { kind: "result"; quiz: Quiz; attempt: Attempt; wrong: WrongItem[] };

export function QuizPane() {
  const topic = useApp((s) => s.topic)!;
  const send = useApp((s) => s.send);
  const toast = useApp((s) => s.toast);

  const [quizzes, setQuizzes] = useState<QuizSummary[]>([]);
  const [view, setView] = useState<View>({ kind: "list" });
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [grading, setGrading] = useState(false);
  const [confirmDel, setConfirmDel] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setQuizzes(await api.quizList(topic.slug));
    } catch (e) {
      toast("error", errText(e));
    }
  }, [topic.slug, toast]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function openQuiz(id: string) {
    setBusy(true);
    try {
      const quiz = await api.quizGet(topic.slug, id);
      setAnswers({});
      setView({ kind: "take", quiz });
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function submit(quiz: Quiz) {
    const payload: QuizAnswer[] = quiz.questions.map((q) => ({
      questionId: q.id,
      value: answers[q.id] ?? "",
    }));
    const unanswered = payload.filter((a) => !a.value.trim()).length;
    if (unanswered > 0) {
      const ok = window.confirm(`还有 ${unanswered} 题没作答，确定交卷？`);
      if (!ok) return;
    }
    setBusy(true);
    try {
      const attempt = await api.quizSubmit(topic.slug, quiz.id, payload);
      const wrong = await api.quizWrongItems(topic.slug, quiz.id, attempt.id);
      setView({ kind: "result", quiz, attempt, wrong });
      void refresh();
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function gradeSubjective() {
    if (view.kind !== "result") return;
    setGrading(true);
    try {
      const attempt = await api.quizGradeSubjective(topic.slug, view.quiz.id, view.attempt.id);
      const wrong = await api.quizWrongItems(topic.slug, view.quiz.id, attempt.id);
      setView({ kind: "result", quiz: view.quiz, attempt, wrong });
      toast("success", "主观题已按采分点评完");
    } catch (e) {
      toast("error", errText(e));
    } finally {
      setGrading(false);
    }
  }

  // B 型题共用选项：从同组的第一道带选项的题里取
  const groupOptions = useMemo(() => {
    if (view.kind === "list") return new Map<string, Question["options"]>();
    const map = new Map<string, Question["options"]>();
    for (const q of view.quiz.questions) {
      if (q.group && q.options.length > 0 && !map.has(q.group)) map.set(q.group, q.options);
    }
    return map;
  }, [view]);

  if (view.kind === "take") {
    const quiz = view.quiz;
    return (
      <div className="wb-pane">
        <div className="row">
          <button className="icon-btn" title="返回试卷列表" onClick={() => setView({ kind: "list" })}>
            <Icon name="arrow-left" />
          </button>
          <h2 style={{ margin: 0, fontSize: 15 }}>{quiz.title}</h2>
          <span className="tag">{quiz.questions.length} 题 / {quiz.questions.reduce((n, q) => n + q.score, 0)} 分</span>
          <div className="grow" />
          <span className="muted" style={{ fontSize: 11.5 }}>
            客观题交卷即出分，主观题由 agent 按采分点评
          </span>
          <button className="btn primary" disabled={busy} onClick={() => void submit(quiz)}>
            {busy ? <Spinner /> : <Icon name="check" size={13} />} 交卷
          </button>
        </div>

        {quiz.scope && <div className="muted" style={{ fontSize: 12 }}>覆盖范围：{quiz.scope}</div>}

        <div className="col" style={{ gap: 14 }}>
          {quiz.questions.map((q, i) => (
            <div className="card-box" key={q.id} style={{ gap: 8 }}>
              <div className="row" style={{ gap: 8 }}>
                <span className="tag accent">{QUESTION_TYPE_LABEL[q.type]}</span>
                <span className="muted mono" style={{ fontSize: 11 }}>
                  {i + 1} / {q.score} 分
                </span>
              </div>
              {q.caseText && (
                <div className="panel" style={{ background: "var(--bg-sub)" }}>
                  <div className="panel-body" style={{ fontSize: 12.5 }}>{q.caseText}</div>
                </div>
              )}
              <div style={{ fontWeight: 450, whiteSpace: "pre-wrap" }}>{q.stem}</div>

              <QuestionInput
                q={q}
                options={q.options.length > 0 ? q.options : q.group ? groupOptions.get(q.group) ?? [] : []}
                value={answers[q.id] ?? ""}
                onChange={(v) => setAnswers((a) => ({ ...a, [q.id]: v }))}
              />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (view.kind === "result") {
    const { quiz, attempt, wrong } = view;
    const byId = new Map(quiz.questions.map((q) => [q.id, q]));
    return (
      <div className="wb-pane">
        <div className="row">
          <button className="icon-btn" title="返回试卷列表" onClick={() => setView({ kind: "list" })}>
            <Icon name="arrow-left" />
          </button>
          <h2 style={{ margin: 0, fontSize: 15 }}>{quiz.title} · 结果</h2>
          <div className="grow" />
          {attempt.pendingSubjective.length > 0 && (
            <button className="btn primary" disabled={grading} onClick={() => void gradeSubjective()}>
              {grading ? <Spinner /> : <Icon name="sparkle" size={13} />}
              让 agent 评分（{attempt.pendingSubjective.length} 题主观题）
            </button>
          )}
        </div>

        <div className="brief-grid">
          <div className="brief-card">
            <div className={"num" + (attempt.score / Math.max(1, attempt.total) < 0.6 ? " hot" : "")}>
              {attempt.score}
            </div>
            <div className="lbl">得分 / {attempt.total}</div>
          </div>
          <div className="brief-card">
            <div className="num">{Math.round((attempt.score / Math.max(1, attempt.total)) * 100)}%</div>
            <div className="lbl">正确率</div>
          </div>
          <div className="brief-card">
            <div className="num hot">{wrong.length}</div>
            <div className="lbl">失分题</div>
          </div>
          <div className="brief-card">
            <div className="num">{attempt.pendingSubjective.length}</div>
            <div className="lbl">待评主观题</div>
          </div>
        </div>

        <div className="col" style={{ gap: 10 }}>
          {attempt.results.map((r, i) => {
            const q = byId.get(r.questionId);
            if (!q) return null;
            const ok = r.correct;
            return (
              <div className="card-box" key={r.questionId} style={{ gap: 6 }}>
                <div className="row" style={{ gap: 8 }}>
                  <Icon
                    name={ok === true ? "check" : ok === false ? "close" : "clock"}
                    size={14}
                    style={{ color: ok === true ? "var(--ok)" : ok === false ? "var(--danger)" : "var(--text-faint)" }}
                  />
                  <span className="tag">{QUESTION_TYPE_LABEL[q.type]}</span>
                  <span style={{ fontWeight: 450 }}>
                    {i + 1}. {q.stem}
                  </span>
                  <div className="grow" />
                  <span className="mono" style={{ fontSize: 12 }}>
                    {r.score} / {r.maxScore}
                  </span>
                </div>
                <div className="sub" style={{ fontSize: 12.5 }}>{r.comment}</div>
                {r.missing.length > 0 && (
                  <div className="col" style={{ gap: 3 }}>
                    <span className="muted" style={{ fontSize: 11.5 }}>漏掉的采分点</span>
                    {r.missing.map((m, k) => (
                      <span key={k} style={{ fontSize: 12.5, color: "var(--warn)" }}>· {m}</span>
                    ))}
                  </div>
                )}
                {q.explanation && (
                  <details>
                    <summary className="muted" style={{ fontSize: 11.5, cursor: "pointer" }}>解析</summary>
                    <div className="sub" style={{ fontSize: 12.5, marginTop: 4 }}>{q.explanation}</div>
                  </details>
                )}
              </div>
            );
          })}
        </div>

        {wrong.length > 0 && (
          <div className="row">
            <button
              className="btn"
              onClick={() =>
                void send(
                  `我刚做完「${quiz.title}」，错了这些题：\n\n${wrong
                    .map((w, i) => `${i + 1}. ${w.stem}\n   我的答案：${w.yourAnswer || "（空）"}\n   正确答案：${w.correctAnswer}`)
                    .join("\n")}\n\n请按顺序给我讲一遍：每道题我错在哪、正确思路是什么，最后帮我判断该补哪个知识点。`,
                )
              }
            >
              <Icon name="sparkle" size={13} /> 让 agent 讲讲这些错题
            </button>
          </div>
        )}
      </div>
    );
  }

  // ---- 列表 ----
  return (
    <div className="wb-pane">
      <div className="row">
        <span className="sub" style={{ fontSize: 12.5 }}>
          让 agent 出题，或者自己手写。交卷后客观题立刻出分，主观题按采分点评。
        </span>
        <div className="grow" />
        <button
          className="btn primary"
          onClick={() =>
            void send("请根据我最近学的内容出一份小测（用 quiz_create）。题型请搭配着来：A1/A2 考基础与推理，X 型考容易漏的点，再来 1~2 道名词解释或简答题。出完告诉我一共几题、覆盖了什么。")
          }
        >
          <Icon name="plus" size={13} /> 让 agent 出题
        </button>
        <button className="btn" onClick={() => void refresh()}>
          <Icon name="refresh" size={13} />
        </button>
      </div>

      {quizzes.length === 0 ? (
        <Empty icon="target">
          还没有测验。
          <br />
          点右上角「让 agent 出题」，它会把题目存成试卷，你在这里作答。
        </Empty>
      ) : (
        <div className="col" style={{ gap: 6 }}>
          {quizzes.map((q) => (
            <div key={q.id} className="list-row" onClick={() => void openQuiz(q.id)}>
              <Icon name="target" size={14} />
              <div className="li-main">
                <div className="li-title">{q.title}</div>
                <div className="li-sub">
                  {q.typeSummary}｜{q.questionCount} 题 / {q.totalScore} 分
                  {q.scope ? `｜${q.scope}` : ""}
                  {q.attemptCount > 0 ? `｜做过 ${q.attemptCount} 次` : ""}
                </div>
              </div>
              {q.lastPercent != null && (
                <span className={"tag" + (q.lastPercent < 60 ? " accent" : "")}>上次 {q.lastPercent}%</span>
              )}
              <button
                className="icon-btn"
                title="删除"
                onClick={(e) => {
                  e.stopPropagation();
                  setConfirmDel(q.id);
                }}
              >
                <Icon name="trash" size={13} />
              </button>
            </div>
          ))}
        </div>
      )}

      {confirmDel && (
        <Modal
          title="删除这份试卷？"
          icon="alert"
          onClose={() => setConfirmDel(null)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirmDel(null)}>
                取消
              </button>
              <button
                className="btn danger"
                onClick={async () => {
                  try {
                    await api.quizDelete(topic.slug, confirmDel);
                    setConfirmDel(null);
                    void refresh();
                  } catch (e) {
                    toast("error", errText(e));
                  }
                }}
              >
                删除
              </button>
            </>
          }
        >
          <div className="sub">试卷与作答记录都会删除（试卷进回收站，可以恢复）。</div>
        </Modal>
      )}
    </div>
  );
}

/** 按题型渲染作答控件：单选 / 多选 / 填空 */
function QuestionInput({
  q,
  options,
  value,
  onChange,
}: {
  q: Question;
  options: Question["options"];
  value: string;
  onChange: (v: string) => void;
}) {
  const subjective = SUBJECTIVE_TYPES.includes(q.type);

  if (subjective) {
    return (
      <textarea
        className="textarea"
        rows={q.type === "term" ? 2 : 4}
        placeholder={q.type === "term" ? "写出这个词的定义与关键限定" : "分点作答，写清要点"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }

  if (options.length === 0) {
    return (
      <input
        className="input"
        placeholder="填写选项号，多个用逗号分隔（如 A,C）"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }

  const multi = q.type === "x";
  const selected = value
    .split(/[^A-Za-z]/)
    .map((s) => s.toUpperCase())
    .filter(Boolean);

  const toggle = (key: string) => {
    if (multi) {
      const next = selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key];
      onChange(next.sort().join(""));
    } else {
      onChange(key);
    }
  };

  return (
    <div className="col" style={{ gap: 4 }}>
      {options.map((o) => {
        const on = selected.includes(o.key.toUpperCase());
        return (
          <label
            key={o.key}
            className="row"
            style={{
              gap: 8,
              alignItems: "flex-start",
              padding: "5px 8px",
              borderRadius: 8,
              cursor: "pointer",
              background: on ? "var(--accent-soft)" : undefined,
            }}
          >
            <input
              type={multi ? "checkbox" : "radio"}
              checked={on}
              onChange={() => toggle(o.key)}
              style={{ marginTop: 3 }}
            />
            <span style={{ fontSize: 13 }}>
              <b className="mono">{o.key}</b>. {o.text}
            </span>
          </label>
        );
      })}
      {multi && (
        <div className="muted" style={{ fontSize: 11.5 }}>
          X 型题可多选，多选、漏选都不得分
        </div>
      )}
    </div>
  );
}
