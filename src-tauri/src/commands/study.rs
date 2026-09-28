//! 学习资产命令：卡片（间隔重复）、计划任务、学习会话。

use crate::domain::card::{Card, Grade};
use crate::domain::session::StudySession;
use crate::domain::stage::StudyStage;
use crate::domain::task::{AgendaBucket, AgendaItem, PlanTask, TaskStatus};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store;
use chrono::{Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;

// ============================================================ 卡片

#[tauri::command]
pub async fn card_list(
    state: State<'_, AppState>,
    slug: String,
    due_only: Option<bool>,
    query: Option<String>,
) -> AppResult<Vec<Card>> {
    let topic = state.0.workspace().resolve(&slug)?;
    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let now = Utc::now();
    if due_only.unwrap_or(false) {
        cards.retain(|c| c.srs.is_due(now));
    }
    if let Some(q) = query.map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty()) {
        cards.retain(|c| {
            c.front.to_lowercase().contains(&q)
                || c.back.to_lowercase().contains(&q)
                || c.tags.iter().any(|t| t.to_lowercase().contains(&q))
        });
    }
    // 排序：先到期的（最早到期在前），再新卡，最后是还没到期的
    let rank = |c: &Card| {
        if c.srs.is_due(now) {
            0
        } else if c.srs.is_new() {
            1
        } else {
            2
        }
    };
    cards.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.srs.due.cmp(&b.srs.due)));
    Ok(cards)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardInput {
    pub front: String,
    #[serde(default)]
    pub back: String,
    /// basic / reversed / cloze，默认 basic
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

fn parse_kind(raw: Option<&str>) -> crate::domain::card::CardKind {
    raw.and_then(crate::domain::card::CardKind::parse)
        .unwrap_or_default()
}

#[tauri::command]
pub async fn card_create(
    state: State<'_, AppState>,
    slug: String,
    input: CardInput,
) -> AppResult<Card> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let kind = parse_kind(input.kind.as_deref());

    let mut card = match kind {
        crate::domain::card::CardKind::Cloze => Card::new_cloze(input.front.trim()),
        _ => {
            let mut c = Card::new(input.front.trim(), input.back.trim());
            c.kind = kind;
            c
        }
    };
    card.tags = input.tags;
    card.module = input.module;
    card.source = input.source;
    card.validate().map_err(AppError::invalid)?;

    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let fp = card.fingerprint();
    if cards.iter().any(|c| c.fingerprint() == fp) {
        return Err(AppError::invalid("这张卡片已经存在了"));
    }
    cards.push(card.clone());
    store::write_jsonl(&topic.cards_path(), &cards)?;
    core.emit_topics_updated(&topic);
    Ok(card)
}

#[tauri::command]
pub async fn card_update(
    state: State<'_, AppState>,
    slug: String,
    id: String,
    input: CardInput,
) -> AppResult<Card> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let kind = parse_kind(input.kind.as_deref());
    let card = cards
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::NotFound(format!("找不到卡片 {id}")))?;
    card.kind = kind;
    card.front = input.front.trim().to_string();
    card.back = input.back.trim().to_string();
    card.tags = input.tags;
    card.module = input.module;
    card.source = input.source;
    card.validate().map_err(AppError::invalid)?;
    let out = card.clone();
    store::write_jsonl(&topic.cards_path(), &cards)?;
    Ok(out)
}

#[tauri::command]
pub async fn card_delete(
    state: State<'_, AppState>,
    slug: String,
    ids: Vec<String>,
) -> AppResult<usize> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let before = cards.len();
    cards.retain(|c| !ids.contains(&c.id));
    let removed = before - cards.len();
    store::write_jsonl(&topic.cards_path(), &cards)?;
    core.emit_topics_updated(&topic);
    Ok(removed)
}

/// 记录一次复习。`grade` 接受 again/hard/good/easy 或 1~4。
#[tauri::command]
pub async fn card_review(
    state: State<'_, AppState>,
    slug: String,
    id: String,
    grade: String,
) -> AppResult<Card> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let parsed = Grade::parse(&grade)
        .ok_or_else(|| AppError::invalid(format!("未知评分：{grade}（用 again/hard/good/easy）")))?;
    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let card = cards
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::NotFound(format!("找不到卡片 {id}")))?;
    card.srs.apply(parsed, Utc::now());
    let out = card.clone();
    store::write_jsonl(&topic.cards_path(), &cards)?;
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiExport {
    pub path: String,
    pub count: usize,
    /// Anki 导入时的字段顺序提示
    pub format: String,
}

