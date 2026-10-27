//! 记忆卡片 + 间隔重复（SM-2 变体）。
//!
//! 卡片存在 `cards/cards.jsonl`，一枚卡片一行，既能被本应用调度，
//! 调度状态内联在卡片上，所以文件可以单独带走。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrsState {
    /// 难度系数（ease factor）
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
                // Hard 也是**成功回忆**（SM-2 的 quality≥3、Anki 同义），计进连续
                // 答对次数——不计数的话，连按几次「吃力」攒起来的间隔会在下一次
                // 「记得」时被阶梯（repetitions==1 → 1 天）整段拍回去：更好的评分
                // 得到更短的间隔，复习节奏倒退。
                self.repetitions += 1;
                self.interval_days = if self.interval_days < 1.0 {
                    1.0
                } else {
                    (self.interval_days * 1.2).max(self.interval_days + 0.5)
                };
            }
            Grade::Good => {
                self.repetitions += 1;
                self.interval_days = match self.repetitions {
                    // 「第一次成功记 1 天」只对**确实还没起步**的卡生效：beta 期间
                    // 产生的存量数据里有「连按过多次吃力（当时 Hard 不计数）、
                    // 间隔已爬起来而 repetitions 仍为 0」的卡，这里再按一次
                    // 「记得」不能把它拍回 1 天。
                    1 if self.interval_days < 1.0 => 1.0,
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

/// 卡片类型：决定这张卡怎么问、背面怎么填。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// 正问反答
    #[default]
    Basic,
    /// 反向卡：正反都能问
    Reversed,
    /// 完形填空：正文里用 {{c1::...}} 标出要挖空的部分
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

    /// 校验：完形卡必须至少有一个 {{cN::}} 标记，否则这张卡要么没得问、要么和普通卡重复。
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

    /// 连按「吃力」攒起来的间隔，在下一次「记得」时不能被拍回 1 天：
    /// Hard 是成功回忆（要计数），「记得」的按钮语义是「正常推进」——
    /// 更好的评分得到更短的间隔就是节奏倒退。
    #[test]
    fn hard_streak_then_good_keeps_progressing() {
        let t0 = Utc::now();
        let mut s = SrsState::default();
        for _ in 0..8 {
            s.apply(Grade::Hard, t0);
        }
        assert!(s.interval_days > 3.0, "连按吃力间隔应该爬起来：{}", s.interval_days);
        let before = s.interval_days;
        s.apply(Grade::Good, t0);
        assert!(
            s.interval_days > before,
            "「记得」不能比「吃力」排得更近：{} → {}",
            before,
            s.interval_days
        );
        // 最小复现：2 连 Hard（1.5 → 1.8）后按「记得」也不能倒退
        let mut s2 = SrsState::default();
        s2.apply(Grade::Hard, t0);
        s2.apply(Grade::Hard, t0);
        let before2 = s2.interval_days;
        s2.apply(Grade::Good, t0);
        assert!(s2.interval_days > before2, "{} → {}", before2, s2.interval_days);
        // 复学路径：忘了 → 吃力 → 吃力 → 记得，一路单调不倒退
        let mut s3 = SrsState::default();
        s3.apply(Grade::Again, t0);
        s3.apply(Grade::Hard, t0);
        s3.apply(Grade::Hard, t0);
        let before3 = s3.interval_days;
        s3.apply(Grade::Good, t0);
        assert!(s3.interval_days > before3, "{} → {}", before3, s3.interval_days);
        // 存量数据：beta 期间「Hard 不计数」留下的卡——间隔已爬起来、repetitions
        // 仍为 0。再按「记得」也不能被阶梯拍回 1 天
        let legacy = SrsState {
            repetitions: 0,
            interval_days: 5.18,
            ..SrsState::default()
        };
        let mut legacy = legacy;
        legacy.apply(Grade::Good, t0);
        assert!(legacy.interval_days > 5.18, "{}", legacy.interval_days);

        // 新卡与 Again 清零后的阶梯保持原语义：1 天 → 3 天
        let mut s4 = SrsState::default();
        s4.apply(Grade::Good, t0);
        assert_eq!(s4.interval_days, 1.0);
        s4.apply(Grade::Good, t0);
        assert_eq!(s4.interval_days, 3.0);
        let mut s5 = SrsState::default();
        s5.apply(Grade::Again, t0);
        s5.apply(Grade::Good, t0);
        assert_eq!(s5.interval_days, 1.0);
        s5.apply(Grade::Good, t0);
        assert_eq!(s5.interval_days, 3.0);
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
