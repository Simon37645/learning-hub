//! 记忆卡片 + 间隔重复（SM-2 变体）。
//!
//! 卡片存在 `cards/cards.jsonl`，一枚卡片一行，既能被本应用调度，
//! 也能一键同步到 Anki（`anki` 模块）。调度状态内联在卡片上，所以文件可以单独带走。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrsState {
    /// 难度系数，Anki 里叫 ease factor
    pub ease: f32,
    /// 当前间隔（天）
    pub interval_days: f32,
    /// 连续答对次数
    pub repetitions: u32,
    /// 遗忘次数
    pub lapses: u32,
    /// 到期时间
    pub due: DateTime<Utc>,
    #[serde(default)]
    pub last_review: Option<DateTime<Utc>>,
    /// 累计复习次数
    #[serde(default)]
    pub reviews: u32,
}

impl Default for SrsState {
    fn default() -> Self {
        Self {
            ease: 2.5,
            interval_days: 0.0,
            repetitions: 0,
            lapses: 0,
            due: Utc::now(),
            last_review: None,
            reviews: 0,
        }
    }
}

/// 复习时的四档评分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// 忘了
    Again,
    /// 想起来了但很费劲
    Hard,
    /// 正常想起
    Good,
    /// 太简单
    Easy,
}

impl Grade {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "again" | "忘了" | "1" => Some(Grade::Again),
            "hard" | "困难" | "2" => Some(Grade::Hard),
            "good" | "记得" | "3" => Some(Grade::Good),
            "easy" | "简单" | "4" => Some(Grade::Easy),
            _ => None,
        }
    }
}

impl SrsState {
    /// SM-2 变体：Again 走「10 分钟后再来」，其余按间隔 × ease 推进。
    pub fn apply(&mut self, grade: Grade, now: DateTime<Utc>) {
        self.reviews += 1;
        self.last_review = Some(now);

        match grade {
            Grade::Again => {
                self.lapses += 1;
                self.repetitions = 0;
                self.interval_days = 0.0;
                self.ease = (self.ease - 0.2).max(1.3);
                self.due = now + Duration::minutes(10);
                return;
            }
            Grade::Hard => {
                self.ease = (self.ease - 0.15).max(1.3);
                self.interval_days = if self.interval_days < 1.0 {
                    1.0
                } else {
                    (self.interval_days * 1.2).max(self.interval_days + 0.5)
                };
            }
            Grade::Good => {
                self.repetitions += 1;
                self.interval_days = match self.repetitions {
                    1 => 1.0,
                    2 => 3.0,
                    _ => (self.interval_days * self.ease).max(self.interval_days + 1.0),
                };
            }
            Grade::Easy => {
                self.repetitions += 1;
                self.ease = (self.ease + 0.15).min(3.0);
                self.interval_days = if self.interval_days < 1.0 {
                    4.0
                } else {
                    (self.interval_days * self.ease * 1.3).max(self.interval_days + 2.0)
                };
            }
        }

        self.interval_days = self.interval_days.min(365.0 * 3.0);
        // 小于 1 天的间隔用小时表示，避免 due 被截成「今天 0 点」
        let secs = (self.interval_days * 86_400.0).round() as i64;
        self.due = now + Duration::seconds(secs.max(600));
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.due <= now
    }

    /// 预览「打这一档，下次什么时候再见」——返回距现在的秒数，不改状态。
    ///
    /// 直接把 `apply` 跑在副本上：以后调算法，界面上的按钮文案**自动**跟着变，
    /// 不会出现「按钮写着 10 分钟、实际排到明天」这种两套逻辑打架的情况。
    pub fn preview_secs(&self, grade: Grade, now: DateTime<Utc>) -> i64 {
        let mut probe = self.clone();
        probe.apply(grade, now);
        (probe.due - now).num_seconds().max(0)
    }

    /// 「新卡」= 从没复习过。
    pub fn is_new(&self) -> bool {
        self.reviews == 0
    }
}

/// 卡片类型。决定导出到 Anki 时用哪个笔记模板，也决定背面怎么填。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// 正问反答（Anki 的 Basic）
    #[default]
    Basic,
    /// 反向卡：正反都能问（Anki 的 Basic (and reversed card)）
    Reversed,
    /// 完形填空：正文里用 {{c1::...}} 标出空格（Anki 的 Cloze）
    Cloze,
}

