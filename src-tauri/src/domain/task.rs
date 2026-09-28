//! 日程与任务：`plan/tasks.jsonl`。
//!
//! 刻意保持极简（没有日历后端、没有同步），因为学习计划的高频操作只有三件事：
//! 今天要做什么、什么时候截止、做完没有。

use crate::domain::stage::StudyStage;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Todo,
    Doing,
    Done,
    Archived,
}

impl TaskStatus {
    pub fn is_open(self) -> bool {
        matches!(self, TaskStatus::Todo | TaskStatus::Doing)
    }
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Todo => "待办",
            TaskStatus::Doing => "进行中",
            TaskStatus::Done => "已完成",
            TaskStatus::Archived => "已归档",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "todo" | "待办" | "未开始" => Some(TaskStatus::Todo),
            "doing" | "进行中" | "在做" => Some(TaskStatus::Doing),
            "done" | "已完成" | "完成" => Some(TaskStatus::Done),
            "archived" | "归档" => Some(TaskStatus::Archived),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub status: TaskStatus,
    /// 1=低 2=中 3=高
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// 截止时间（可空）
    #[serde(default)]
    pub due: Option<DateTime<Utc>>,
    /// 预计耗时（分钟）
    #[serde(default)]
    pub estimate_min: Option<u32>,
    /// 归属的学习阶段，用于「今天该预习还是复习」
    #[serde(default)]
    pub stage: Option<StudyStage>,
    /// 关联的子话题/章节
    #[serde(default)]
    pub module: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub done_at: Option<DateTime<Utc>>,
}

fn default_priority() -> u8 {
    2
}

impl PlanTask {
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            detail: String::new(),
            status: TaskStatus::Todo,
            priority: default_priority(),
            due: None,
            estimate_min: None,
            stage: None,
            module: None,
            created_at: now,
            updated_at: now,
            done_at: None,
        }
    }

    /// 读一个 tasks.jsonl，按「先逾期、再按截止时间」排序。
    pub fn load_all(path: &std::path::Path) -> Vec<PlanTask> {
        crate::store::read_jsonl::<PlanTask>(path).unwrap_or_default()
    }

    /// 只取未完成的（todo/doing），已排序。
    pub fn open_list(path: &std::path::Path) -> Vec<PlanTask> {
        let mut v: Vec<PlanTask> = Self::load_all(path)
            .into_iter()
            .filter(|t| t.status.is_open())
            .collect();
        v.sort_by(|a, b| match (a.due, b.due) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.priority.cmp(&a.priority),
        });
        v
    }

    pub fn is_overdue(&self, now: DateTime<Utc>) -> bool {
        self.status.is_open() && self.due.is_some_and(|d| d < now)
    }

    pub fn due_date(&self) -> Option<NaiveDate> {
        self.due.map(|d| d.date_naive())
    }
}

/// 日程分组：把任务按「逾期 / 今天 / 明天 / 本周 / 以后 / 无期限」归档，
/// 前端直接按这个顺序渲染。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaBucket {
    pub key: String,
    pub label: String,
    pub tasks: Vec<AgendaItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaItem {
    pub task: PlanTask,
    pub topic_slug: String,
    pub topic_name: String,
}

/// 任务属于哪个日程分组。`today` 用本地日期，因为「今天」是使用者的今天。
pub fn bucket_of(task: &PlanTask, today: NaiveDate, _now: DateTime<Utc>) -> &'static str {
    if task.status == TaskStatus::Done || task.status == TaskStatus::Archived {
        return "done";
    }
    match task.due_date() {
        None => "someday",
        Some(d) => {
            let delta = (d - today).num_days();
            if delta < 0 {
                "overdue"
            } else if delta == 0 {
                "today"
            } else if delta == 1 {
                "tomorrow"
            } else if delta <= 7 {
                "week"
            } else {
                "later"
            }
        }
    }
}

pub const BUCKETS: [(&str, &str); 7] = [
    ("overdue", "已逾期"),
    ("today", "今天"),
    ("tomorrow", "明天"),
    ("week", "本周"),
    ("later", "以后"),
    ("someday", "未排期"),
    ("done", "已完成"),
];