/// 检查 Anki 那边的连接状态（设置页与卡片页都会用）。
#[tauri::command]
pub async fn anki_status(state: State<'_, AppState>) -> AppResult<crate::anki::AnkiStatus> {
    let core = state.0.clone();
    let url = core.config_read().anki.url.clone();
    Ok(crate::anki::status(&core.http, &url).await)
}

/// 把还没同步过的卡片推进 Anki。
///
/// 已经同步过的（带 ankiNoteId）会跳过：调度权交给 Anki 之后，
/// 反复推送只会制造重复卡片。
#[tauri::command]
pub async fn anki_sync(
    state: State<'_, AppState>,
    slug: String,
    deck: Option<String>,
) -> AppResult<crate::anki::SyncOutcome> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let cfg = core.config_read();
    let url = cfg.anki.url.clone();
    let deck_name = deck
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| format!("{}{}", cfg.anki.deck_prefix, topic.meta.name));

    let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    let pending: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.anki_note_id.is_none())
        .map(|(i, _)| i)
        .collect();

    if pending.is_empty() {
        return Ok(crate::anki::SyncOutcome {
            added: 0,
            skipped: cards.len(),
            failed: Vec::new(),
            deck: deck_name,
        });
    }

    // 校验：完形卡没有 {{c1::}} 标记的话，到 Anki 就是一张空白卡
    let mut failed: Vec<String> = Vec::new();
    let mut notes = Vec::new();
    let mut note_index: Vec<usize> = Vec::new();
    for i in &pending {
        let card = &cards[*i];
        if let Err(e) = card.validate() {
            failed.push(format!("{}：{e}", card.front.chars().take(30).collect::<String>()));
            continue;
        }
        let mut tags: Vec<String> = card.tags.clone();
        if let Some(m) = &card.module {
            tags.push(m.clone());
        }
        tags.push("学习中枢".to_string());
        tags.push(topic.meta.name.clone());
        notes.push(serde_json::json!({
            "deckName": deck_name,
            "modelName": card.kind.anki_model(),
            "fields": card.anki_fields(),
            "tags": tags,
            "options": { "allowDuplicate": false },
        }));
        note_index.push(*i);
    }

    let ids = crate::anki::add_notes(&core.http, &url, &deck_name, notes).await?;
    let mut added = 0usize;
    for (pos, id) in ids.iter().enumerate() {
        match id {
            Some(nid) => {
                if let Some(ci) = note_index.get(pos) {
                    cards[*ci].anki_note_id = Some(*nid);
                    added += 1;
                }
            }
            None => {
                if let Some(ci) = note_index.get(pos) {
                    failed.push(format!(
                        "{}（Anki 判定为重复，未添加）",
                        cards[*ci].front.chars().take(30).collect::<String>()
                    ));
                }
            }
        }
    }
    if added > 0 {
        store::write_jsonl(&topic.cards_path(), &cards)?;
        core.emit_topics_updated(&topic);
    }

    Ok(crate::anki::SyncOutcome {
        added,
        skipped: cards.len() - pending.len(),
        failed,
        deck: deck_name,
    })
}

/// 测试 AnkiConnect 是否可达（设置页的「测试连接」）。
#[tauri::command]
pub async fn anki_ping(state: State<'_, AppState>) -> AppResult<String> {
    let core = state.0.clone();
    let url = core.config_read().anki.url.clone();
    crate::anki::ping(&core.http, &url).await
}