impl CardKind {
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Basic => "基础",
            CardKind::Reversed => "反向",
            CardKind::Cloze => "完形",
        }
    }

    /// 对应的 Anki 笔记模板名
    pub fn anki_model(self) -> &'static str {
        match self {
            CardKind::Basic => "Basic",
            CardKind::Reversed => "Basic (and reversed card)",
            CardKind::Cloze => "Cloze",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "basic" | "基础" | "正反" => Some(CardKind::Basic),
            "reversed" | "reverse" | "反向" => Some(CardKind::Reversed),
            "cloze" | "完形" | "填空" => Some(CardKind::Cloze),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    /// 卡片类型
    #[serde(default)]
    pub kind: CardKind,
    /// 正面：问题 / 提示；完形卡这里放带 {{c1::}} 的整段文字
    pub front: String,
    /// 背面：答案（完形卡通常为空，答案就在标记里）
    pub back: String,
    /// 出处：笔记相对路径、PDF 页码、网址
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 关联的子话题/章节，用于按模块复习
    #[serde(default)]
    pub module: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub srs: SrsState,
    /// 同步到 Anki 后回填的 note id
    #[serde(default)]
    pub anki_note_id: Option<i64>,
}

impl Card {
    pub fn new(front: impl Into<String>, back: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: CardKind::Basic,
            front: front.into(),
            back: back.into(),
            source: None,
            tags: Vec::new(),
            module: None,
            created_at: Utc::now(),
            srs: SrsState::default(),
            anki_note_id: None,
        }
    }

    /// 完形卡：正文里用 {{c1::...}} 标出要挖空的部分。
    pub fn new_cloze(text: impl Into<String>) -> Self {
        let mut card = Self::new(text, "");
        card.kind = CardKind::Cloze;
        card
    }

    /// 去重用的指纹：正反面归一化后比较，避免同一张卡被反复存。
    pub fn fingerprint(&self) -> String {
        let norm = |s: &str| {
            s.chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        };
        format!("{:?}||{}||{}", self.kind, norm(&self.front), norm(&self.back))
    }

    /// 拆成 Anki 的字段表（不同模板字段名不同）。
    pub fn anki_fields(&self) -> std::collections::BTreeMap<String, String> {
        let mut map = std::collections::BTreeMap::new();
        match self.kind {
            CardKind::Cloze => {
                map.insert("Text".into(), self.front.clone());
                map.insert("Back Extra".into(), self.back.clone());
            }
            _ => {
                map.insert("Front".into(), self.front.clone());
                map.insert("Back".into(), self.back.clone());
            }
        }
        map
    }

    /// 校验：完形卡必须有至少一个 {{cN::}} 标记，否则到 Anki 那边会变成空白卡。
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self.kind, CardKind::Cloze) && !has_cloze_marker(&self.front) {
            return Err("完形卡必须在正面里用 {{c1::要挖空的内容}} 标出空格".into());
        }
        if !matches!(self.kind, CardKind::Cloze) && self.front.trim().is_empty() {
            return Err("卡片正面不能为空".into());
        }
        Ok(())
    }
}

/// 是否存在 `{{c1::...}}` 形式的完形标记。
pub fn has_cloze_marker(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let rest = &text[i + 2..];
            let mut chars = rest.chars();
            if let Some(c) = chars.next() {
                if c == 'c' {
                    // c 后面必须紧跟数字与 "::"
                    let digits: String = chars.clone().take_while(|ch| ch.is_ascii_digit()).collect();
                    if !digits.is_empty() {
                        let after = chars.skip(digits.len()).collect::<String>();
                        if after.starts_with("::") {
                            return true;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm2_progression() {
        let mut s = SrsState::default();
        let t0 = Utc::now();
        s.apply(Grade::Good, t0);
        assert_eq!(s.interval_days, 1.0);
        s.apply(Grade::Good, t0);
        assert_eq!(s.interval_days, 3.0);
        s.apply(Grade::Good, t0);
        assert!(s.interval_days > 7.0);
        let before = s.ease;
        s.apply(Grade::Again, t0);
        assert!(s.ease < before);
        assert_eq!(s.repetitions, 0);
        assert!(s.due <= t0 + Duration::minutes(11));
    }

    /// 复习按钮上的「10 分钟 / 1 天 / 4 天」来自 preview_secs，
    /// 它必须和真正执行 apply 的结果一致，而且不能改状态。
    #[test]
    fn preview_matches_apply() {
        let now = Utc::now();
        let fresh = SrsState::default();
        assert_eq!(fresh.preview_secs(Grade::Again, now), 600);
        assert_eq!(fresh.preview_secs(Grade::Hard, now), 86_400);
        assert_eq!(fresh.preview_secs(Grade::Good, now), 86_400);
        assert_eq!(fresh.preview_secs(Grade::Easy, now), 4 * 86_400);

        // 预览是只读的
        let _ = fresh.preview_secs(Grade::Easy, now);
        assert_eq!(fresh.interval_days, 0.0);
        assert_eq!(fresh.repetitions, 0);
        assert_eq!(fresh.reviews, 0);

        // 与真实执行逐档一致
        for grade in [Grade::Again, Grade::Hard, Grade::Good, Grade::Easy] {
            let mut real = fresh.clone();
            real.apply(grade, now);
            assert_eq!(
                fresh.preview_secs(grade, now),
                (real.due - now).num_seconds(),
                "{grade:?} 的预览与执行结果不一致"
            );
        }
    }
}
