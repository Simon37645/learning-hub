//! 测验：题型、试卷、作答与判分。
//!
//! 题型按医学考试的习惯分四类客观题 + 两类主观题：
//! - `A1`：单句型最佳选择题（一个题干、五个选项、一个答案）
//! - `A2`：病例摘要型最佳选择题（先给一段病例，再问一个问题）
//! - `B`：标准配伍题（一组选项配若干小题，共用选项）
//! - `X`：多项选择题（可多选，答案是一个集合）
//! - `Term`：名词解释
//! - `Short`：简答题
//!
//! 判分策略：客观题本地按集合比对（不花 token、结果确定）；
//! 主观题把「要点清单」交给模型逐条对照给分，并把给分理由写回来——
//! 这样用户既能看到分数，也知道自己漏了哪一条。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    A1,
    A2,
    B,
    X,
    /// 名词解释
    Term,
    /// 简答题
    Short,
}

impl QuestionType {
    pub fn is_subjective(self) -> bool {
        matches!(self, QuestionType::Term | QuestionType::Short)
    }

    pub fn label(self) -> &'static str {
        match self {
            QuestionType::A1 => "A1 型",
            QuestionType::A2 => "A2 型",
            QuestionType::B => "B 型",
            QuestionType::X => "X 型",
            QuestionType::Term => "名词解释",
            QuestionType::Short => "简答",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "A1" => Some(QuestionType::A1),
            "A2" => Some(QuestionType::A2),
            "B" | "B1" | "B2" => Some(QuestionType::B),
            "X" => Some(QuestionType::X),
            "TERM" | "名词解释" => Some(QuestionType::Term),
            "SHORT" | "简答" | "简答题" => Some(QuestionType::Short),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizOption {
    /// 选项号：A / B / C / D / E
    pub key: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: QuestionType,
    /// 题干。A2 型只写问题本身，病例放在 case_text
    pub stem: String,
    /// A2 型的病例摘要
    #[serde(default)]
    pub case_text: Option<String>,
    #[serde(default)]
    pub options: Vec<QuizOption>,
    /// 正确答案：客观题是选项号的集合，主观题留空
    #[serde(default)]
    pub answer: Vec<String>,
    /// 主观题的采分点，判分时逐条对照
    #[serde(default)]
    pub key_points: Vec<String>,
    /// 解析
    #[serde(default)]
    pub explanation: String,
    /// 出处，例如 materials/ch1.pdf 第 12 页
    #[serde(default)]
    pub source: Option<String>,
    /// 分值
    #[serde(default = "default_score")]
    pub score: u32,
    /// B 型题：同组小题共用选项，用 group 串起来
    #[serde(default)]
    pub group: Option<String>,
}

fn default_score() -> u32 {
    1
}

impl Question {
    pub fn new(kind: QuestionType, stem: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            stem: stem.into(),
            case_text: None,
            options: Vec::new(),
            answer: Vec::new(),
            key_points: Vec::new(),
            explanation: String::new(),
            source: None,
            score: default_score(),
            group: None,
        }
    }

    /// 客观题作答是否算对。多选题必须完全一致（多选、漏选都不给分）。
    pub fn is_correct(&self, answer: &str) -> bool {
        let given = normalize_keys(answer);
        let expect = normalize_keys(&self.answer.join(""));
        !expect.is_empty() && given == expect
    }
}

/// 把 "A,C" / "AC" / "a c" 统一成排序去重后的 "AC"。
pub fn normalize_keys(raw: &str) -> String {
    let mut keys: Vec<char> = raw
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuizKind {
    /// 随堂练习：按当前进度抽题
    #[default]
    Practice,
    /// 模拟考试：更正式，按题型配比
    Exam,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quiz {
    pub id: String,
    pub topic_id: String,
    pub title: String,
    #[serde(default)]
    pub kind: QuizKind,
    /// 出题范围说明，便于回看「这份卷子覆盖了什么」
    #[serde(default)]
    pub scope: String,
    pub questions: Vec<Question>,
    pub created_at: DateTime<Utc>,
    /// 试卷的来源：agent 生成 / 用户手写
    #[serde(default)]
    pub created_by: String,
}

impl Quiz {
    pub fn new(topic_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            topic_id: topic_id.into(),
            title: title.into(),
            kind: QuizKind::Practice,
            scope: String::new(),
            questions: Vec::new(),
            created_at: Utc::now(),
            created_by: "agent".into(),
        }
    }

    pub fn total_score(&self) -> u32 {
        self.questions.iter().map(|q| q.score).sum()
    }

    pub fn type_summary(&self) -> String {
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for q in &self.questions {
            *counts.entry(q.kind.label()).or_default() += 1;
        }
        counts
            .into_iter()
            .map(|(k, v)| format!("{k}×{v}"))
            .collect::<Vec<_>>()
            .join("，")
    }
}

/// 一次作答。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub question_id: String,
    /// 客观题是选项号（如 "AC"），主观题是文本
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionResult {
    pub question_id: String,
    /// 客观题的对错；主观题为 None（等 agent 评分）
    pub correct: Option<bool>,
    pub score: f32,
    pub max_score: f32,
    /// 判分说明（主观题是 agent 写的评语，客观题是标准答案对照）
    pub comment: String,
    /// 主观题漏掉的采分点
    #[serde(default)]
    pub missing: Vec<String>,
    /// 是否已经由 agent 评过分（主观题用）
    #[serde(default)]
    pub graded_by_agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub id: String,
    pub quiz_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    pub answers: Vec<Answer>,
    pub results: Vec<QuestionResult>,
    pub score: f32,
    pub total: f32,
    #[serde(default)]
    pub pending_subjective: Vec<String>,
}

impl Attempt {
    pub fn new(quiz_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            quiz_id: quiz_id.into(),
            started_at: Utc::now(),
            finished_at: None,
            answers: Vec::new(),
            results: Vec::new(),
            score: 0.0,
            total: 0.0,
            pending_subjective: Vec::new(),
        }
    }

    /// 客观题本地判分，主观题先挂起等模型。
    pub fn grade_objective(&mut self, quiz: &Quiz) {
        self.results.clear();
        self.pending_subjective.clear();
        let mut score = 0.0f32;
        let mut total = 0.0f32;

        for q in &quiz.questions {
            let max = q.score as f32;
            total += max;
            let given = self
                .answers
                .iter()
                .find(|a| a.question_id == q.id)
                .map(|a| a.value.clone())
                .unwrap_or_default();

            if q.kind.is_subjective() {
                self.pending_subjective.push(q.id.clone());
                self.results.push(QuestionResult {
                    question_id: q.id.clone(),
                    correct: None,
                    score: 0.0,
                    max_score: max,
                    comment: "待评分".into(),
                    missing: Vec::new(),
                    graded_by_agent: false,
                });
                continue;
            }

            let ok = q.is_correct(&given);
            if ok {
                score += max;
            }
            self.results.push(QuestionResult {
                question_id: q.id.clone(),
                correct: Some(ok),
                score: if ok { max } else { 0.0 },
                max_score: max,
                comment: if ok {
                    format!("正确。标准答案：{}", q.answer.join(""))
                } else {
                    format!("你的作答：{}；标准答案：{}", given, q.answer.join(""))
                },
                missing: Vec::new(),
                graded_by_agent: false,
            });
        }

        self.score = score;
        self.total = total;
        self.finished_at = Some(Utc::now());
    }

    pub fn is_fully_graded(&self) -> bool {
        self.pending_subjective.is_empty() || self.results.iter().all(|r| r.graded_by_agent)
    }

    /// 写回主观题评分。
    pub fn apply_subjective(&mut self, question_id: &str, score: f32, comment: String, missing: Vec<String>) {
        if let Some(r) = self.results.iter_mut().find(|r| r.question_id == question_id) {
            r.score = score.clamp(0.0, r.max_score);
            r.comment = comment;
            r.missing = missing;
            r.graded_by_agent = true;
            r.correct = Some(r.score >= r.max_score - 0.01);
        }
        self.pending_subjective.retain(|q| q != question_id);
        self.score = self.results.iter().map(|r| r.score).sum();
    }

    pub fn percent(&self) -> f32 {
        if self.total <= 0.0 {
            0.0
        } else {
            (self.score / self.total * 100.0).round()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a1() -> Question {
        let mut q = Question::new(QuestionType::A1, "1+1=?");
        q.options = vec![
            QuizOption { key: "A".into(), text: "1".into() },
            QuizOption { key: "B".into(), text: "2".into() },
        ];
        q.answer = vec!["B".into()];
        q
    }

    #[test]
    fn objective_grading() {
        let q = a1();
        assert!(q.is_correct("B"));
        assert!(q.is_correct("b"));
        assert!(!q.is_correct("A"));
        assert!(!q.is_correct(""));
    }

    #[test]
    fn x_type_needs_exact_set() {
        let mut q = Question::new(QuestionType::X, "多选");
        q.answer = vec!["A".into(), "C".into()];
        assert!(q.is_correct("CA"));
        assert!(q.is_correct("a, c"));
        assert!(!q.is_correct("A")); // 漏选不给分
        assert!(!q.is_correct("ABC")); // 多选不给分
    }

    #[test]
    fn attempt_scores_objective_and_holds_subjective() {
        let mut quiz = Quiz::new("t", "小测");
        quiz.questions = vec![a1(), Question::new(QuestionType::Short, "解释一下")];
        let mut attempt = Attempt::new(&quiz.id);
        attempt.answers = vec![
            Answer { question_id: quiz.questions[0].id.clone(), value: "B".into() },
            Answer { question_id: quiz.questions[1].id.clone(), value: "因为…".into() },
        ];
        attempt.grade_objective(&quiz);
        assert_eq!(attempt.score, 1.0);
        assert_eq!(attempt.total, 2.0);
        assert_eq!(attempt.pending_subjective.len(), 1);
        assert!(!attempt.is_fully_graded());

        let qid = quiz.questions[1].id.clone();
        attempt.apply_subjective(&qid, 0.5, "漏了一点".into(), vec!["第二点".into()]);
        assert!(attempt.is_fully_graded());
        assert_eq!(attempt.score, 1.5);
    }
}