/// 导出为 Anki 可导入的 TSV（正面<TAB>背面<TAB>标签）。
///
/// 选择文件导出而不是直接调 AnkiConnect：不依赖 Anki 是否开着，
/// 用户自己决定导到哪个牌组、什么时候导。
#[tauri::command]
pub async fn card_export_anki(state: State<'_, AppState>, slug: String) -> AppResult<AnkiExport> {
    let topic = state.0.workspace().resolve(&slug)?;
    let cards = store::read_jsonl::<Card>(&topic.cards_path())?;
    if cards.is_empty() {
        return Err(AppError::invalid("这个主题还没有卡片"));
    }
    let mut out = String::new();
    for c in &cards {
        let front = c.front.replace(['\t', '\n'], " ");
        let back = c.back.replace(['\t', '\n'], " ");
        let mut tags: Vec<String> = c.tags.clone();
        if let Some(m) = &c.module {
            tags.push(m.clone());
        }
        tags.push("学习中枢".to_string());
        tags.push(topic.meta.name.clone());
        out.push_str(&format!("{front}\t{back}\t{}\n", tags.join(" ")));
    }
    let path = topic.dir.join("cards").join("anki-import.tsv");
    store::atomic_write(&path, out.as_bytes())?;
    Ok(AnkiExport {
        path: path.to_string_lossy().to_string(),
        count: cards.len(),
        format: "字段顺序：正面 / 背面 / 标签（制表符分隔，UTF-8）".into(),
    })
}

// ============================================================ 计划任务

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWithTopic {
    pub topic_slug: String,
    pub topic_name: String,
    pub task: PlanTask,
    pub overdue: bool,
}

/// 列出任务。不给 slug 就跨主题汇总（默认只看未完成）。
#[tauri::command]
pub async fn task_list(
    state: State<'_, AppState>,
    slug: Option<String>,
    status: Option<String>,
    include_done: Option<bool>,
) -> AppResult<Vec<TaskWithTopic>> {
    let core = state.0.clone();
    let ws = core.workspace();
    let topics = match slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => vec![ws.resolve(s)?],
        None => ws
            .list()?
            .iter()
            .filter_map(|s| ws.load(&s.slug).ok())
            .collect(),
    };
    let want = status.as_deref().and_then(TaskStatus::parse);
    let include_done = include_done.unwrap_or(false);
    let now = Utc::now();

    let mut out = Vec::new();
    for t in topics {
        for task in PlanTask::load_all(&t.tasks_path()) {
            let keep = match want {
                Some(st) => task.status == st,
                None => include_done || task.status.is_open(),
            };
            if !keep {
                continue;
            }
            out.push(TaskWithTopic {
                topic_slug: t.slug(),
                topic_name: t.meta.name.clone(),
                overdue: task.is_overdue(now),
                task,
            });
        }
    }
    out.sort_by(|a, b| match (a.task.due, b.task.due) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.task.priority.cmp(&a.task.priority),
    });
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInput {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub estimate_min: Option<u32>,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
}

#[tauri::command]
pub async fn task_create(
    state: State<'_, AppState>,
    slug: String,
    input: TaskInput,
) -> AppResult<PlanTask> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let title = input.title.trim();
    if title.is_empty() {
        return Err(AppError::invalid("任务标题不能为空"));
    }
    let mut task = PlanTask::new(title);
    task.detail = input.detail;
    task.priority = input.priority.unwrap_or(2).clamp(1, 3);
    task.estimate_min = input.estimate_min;
    task.module = input.module;
    task.stage = input.stage.as_deref().and_then(StudyStage::parse);
    task.due = input.due.as_deref().and_then(crate::agent::tools::study::parse_due);
    store::append_jsonl(&topic.tasks_path(), &task)?;
    core.emit_topics_updated(&topic);
    Ok(task)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    pub title: Option<String>,
    pub detail: Option<String>,
    pub status: Option<String>,
    pub due: Option<String>,
    pub priority: Option<u8>,
    pub estimate_min: Option<u32>,
}

#[tauri::command]
pub async fn task_update(
    state: State<'_, AppState>,
    slug: String,
    id: String,
    patch: TaskPatch,
) -> AppResult<PlanTask> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let mut tasks = PlanTask::load_all(&topic.tasks_path());
    let task = tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| AppError::NotFound(format!("找不到任务 {id}")))?;

    if let Some(t) = patch.title.filter(|t| !t.trim().is_empty()) {
        task.title = t;
    }
    if let Some(d) = patch.detail {
        task.detail = d;
    }
    if let Some(s) = patch.status.as_deref() {
        let st = TaskStatus::parse(s).ok_or_else(|| AppError::invalid(format!("未知状态：{s}")))?;
        task.status = st;
        task.done_at = if st == TaskStatus::Done { Some(Utc::now()) } else { None };
    }
    if let Some(d) = patch.due.as_deref() {
        task.due = if d.trim().is_empty() {
            None
        } else {
            crate::agent::tools::study::parse_due(d)
        };
    }
    if let Some(p) = patch.priority {
        task.priority = p.clamp(1, 3);
    }
    if let Some(e) = patch.estimate_min {
        task.estimate_min = Some(e);
    }
    task.updated_at = Utc::now();
    let out = task.clone();
    store::write_jsonl(&topic.tasks_path(), &tasks)?;
    core.emit_topics_updated(&topic);
    Ok(out)
}

