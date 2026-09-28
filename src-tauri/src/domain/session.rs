//! 学习会话：一次「预习 / 学习 / 复习 / 测验」的完整记录。
//!
//! 会话是学习中枢的数据主线：它把「聊了什么」（`.hub/chats/<chatId>.jsonl`）
//! 与「产出了什么」（笔记、卡片、任务）串起来，后续的复习计划与周报都从这里长出来。

use crate::domain::stage::StudyStage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudySession {
    pub id: String,
    pub topic_id: String,
    /// 目录名，方便直接定位
    pub topic_slug: String,
    pub title: String,
    pub stage: StudyStage,
    /// 本次会话想达成的目标（agent 会据此组织内容）
    #[serde(default)]
    pub goals: Vec<String>,
    /// 用到的资料（相对主题目录的路径）
    #[serde(default)]
    pub materials: Vec<String>,
    /// 关联的对话记录 id
    pub chat_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    /// 结束时 agent 生成的总结
    #[serde(default)]
    pub summary: String,
    /// 本次会话的要点
    #[serde(default)]
    pub highlights: Vec<String>,
    /// 悬而未决的问题（下次接着解决）
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub cards_created: u32,
    #[serde(default)]
    pub notes_created: u32,
}

impl StudySession {
    pub fn new(
        topic_id: impl Into<String>,
        topic_slug: impl Into<String>,
        title: impl Into<String>,
        stage: StudyStage,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            topic_id: topic_id.into(),
            topic_slug: topic_slug.into(),
            title: title.into(),
            stage,
            goals: Vec::new(),
            materials: Vec::new(),
            chat_id: uuid::Uuid::new_v4().to_string(),
            started_at: Utc::now(),
            ended_at: None,
            summary: String::new(),
            highlights: Vec::new(),
            open_questions: Vec::new(),
            cards_created: 0,
            notes_created: 0,
        }
    }

    pub fn duration_minutes(&self) -> i64 {
        let end = self.ended_at.unwrap_or_else(Utc::now);
        (end - self.started_at).num_minutes().max(0)
    }
}
