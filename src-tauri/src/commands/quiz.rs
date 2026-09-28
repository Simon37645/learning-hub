//! 测验命令与判分。
//!
//! 判分分两条路：
//! - **客观题**本地比对（`Attempt::grade_objective`），零成本、结果确定
//! - **主观题**交给模型按「采分点清单」逐条对照，要求返回结构化 JSON，
//!   再把分数、评语、漏掉的要点写回作答记录
//!
//! 试卷与作答都存成文件：`quizzes/<id>.json` 与 `quizzes/<id>.attempts.jsonl`。

use crate::agent::complete_once;
use crate::domain::quiz::{Answer, Attempt, Question, QuestionResult, Quiz};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizSummary {
    pub id: String,
    pub title: String,
    pub kind: crate::domain::quiz::QuizKind,
    pub scope: String,
    pub question_count: usize,
    pub total_score: u32,
    pub type_summary: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 最近一次作答的得分率（百分比），没做过就是 None
    pub last_percent: Option<f32>,
    pub attempt_count: usize,
}

fn quizzes_dir(state: &AppState, slug: &str) -> AppResult<std::path::PathBuf> {
    let topic = state.0.workspace().resolve(slug)?;
    let dir = topic.dir.join(crate::domain::topic::DIR_QUIZZES);
    crate::paths::ensure_dir(&dir)?;
    Ok(dir)
}

fn quiz_path(state: &AppState, slug: &str, id: &str) -> AppResult<std::path::PathBuf> {
    Ok(quizzes_dir(state, slug)?.join(format!("{id}.json")))
}

fn attempts_path(state: &AppState, slug: &str, id: &str) -> AppResult<std::path::PathBuf> {
    Ok(quizzes_dir(state, slug)?.join(format!("{id}.attempts.jsonl")))
}

