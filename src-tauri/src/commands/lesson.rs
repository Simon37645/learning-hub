//! 讲解模式：先出一份「讲解方案」，再按方案一步步讲。
//!
//! 为什么要把方案落成结构化文件而不是让它留在对话里：
//! 一次讲解会跨很多轮对话，模型需要始终知道「讲到第几步了、下一步是什么」；
//! 用户也需要看到整体进度，而不是被一条条消息推着走。
//!
//! 方案存在 `<主题>/lessons/<id>.json`，每一步都带「讲什么 / 怎么检验懂了」，
//! 讲完一步就把状态改成 done —— 这个进度会注入系统提示词。

use crate::error::{AppError, AppResult};
use crate::paths::ensure_dir;
use crate::state::AppState;
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Todo,
    Doing,
    Done,
    Skipped,
}

impl StepStatus {
    pub fn label(self) -> &'static str {
        match self {
            StepStatus::Todo => "待讲",
            StepStatus::Doing => "讲解中",
            StepStatus::Done => "已讲完",
            StepStatus::Skipped => "跳过",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LessonStep {
    /// 第几步（从 1 开始，前端直接显示）
    pub index: u32,
    pub title: String,
    /// 这一步要讲清什么（给 agent 自己的提示）
    pub focus: String,
    /// 怎么判断用户懂了（要向用户提的问题 / 要用户做的动作）
    #[serde(default)]
    pub check: String,
    /// 用到哪些资料（相对主题目录的路径，可带页码）
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub status: StepStatus,
    /// 讲解这一步时可以生成的 HTML 演示页（相对路径）
    #[serde(default)]
    pub demo_html: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LessonPlan {
    pub id: String,
    pub topic_id: String,
    pub title: String,
    /// 这次讲解要达成的目标（一句话）
    pub goal: String,
    /// 需要的前置知识，含「在哪个主题里学过」
    #[serde(default)]
    pub prereqs: Vec<String>,
    #[serde(default)]
    pub steps: Vec<LessonStep>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub finished: bool,
}

impl LessonPlan {
    pub fn current_step(&self) -> Option<&LessonStep> {
        self.steps
            .iter()
            .find(|s| s.status == StepStatus::Doing)
            .or_else(|| self.steps.iter().find(|s| s.status == StepStatus::Todo))
    }

    pub fn progress(&self) -> (usize, usize) {
        let done = self
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Done | StepStatus::Skipped))
            .count();
        (done, self.steps.len())
    }

    /// 注入系统提示词用的紧凑视图。
    pub fn digest(&self) -> String {
        let (done, total) = self.progress();
        let mut out = format!(
            "《{}》（目标：{}）进度 {done}/{total}\n",
            self.title, self.goal
        );
        if !self.prereqs.is_empty() {
            out.push_str(&format!("前置：{}\n", self.prereqs.join("；")));
        }
        for s in &self.steps {
            let mark = match s.status {
                StepStatus::Done => "[已讲完]",
                StepStatus::Doing => "[正在讲]",
                StepStatus::Skipped => "[跳过]",
                StepStatus::Todo => "[待讲]",
            };
            out.push_str(&format!(
                "{mark} {}. {}{}{}\n",
                s.index,
                s.title,
                if s.focus.is_empty() { String::new() } else { format!("：{}", s.focus) },
                if s.demo_html.is_some() { "（有演示页）" } else { "" }
            ));
        }
        out
    }
}

fn lessons_dir(state: &AppState, slug: &str) -> AppResult<std::path::PathBuf> {
    let topic = state.0.workspace().resolve(slug)?;
    let dir = topic.dir.join(crate::domain::topic::DIR_LESSONS);
    ensure_dir(&dir)?;
    Ok(dir)
}

/// 读当前进行中的讲解方案（最近更新且未结束的那一份）。
pub fn active_plan(topic_dir: &std::path::Path) -> Option<LessonPlan> {
    let dir = topic_dir.join(crate::domain::topic::DIR_LESSONS);
    let mut plans: Vec<LessonPlan> = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.extension().is_none_or(|e| e != "json") {
            continue;
        }
        if let Ok(Some(plan)) = store::read_json_opt::<LessonPlan>(&p) {
            if !plan.finished {
                plans.push(plan);
            }
        }
    }
    plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    plans.into_iter().next()
}