#[tauri::command]
pub async fn task_delete(state: State<'_, AppState>, slug: String, id: String) -> AppResult<bool> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let mut tasks = PlanTask::load_all(&topic.tasks_path());
    let before = tasks.len();
    tasks.retain(|t| t.id != id);
    if tasks.len() == before {
        return Ok(false);
    }
    store::write_jsonl(&topic.tasks_path(), &tasks)?;
    core.emit_topics_updated(&topic);
    Ok(true)
}

// ============================================================ 日程视图

/// 把未来若干天的任务按「逾期/今天/明天/本周/以后/未排期/已完成」分组。
#[tauri::command]
pub async fn agenda(state: State<'_, AppState>, horizon_days: Option<u32>) -> AppResult<Vec<AgendaBucket>> {
    let core = state.0.clone();
    let ws = core.workspace();
    let now = Utc::now();
    let today = chrono::Local::now().date_naive();
    let horizon = horizon_days.unwrap_or(7) as i64;
    let limit_date = today + Duration::days(horizon);

    let mut buckets: Vec<AgendaBucket> = crate::domain::task::BUCKETS
        .iter()
        .map(|(key, label)| AgendaBucket {
            key: (*key).to_string(),
            label: (*label).to_string(),
            tasks: Vec::new(),
        })
        .collect();

    for s in ws.list()? {
        let Ok(topic) = ws.load(&s.slug) else { continue };
        for task in PlanTask::load_all(&topic.tasks_path()) {
            // 已完成/归档的只保留最近 3 天的，避免日程被历史淹没
            if matches!(task.status, TaskStatus::Done | TaskStatus::Archived) {
                let recent = task
                    .done_at
                    .or(Some(task.updated_at))
                    .map(|d| (today - d.date_naive()).num_days() <= 3)
                    .unwrap_or(false);
                if !recent {
                    continue;
                }
            }
            if let Some(d) = task.due_date() {
                if d > limit_date && task.status.is_open() {
                    // 超出视野的排期任务仍然归入「以后」
                }
            }
            let key = crate::domain::task::bucket_of(&task, today, now);
            if let Some(b) = buckets.iter_mut().find(|b| b.key == key) {
                b.tasks.push(AgendaItem {
                    task,
                    topic_slug: topic.slug(),
                    topic_name: topic.meta.name.clone(),
                });
            }
        }
    }
    buckets.retain(|b| !b.tasks.is_empty());
    Ok(buckets)
}

// ============================================================ 学习会话

#[tauri::command]
pub async fn session_list(state: State<'_, AppState>, slug: String) -> AppResult<Vec<StudySession>> {
    let topic = state.0.workspace().resolve(&slug)?;
    let mut items = Vec::new();
    if let Ok(rd) = std::fs::read_dir(topic.sessions_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(Some(s)) = store::read_json_opt::<StudySession>(&p) {
                    items.push(s);
                }
            }
        }
    }
    items.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(items)
}

#[tauri::command]
pub async fn session_current(state: State<'_, AppState>) -> AppResult<Option<StudySession>> {
    Ok(state.0.current_session())
}