fn load_attempts(state: &AppState, slug: &str, quiz_id: &str) -> Vec<Attempt> {
    attempts_path(state, slug, quiz_id)
        .ok()
        .and_then(|p| store::read_jsonl::<Attempt>(&p).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn quiz_list(state: State<'_, AppState>, slug: String) -> AppResult<Vec<QuizSummary>> {
    let dir = quizzes_dir(&state, &slug)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| AppError::other(e.to_string()))? {
        let path = match entry {
            Ok(e) => e.path(),
            Err(_) => continue,
        };
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let quiz: Quiz = match store::read_json_opt(&path) {
            Ok(Some(q)) => q,
            _ => continue,
        };
        let attempts = load_attempts(&state, &slug, &quiz.id);
        let last = attempts.last().map(|a| a.percent());
        out.push(QuizSummary {
            id: quiz.id.clone(),
            title: quiz.title.clone(),
            kind: quiz.kind,
            scope: quiz.scope.clone(),
            question_count: quiz.questions.len(),
            total_score: quiz.total_score(),
            type_summary: quiz.type_summary(),
            created_at: quiz.created_at,
            last_percent: last,
            attempt_count: attempts.len(),
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

#[tauri::command]
pub async fn quiz_get(state: State<'_, AppState>, slug: String, id: String) -> AppResult<Quiz> {
    let path = quiz_path(&state, &slug, &id)?;
    store::read_json_opt::<Quiz>(&path)?
        .ok_or_else(|| AppError::NotFound(format!("找不到试卷 {id}")))
}

#[tauri::command]
pub async fn quiz_delete(state: State<'_, AppState>, slug: String, id: String) -> AppResult<()> {
    let path = quiz_path(&state, &slug, &id)?;
    if path.exists() {
        let trash = state
            .0
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("trash");
        store::move_to_trash(&trash, &path)?;
    }
    let ap = attempts_path(&state, &slug, &id)?;
    if ap.exists() {
        let _ = std::fs::remove_file(ap);
    }
    Ok(())
}

/// 保存（或覆盖）一份试卷。agent 出题与用户手写都走这里。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizInput {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub kind: Option<crate::domain::quiz::QuizKind>,
    #[serde(default)]
    pub scope: String,
    pub questions: Vec<Question>,
}

#[tauri::command]
pub async fn quiz_save(
    state: State<'_, AppState>,
    slug: String,
    input: QuizInput,
) -> AppResult<Quiz> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    if input.questions.is_empty() {
        return Err(AppError::invalid("试卷里至少要有一道题"));
    }
    let mut quiz = match input
        .id
        .as_deref()
        .and_then(|id| quiz_path(&state, &slug, id).ok())
        .and_then(|p| store::read_json_opt::<Quiz>(&p).ok().flatten())
    {
        Some(existing) => existing,
        None => Quiz::new(topic.meta.id.clone(), input.title.clone()),
    };
    quiz.title = input.title;
    if let Some(k) = input.kind {
        quiz.kind = k;
    }
    quiz.scope = input.scope;
    quiz.questions = input.questions;
    quiz.created_by = "user".into();

    let path = quiz_path(&state, &slug, &quiz.id)?;
    store::write_json(&path, &quiz)?;
    core.emit_topics_updated(&topic);
    Ok(quiz)
}

/// 交卷：客观题立刻判分，主观题挂起等模型。
#[tauri::command]
pub async fn quiz_submit(
    state: State<'_, AppState>,
    slug: String,
    id: String,
    answers: Vec<Answer>,
) -> AppResult<Attempt> {
    let quiz = quiz_get(state.clone(), slug.clone(), id.clone()).await?;
    let mut attempt = Attempt::new(&quiz.id);
    attempt.answers = answers;
    attempt.grade_objective(&quiz);
    let path = attempts_path(&state, &slug, &id)?;
    store::append_jsonl(&path, &attempt)?;
    Ok(attempt)
}

#[tauri::command]
pub async fn quiz_attempts(
    state: State<'_, AppState>,
    slug: String,
    id: String,
) -> AppResult<Vec<Attempt>> {
    Ok(load_attempts(&state, &slug, &id))
}

/// 判分用的系统提示词。要求模型严格按采分点给分，并只返回 JSON。
const GRADER_SYSTEM: &str = "你是一位严格的阅卷老师。你会拿到若干道主观题、每题的采分点，以及学生的作答。\n\
请逐题评分：\n\
- 分数只按「命中了几个采分点」给，不要因为字写得多就给分，也不要因为表述不同就扣分\n\
- 每题满分见 maxScore；可以给小数（例如 0.5）\n\
- comment 用一两句中文说明扣分原因，直接对学生说\n\
- missing 列出学生没说到的采分点（原文照抄采分点，不要改写）\n\
- 只输出 JSON 数组，不要任何解释文字、不要 markdown 围栏。格式：\n\
[{\"id\":\"题目id\",\"score\":1.5,\"comment\":\"…\",\"missing\":[\"…\"]}]";

#[derive(Debug, Deserialize)]
struct GradeItem {
    id: String,
    score: f32,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    missing: Vec<String>,
}

/// 让模型给主观题打分。
#[tauri::command]
pub async fn quiz_grade_subjective(
    state: State<'_, AppState>,
    slug: String,
    quiz_id: String,
    attempt_id: String,
) -> AppResult<Attempt> {
    let core = state.0.clone();
    let quiz = quiz_get(state.clone(), slug.clone(), quiz_id.clone()).await?;
    let path = attempts_path(&state, &slug, &quiz_id)?;
    let mut attempts = store::read_jsonl::<Attempt>(&path)?;
    let idx = attempts
        .iter()
        .position(|a| a.id == attempt_id)
        .ok_or_else(|| AppError::NotFound("找不到这次作答".into()))?;

    let subjective: Vec<&Question> = quiz
        .questions
        .iter()
        .filter(|q| q.kind.is_subjective())
        .filter(|q| {
            attempts[idx]
                .results
                .iter()
                .any(|r| r.question_id == q.id && !r.graded_by_agent)
        })
        .collect();
    if subjective.is_empty() {
        return Ok(attempts[idx].clone());
    }

    let mut user = String::from("请批改以下主观题。\n\n");
    for q in &subjective {
        let given = attempts[idx]
            .answers
            .iter()
            .find(|a| a.question_id == q.id)
            .map(|a| a.value.as_str())
            .unwrap_or("（未作答）");
        user.push_str(&format!(
            "### 题目 id: {}\n题型：{}\n题干：{}\n满分：{}\n采分点：\n{}\n\n学生作答：\n{}\n\n---\n\n",
            q.id,
            q.kind.label(),
            q.stem,
            q.score,
            if q.key_points.is_empty() {
                "（未给采分点，请按题干的合理要点自行判断）".to_string()
            } else {
                q.key_points
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("{}. {p}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
            if given.trim().is_empty() { "（未作答）" } else { given }
        ));
    }

    let reply = complete_once(&core, GRADER_SYSTEM, &user, 4096).await?;
    let items: Vec<GradeItem> = crate::agent::extract_json(&reply)?;
    for item in items {
        let max = quiz
            .questions
            .iter()
            .find(|q| q.id == item.id)
            .map(|q| q.score as f32)
            .unwrap_or(1.0);
        let score = item.score.clamp(0.0, max);
        attempts[idx].apply_subjective(&item.id, score, item.comment, item.missing);
    }
    store::write_jsonl(&path, &attempts)?;
    Ok(attempts[idx].clone())
}

/// 汇总某次作答的错题（复习时按这些点再讲一遍）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrongItem {
    pub question_id: String,
    pub kind: String,
    pub stem: String,
    pub your_answer: String,
    pub correct_answer: String,
    pub explanation: String,
    pub missing: Vec<String>,
}

#[tauri::command]
pub async fn quiz_wrong_items(
    state: State<'_, AppState>,
    slug: String,
    quiz_id: String,
    attempt_id: String,
) -> AppResult<Vec<WrongItem>> {
    let quiz = quiz_get(state.clone(), slug.clone(), quiz_id.clone()).await?;
    let attempts = load_attempts(&state, &slug, &quiz_id);
    let attempt = attempts
        .iter()
        .find(|a| a.id == attempt_id)
        .ok_or_else(|| AppError::NotFound("找不到这次作答".into()))?;

    let mut out = Vec::new();
    for r in &attempt.results {
        let lost = r.score < r.max_score - 0.01;
        if !lost {
            continue;
        }
        let Some(q) = quiz.questions.iter().find(|q| q.id == r.question_id) else {
            continue;
        };
        let your = attempt
            .answers
            .iter()
            .find(|a| a.question_id == q.id)
            .map(|a| a.value.clone())
            .unwrap_or_default();
        out.push(WrongItem {
            question_id: q.id.clone(),
            kind: q.kind.label().to_string(),
            stem: q.stem.clone(),
            your_answer: your,
            correct_answer: if q.answer.is_empty() {
                r.comment.clone()
            } else {
                q.answer.join("")
            },
            explanation: q.explanation.clone(),
            missing: r.missing.clone(),
        });
    }
    Ok(out)
}

/// 给工具层复用的「本地判分」入口（agent 也可以主动帮用户批改）。
pub fn grade_locally(quiz: &Quiz, answers: Vec<Answer>) -> (Attempt, Vec<QuestionResult>) {
    let mut attempt = Attempt::new(&quiz.id);
    attempt.answers = answers;
    attempt.grade_objective(quiz);
    let results = attempt.results.clone();
    (attempt, results)
}