#[tauri::command]
pub async fn lesson_list(state: State<'_, AppState>, slug: String) -> AppResult<Vec<LessonPlan>> {
    let dir = lessons_dir(&state, &slug)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "json") {
            if let Ok(Some(plan)) = store::read_json_opt::<LessonPlan>(&p) {
                out.push(plan);
            }
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

#[tauri::command]
pub async fn lesson_get(state: State<'_, AppState>, slug: String, id: String) -> AppResult<LessonPlan> {
    let path = lessons_dir(&state, &slug)?.join(format!("{id}.json"));
    store::read_json_opt::<LessonPlan>(&path)?
        .ok_or_else(|| AppError::NotFound(format!("找不到讲解方案 {id}")))
}

/// 保存方案（agent 建方案、用户改状态都走这里）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LessonInput {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub prereqs: Vec<String>,
    #[serde(default)]
    pub steps: Vec<LessonStep>,
    #[serde(default)]
    pub finished: Option<bool>,
}

/// 保存逻辑的纯函数版本：不依赖 Tauri State，工具层直接复用。
pub fn save_plan_inner(topic: &crate::domain::topic::Topic, input: LessonInput) -> AppResult<LessonPlan> {
    let dir = topic.lessons_dir();
    ensure_dir(&dir)?;

    let mut plan = match input
        .id
        .as_deref()
        .and_then(|id| store::read_json_opt::<LessonPlan>(&dir.join(format!("{id}.json"))).ok().flatten())
    {
        Some(p) => p,
        None => LessonPlan {
            id: uuid::Uuid::new_v4().to_string(),
            topic_id: topic.meta.id.clone(),
            title: input.title.clone(),
            goal: String::new(),
            prereqs: Vec::new(),
            steps: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            finished: false,
        },
    };
    plan.title = input.title;
    plan.goal = input.goal;
    plan.prereqs = input.prereqs;
    if !input.steps.is_empty() {
        // 序号按数组顺序重排，避免模型给的 index 乱掉
        let mut steps = input.steps;
        for (i, s) in steps.iter_mut().enumerate() {
            s.index = i as u32 + 1;
        }
        plan.steps = steps;
    }
    if let Some(f) = input.finished {
        plan.finished = f;
    }
    plan.updated_at = Utc::now();

    store::write_json(&dir.join(format!("{}.json", plan.id)), &plan)?;
    Ok(plan)
}

#[tauri::command]
pub async fn lesson_save(
    state: State<'_, AppState>,
    slug: String,
    input: LessonInput,
) -> AppResult<LessonPlan> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let plan = save_plan_inner(&topic, input)?;
    core.emit_topics_updated(&topic);
    Ok(plan)
}

/// 更新某一步的状态（讲完一步 / 跳到某步）。
#[tauri::command]
pub async fn lesson_set_step(
    state: State<'_, AppState>,
    slug: String,
    id: String,
    index: u32,
    status: StepStatus,
) -> AppResult<LessonPlan> {
    let dir = lessons_dir(&state, &slug)?;
    let path = dir.join(format!("{id}.json"));
    let mut plan = store::read_json_opt::<LessonPlan>(&path)?
        .ok_or_else(|| AppError::NotFound(format!("找不到讲解方案 {id}")))?;

    for s in plan.steps.iter_mut() {
        if s.index == index {
            s.status = status;
        } else if status == StepStatus::Doing && s.status == StepStatus::Doing {
            // 同一时间只有一步处于「讲解中」
            s.status = StepStatus::Todo;
        }
    }
    plan.updated_at = Utc::now();
    if plan.steps.iter().all(|s| matches!(s.status, StepStatus::Done | StepStatus::Skipped)) {
        plan.finished = true;
    }
    store::write_json(&path, &plan)?;
    Ok(plan)
}

#[tauri::command]
pub async fn lesson_delete(state: State<'_, AppState>, slug: String, id: String) -> AppResult<()> {
    let path = lessons_dir(&state, &slug)?.join(format!("{id}.json"));
    if path.exists() {
        let trash = state
            .0
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("trash");
        store::move_to_trash(&trash, &path)?;
    }
    Ok(())
}