#[tauri::command]
pub async fn session_start(
    state: State<'_, AppState>,
    slug: String,
    title: String,
    stage: Option<String>,
    goals: Option<Vec<String>>,
    chat_id: Option<String>,
) -> AppResult<StudySession> {
    let core = state.0.clone();
    let mut topic = core.workspace().resolve(&slug)?;
    let stage = stage
        .as_deref()
        .and_then(StudyStage::parse)
        .unwrap_or(topic.meta.stage);
    let mut session = StudySession::new(
        topic.meta.id.clone(),
        topic.slug(),
        if title.trim().is_empty() { topic.meta.name.clone() } else { title },
        stage,
    );
    session.goals = goals.unwrap_or_default();
    if let Some(c) = chat_id.filter(|c| !c.trim().is_empty()) {
        session.chat_id = c;
    }
    if topic.meta.stage != stage {
        topic.set_stage(stage)?;
        core.emit_topics_updated(&topic);
    }
    store::ensure_dir(&topic.sessions_dir())?;
    store::write_json(&topic.sessions_dir().join(format!("{}.json", session.id)), &session)?;
    core.set_current_session(Some(session.clone()));
    Ok(session)
}

#[tauri::command]
pub async fn session_finish(
    state: State<'_, AppState>,
    summary: String,
    highlights: Option<Vec<String>>,
    open_questions: Option<Vec<String>>,
) -> AppResult<StudySession> {
    let core = state.0.clone();
    let mut session = core
        .current_session()
        .ok_or_else(|| AppError::invalid("当前没有进行中的学习会话"))?;
    session.summary = summary;
    session.highlights = highlights.unwrap_or_default();
    session.open_questions = open_questions.unwrap_or_default();
    session.ended_at = Some(Utc::now());
    let topic = core.workspace().resolve(&session.topic_slug)?;
    store::write_json(&topic.sessions_dir().join(format!("{}.json", session.id)), &session)?;
    core.set_current_session(None);
    core.emit_topics_updated(&topic);
    Ok(session)
}

/// 「今天该学什么」：把到期卡片、今日任务、待复习主题汇总成一句话级别的清单。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBrief {
    pub date: String,
    pub due_cards: usize,
    pub open_tasks: usize,
    pub overdue_tasks: usize,
    pub topics_touched_today: usize,
    pub heatmap: Vec<HeatCell>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatCell {
    pub date: String,
    pub count: usize,
    pub level: u8,
}

#[tauri::command]
pub async fn daily_brief(state: State<'_, AppState>) -> AppResult<DailyBrief> {
    let core = state.0.clone();
    let ws = core.workspace();
    let now = Utc::now();
    let today = chrono::Local::now().date_naive();

    let mut due_cards = 0usize;
    let mut open_tasks = 0usize;
    let mut overdue = 0usize;
    let mut touched_today = 0usize;
    // 近 30 天的会话活跃度
    let mut activity: std::collections::BTreeMap<chrono::NaiveDate, usize> = Default::default();

    for s in ws.list()? {
        let Ok(topic) = ws.load(&s.slug) else { continue };
        for c in store::read_jsonl::<Card>(&topic.cards_path())? {
            if c.srs.is_due(now) {
                due_cards += 1;
            }
        }
        for t in PlanTask::load_all(&topic.tasks_path()) {
            if t.status.is_open() {
                open_tasks += 1;
                if t.is_overdue(now) {
                    overdue += 1;
                }
            }
        }
        let mut touched = false;
        if let Ok(rd) = std::fs::read_dir(topic.sessions_dir()) {
            for e in rd.flatten() {
                if let Ok(Some(sess)) = store::read_json_opt::<StudySession>(&e.path()) {
                    let d = sess.started_at.with_timezone(&chrono::Local).date_naive();
                    if (today - d).num_days() <= 30 && (today - d).num_days() >= 0 {
                        *activity.entry(d).or_default() += 1;
                    }
                    if d == today {
                        touched = true;
                    }
                }
            }
        }
        if touched {
            touched_today += 1;
        }
    }

    // 补齐最近 30 天的空格子，前端直接画
    let mut heatmap = Vec::with_capacity(30);
    for i in (0..30).rev() {
        let d = today - Duration::days(i);
        let count = activity.get(&d).copied().unwrap_or(0);
        let level = match count {
            0 => 0,
            1 => 1,
            2..=3 => 2,
            _ => 3,
        };
        heatmap.push(HeatCell {
            date: d.format("%Y-%m-%d").to_string(),
            count,
            level,
        });
    }

    Ok(DailyBrief {
        date: format!("{} {} ", today.format("%Y-%m-%d"), weekday_cn(today.weekday())),
        due_cards,
        open_tasks,
        overdue_tasks: overdue,
        topics_touched_today: touched_today,
        heatmap,
    })
}

fn weekday_cn(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    }
}
