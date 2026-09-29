//! 学习类工具：主题、笔记、卡片、任务、学习会话。
//!
//! 这一组是「学习中枢」的学习闭环：agent 通过它们把对话沉淀成可复习、可追踪的资产。

use crate::agent::event::Risk;
use crate::agent::registry::{
    arg_bool, arg_str, arg_str_array, arg_str_req, arg_u32, bool_prop, num_prop, object_schema,
    str_array_prop, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::domain::card::{Card, CardKind, Grade};
use crate::domain::note;
use crate::domain::session::StudySession;
use crate::domain::stage::StudyStage;
use crate::domain::task::{PlanTask, TaskStatus};
use crate::domain::topic::Topic;
use crate::error::{AppError, AppResult};
use crate::paths::{ensure_dir, resolve_in_root};
use crate::store;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::path::PathBuf;

// ============================================================ 主题

pub struct TopicCreate;

#[async_trait]
impl Tool for TopicCreate {
    fn name(&self) -> &'static str {
        "topic_create"
    }

    fn description(&self) -> &'static str {
        "新建一个学习主题（会在工作区里创建一个目录，内含 notes/materials/cards/plan/sessions 骨架）。\
         当用户表达出「想系统学某样东西」时主动提议并创建。\
         如果用户只想学某门课里的**一章**，用 parent 参数把它建成那门课的子主题：\
         子主题会继承父主题的讲义与资料（只读），但笔记、卡片、计划都独立。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "name": str_prop("主题名，会成为目录名，例如「线性代数」「第三章 特征值」"),
                "description": str_prop("一句话说明学它做什么（写进主题简介，agent 每次都看得到）"),
                "emoji": str_prop("可选的表情符号，用于侧栏展示"),
                "parent": str_prop("可选。父主题（主题名或 id）：本主题将是它的一个章节，读得到它的资料"),
            }),
            &["name"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("新建主题「{}」", arg_str(input, "name").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let name = arg_str_req(&input, "name")?;
        let desc = arg_str(&input, "description").unwrap_or_default();
        let emoji = arg_str(&input, "emoji");
        let parent = arg_str(&input, "parent");
        let ws = ctx.core.workspace();
        let topic = ws.create(&name, &desc, emoji, parent.as_deref())?;
        ctx.core.emit_topics_created(&topic);
        let parent_line = ws
            .ancestors(&topic)
            .first()
            .map(|p| format!("它是「{}」的子主题，可以读父主题的讲义与资料（只读）。\n", p.meta.name))
            .unwrap_or_default();
        Ok(ToolOutput::ok(format!(
            "已创建主题「{}」（目录：{}）。\n{parent_line}\n{}\n\n可以用 topic_info 查看结构，或直接开始预习阶段。",
            topic.meta.name,
            topic.meta.name,
            skeleton_hint(&topic)
        )))
    }
}

fn skeleton_hint(t: &Topic) -> String {
    format!(
        "目录结构：\n- notes/ 笔记\n- materials/ 资料（把 PDF 放这里）\n- cards/cards.jsonl 卡片\n- plan/tasks.jsonl 计划\n- sessions/ 学习会话\n\n绝对路径：{}",
        t.dir.display()
    )
}

pub struct TopicList;

#[async_trait]
impl Tool for TopicList {
    fn name(&self) -> &'static str {
        "topic_list"
    }

    fn description(&self) -> &'static str {
        "列出工作区里所有学习主题及其规模（笔记数、待复习卡片数、未完成任务数）。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "列出全部主题".into()
    }

    async fn run(&self, ctx: &ToolCtx, _input: Value) -> AppResult<ToolOutput> {
        let list = ctx.core.workspace().list()?;
        if list.is_empty() {
            return Ok(ToolOutput::ok("工作区里还没有主题。可以用 topic_create 新建一个。"));
        }
        let mut out = format!("共 {} 个主题：\n\n", list.len());
        for s in &list {
            out.push_str(&format!(
                "- {}｜笔记 {}｜卡片 {}(待复习 {})｜未完成任务 {}｜阶段 {}\n",
                s.meta.name,
                s.stats.notes,
                s.stats.cards,
                s.stats.cards_due,
                s.stats.tasks_open,
                s.meta.stage.label()
            ));
            if !s.meta.description.trim().is_empty() {
                out.push_str(&format!("  └ {}\n", s.meta.description.trim()));
            }
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct TopicInfo;

#[async_trait]
impl Tool for TopicInfo {
    fn name(&self) -> &'static str {
        "topic_info"
    }

    fn description(&self) -> &'static str {
        "查看某个主题的详细信息：简介、阶段、目录结构、笔记清单、资料清单、计划与卡片统计。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({ "topic": str_prop("主题名或目录名，默认当前主题") }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "查看主题 {}",
            arg_str(input, "topic").unwrap_or_else(|| "（当前）".into())
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        let stats = topic.stats()?;
        let mut out = format!(
            "主题：{}\n目录：{}\n阶段：{}\n简介：{}\n标签：{}\n创建于：{}\n\n统计：笔记 {}｜资料 {}｜卡片 {}（待复习 {}）｜未完成任务 {}｜会话 {}\n",
            topic.meta.name,
            topic.dir.display(),
            topic.meta.stage.label(),
            if topic.meta.description.trim().is_empty() { "（未填写）" } else { topic.meta.description.as_str() },
            if topic.meta.tags.is_empty() { "（无）".into() } else { topic.meta.tags.join("、") },
            topic.meta.created_at.format("%Y-%m-%d %H:%M"),
            stats.notes,
            stats.materials,
            stats.cards,
            stats.cards_due,
            stats.tasks_open,
            stats.sessions,
        );

        let files = super::fs::list_topic_files(&topic.dir, 4);
        if !files.is_empty() {
            out.push_str("\n文件清单：\n");
            for f in files.iter().take(80) {
                out.push_str(&format!("- {f}\n"));
            }
            if files.len() > 80 {
                out.push_str(&format!("…共 {} 个文件\n", files.len()));
            }
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct TopicSetStage;

#[async_trait]
impl Tool for TopicSetStage {
    fn name(&self) -> &'static str {
        "topic_set_stage"
    }

    fn description(&self) -> &'static str {
        "切换主题的学习阶段（preview 预习 / learn 学习 / review 复习 / test 测验）。\
         阶段会改变你的回答方式，用户在界面上也能直接切。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "stage": { "type": "string", "enum": ["preview", "learn", "review", "test"] },
                "topic": str_prop("主题名，默认当前主题"),
            }),
            &["stage"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("切换学习阶段到 {}", arg_str(input, "stage").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let raw = arg_str_req(&input, "stage")?;
        let stage = StudyStage::parse(&raw)
            .ok_or_else(|| AppError::invalid(format!("未知阶段：{raw}（可用 preview/learn/review/test）")))?;
        let mut topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        topic.set_stage(stage)?;
        ctx.core.emit_topics_updated(&topic);
        Ok(ToolOutput::ok(format!(
            "已把「{}」的阶段切换为【{}】。接下来我会按这个阶段的方式带你。",
            topic.meta.name,
            stage.label()
        )))
    }
}

// ============================================================ 笔记

pub struct NoteCreate;

#[async_trait]
impl Tool for NoteCreate {
    fn name(&self) -> &'static str {
        "note_create"
    }

    fn description(&self) -> &'static str {
        "在 notes/ 下写一篇结构化笔记（自动加 front matter 标题与标签）。\
         文件已存在时会在内容末尾追加一个带时间戳的小节，不会覆盖用户已有的文字。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("笔记标题，会成为文件名，例如「特征值与特征向量」"),
                "content": str_prop("Markdown 正文（不要重复写一级标题）"),
                "tags": str_array_prop("标签，例如 [\"线代\", \"矩阵\"]"),
                "dir": str_prop("放在 notes 下的子目录，例如「第三章」，默认直接放 notes/"),
                "topic": str_prop("写进哪个主题的笔记，默认当前主题"),
            }),
            &["title", "content"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("写笔记「{}」", arg_str(input, "title").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        let title = arg_str_req(&input, "title")?;
        let content = arg_str(&input, "content").unwrap_or_default();
        let tags = arg_str_array(&input, "tags");
        let sub = arg_str(&input, "dir").unwrap_or_default();

        let dir = if sub.trim().is_empty() {
            topic.notes_dir()
        } else {
            let sub = sub.trim().trim_matches('/');
            ensure_dir(&topic.notes_dir())?;
            resolve_in_root(&topic.notes_dir(), sub)?
        };
        ensure_dir(&dir)?;

        let stem = crate::paths::sanitize_dir_name(&title);
        let path = dir.join(format!("{stem}.md"));
        let rel = topic.rel(&path);

        let existed = path.exists();
        let mut body = String::new();
        if !existed {
            body.push_str("---\n");
            body.push_str(&format!("title: {title}\n"));
            if !tags.is_empty() {
                body.push_str(&format!("tags: {}\n", tags.join(", ")));
            }
            body.push_str(&format!("created: {}\n", Utc::now().format("%Y-%m-%d %H:%M")));
            body.push_str("---\n\n");
            body.push_str(&format!("# {title}\n\n"));
            body.push_str(content.trim());
            body.push('\n');
            store::atomic_write(&path, body.as_bytes())?;
        } else {
            let mut existing = store::read_text(&path)?;
            if !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(&format!(
                "\n## 补充（{}）\n\n{}\n",
                Utc::now().format("%Y-%m-%d %H:%M"),
                content.trim()
            ));
            store::atomic_write(&path, existing.as_bytes())?;
        }

        ctx.core.bump_session_counters(1, 0);
        const PREVIEW: usize = 400;
        Ok(ToolOutput::ok(format!(
            "已{}笔记：{rel}\n\n内容预览：\n{}",
            if existed { "追加到" } else { "创建" },
            content.chars().take(PREVIEW).collect::<String>()
        )))
    }
}

pub struct NoteList;

#[async_trait]
impl Tool for NoteList {
    fn name(&self) -> &'static str {
        "note_list"
    }

    fn description(&self) -> &'static str {
        "列出主题里的笔记（标题、标签、修改时间、摘要），可按关键词过滤。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("可选：按标题/标签/正文过滤"),
                "limit": num_prop("最多返回多少篇，默认 50"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        match arg_str(input, "query") {
            Some(q) if !q.trim().is_empty() => format!("列出笔记（筛选「{q}」）"),
            _ => "列出全部笔记".into(),
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let query = arg_str(&input, "query").unwrap_or_default().to_lowercase();
        let limit = arg_u32(&input, "limit").unwrap_or(50).min(200) as usize;

        let mut items: Vec<note::NoteSummary> = Vec::new();
        for f in store::walk_files(&topic.notes_dir(), 4) {
            if f.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let Ok(text) = store::read_text_capped(&f, 256 * 1024) else { continue };
            let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            items.push(note::NoteSummary::from_file(topic.rel(&f), &text, size, store::modified_at(&f)));
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        if !query.is_empty() {
            items.retain(|n| {
                n.title.to_lowercase().contains(&query)
                    || n.tags.iter().any(|t| t.to_lowercase().contains(&query))
                    || n.excerpt.to_lowercase().contains(&query)
            });
        }

        if items.is_empty() {
            return Ok(ToolOutput::ok("还没有笔记。"));
        }
        let total = items.len();
        let mut out = format!("共 {total} 篇笔记" );
        if !query.is_empty() {
            out.push_str(&format!("（筛选「{query}」）"));
        }
        out.push_str("：\n\n");
        for n in items.iter().take(limit) {
            out.push_str(&format!(
                "- {}｜{}｜{}\n  {}\n",
                n.title,
                n.path,
                n.updated_at.format("%Y-%m-%d"),
                n.excerpt
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

// ============================================================ 卡片

pub struct CardCreate;

#[async_trait]
impl Tool for CardCreate {
    fn name(&self) -> &'static str {
        "card_create"
    }

    fn description(&self) -> &'static str {
        "存记忆卡片，进入间隔重复队列（也可以同步到 Anki）。三种类型：\n\
         - basic（默认）：正问反答，front 问 / back 答\n\
         - reversed：正反都能问，Anki 里会生成两张卡\n\
         - cloze：完形填空，front 里用 {{c1::要挖空的内容}} 标出空格，可写多个 {{c2::}}、{{c3::}}\n\
         只做「值得长期记住的事实、定义、公式、易错点」；一张卡只考一个点，正面要能独立看懂。\n\
         一次可以连续调用（或直接用 cards 数组一次存多张），不要积攒到最后。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "front": str_prop("正面：一个明确的提问或提示；完形卡则是带 {{c1::...}} 标记的整段文字"),
                "back": str_prop("背面：简洁准确的答案（完形卡可留空）"),
                "kind": {
                    "type": "string",
                    "enum": ["basic", "reversed", "cloze"],
                    "description": "卡片类型，默认 basic"
                },
                "tags": str_array_prop("标签"),
                "module": str_prop("所属章节/子话题，便于按模块复习"),
                "source": str_prop("出处，例如 notes/特征值.md 或 materials/ch1.pdf 第 12 页"),
                "cards": {
                    "type": "array",
                    "description": "批量模式：一次存多张。每项是 {front, back, kind?, tags?, module?, source?}",
                    "items": { "type": "object" }
                },
            }),
            &[],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        if let Some(arr) = input.get("cards").and_then(|c| c.as_array()) {
            return format!("批量存 {} 张卡片", arr.len());
        }
        let front = arg_str(input, "front").unwrap_or_default();
        let kind = arg_str(input, "kind").unwrap_or_else(|| "basic".into());
        let label = match kind.as_str() {
            "cloze" => "完形卡",
            "reversed" => "反向卡",
            _ => "卡片",
        };
        format!("存{label}「{}」", front.chars().take(36).collect::<String>())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
        let existing: std::collections::HashSet<String> =
            cards.iter().map(|c| c.fingerprint()).collect();

        // 支持批量：cards 数组每一项当作一次独立的录入
        let mut specs: Vec<Value> = Vec::new();
        if let Some(arr) = input.get("cards").and_then(|c| c.as_array()) {
            specs.extend(arr.iter().cloned());
        }
        if input.get("front").is_some() {
            specs.push(input.clone());
        }
        if specs.is_empty() {
            return Err(AppError::invalid("要么给 front/back，要么给 cards 数组"));
        }

        let mut added: Vec<Card> = Vec::new();
        let mut dup = 0usize;
        let mut invalid: Vec<String> = Vec::new();

        for spec in &specs {
            let kind = arg_str(spec, "kind")
                .as_deref()
                .and_then(CardKind::parse)
                .unwrap_or(CardKind::Basic);
            let front = arg_str_req(spec, "front")?;
            let back = arg_str(spec, "back").unwrap_or_default();
            let mut card = match kind {
                CardKind::Cloze => Card::new_cloze(front),
                _ => {
                    let mut c = Card::new(front, back);
                    c.kind = kind;
                    c
                }
            };
            card.tags = arg_str_array(spec, "tags");
            card.module = arg_str(spec, "module");
            card.source = arg_str(spec, "source");

            if let Err(e) = card.validate() {
                invalid.push(format!("{}：{e}", card.front.chars().take(30).collect::<String>()));
                continue;
            }
            if existing.contains(&card.fingerprint()) {
                dup += 1;
                continue;
            }
            added.push(card);
        }

        if !added.is_empty() {
            cards.extend(added.iter().cloned());
            store::write_jsonl(&topic.cards_path(), &cards)?;
            ctx.core.bump_session_counters(0, added.len() as u32);
            ctx.core.emit_topics_updated(&topic);
        }

        let now = Utc::now();
        let mut out = if added.len() == 1 {
            format!(
                "已存卡片（id {}，类型 {}）。",
                added[0].id,
                added[0].kind.label()
            )
        } else {
            format!("已存 {} 张卡片（{}）。", added.len(), kinds_summary(&added))
        };
        if dup > 0 {
            out.push_str(&format!("跳过 {dup} 张重复的。"));
        }
        if !invalid.is_empty() {
            out.push_str(&format!("\n有问题没能存：{}", invalid.join("；")));
        }
        out.push_str(&format!(
            "\n该主题现有 {} 张卡片，待复习 {} 张。",
            cards.len(),
            cards.iter().filter(|c| c.srs.is_due(now)).count()
        ));
        Ok(ToolOutput::ok(out))
    }
}

fn kinds_summary(cards: &[Card]) -> String {
    let mut basic = 0;
    let mut cloze = 0;
    let mut reversed = 0;
    for c in cards {
        match c.kind {
            CardKind::Basic => basic += 1,
            CardKind::Cloze => cloze += 1,
            CardKind::Reversed => reversed += 1,
        }
    }
    let mut parts = Vec::new();
    if basic > 0 {
        parts.push(format!("基础 {basic}"));
    }
    if cloze > 0 {
        parts.push(format!("完形 {cloze}"));
    }
    if reversed > 0 {
        parts.push(format!("反向 {reversed}"));
    }
    parts.join("、")
}

pub struct CardList;

#[async_trait]
impl Tool for CardList {
    fn name(&self) -> &'static str {
        "card_list"
    }

    fn description(&self) -> &'static str {
        "列出卡片，可按关键词或「只看到期的」过滤。复习前用它了解队列规模。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("按正面/背面/标签过滤"),
                "due_only": bool_prop("只看今天到期的"),
                "limit": num_prop("默认 50"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        if arg_bool(input, "due_only").unwrap_or(false) {
            "列出到期卡片".into()
        } else {
            "列出卡片".into()
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
        let now = Utc::now();
        if arg_bool(&input, "due_only").unwrap_or(false) {
            cards.retain(|c| c.srs.is_due(now));
        }
        if let Some(q) = arg_str(&input, "query") {
            let q = q.to_lowercase();
            cards.retain(|c| {
                c.front.to_lowercase().contains(&q)
                    || c.back.to_lowercase().contains(&q)
                    || c.tags.iter().any(|t| t.to_lowercase().contains(&q))
            });
        }
        if cards.is_empty() {
            return Ok(ToolOutput::ok("没有符合条件的卡片。"));
        }
        let limit = arg_u32(&input, "limit").unwrap_or(50).min(300) as usize;
        let mut out = format!(
            "共 {} 张卡片（待复习 {} 张）：\n\n",
            cards.len(),
            cards.iter().filter(|c| c.srs.is_due(now)).count()
        );
        for c in cards.iter().take(limit) {
            let state = if c.srs.is_new() {
                "新卡".to_string()
            } else {
                format!(
                    "间隔 {:.1} 天/难度 {:.2}{}",
                    c.srs.interval_days,
                    c.srs.ease,
                    if c.srs.is_due(now) { "·已到期" } else { "" }
                )
            };
            out.push_str(&format!("- [{}] {}｜{state}\n  → {}\n", c.id, c.front, c.back));
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct CardReview;

#[async_trait]
impl Tool for CardReview {
    fn name(&self) -> &'static str {
        "card_review"
    }

    fn description(&self) -> &'static str {
        "记录一次复习结果并推进间隔重复调度。grade：again 忘了 / hard 很吃力 / good 正常 / easy 太简单。\
         用户答完后立刻调用，不要积攒。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "card_id": str_prop("card_list 里给出的卡片 id"),
                "grade": { "type": "string", "enum": ["again", "hard", "good", "easy"] },
            }),
            &["card_id", "grade"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "记录复习结果 {} → {}",
            arg_str(input, "card_id").unwrap_or_default(),
            arg_str(input, "grade").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let id = arg_str_req(&input, "card_id")?;
        let grade_raw = arg_str_req(&input, "grade")?;
        let grade = Grade::parse(&grade_raw)
            .ok_or_else(|| AppError::invalid(format!("未知评分：{grade_raw}")))?;

        let mut cards = store::read_jsonl::<Card>(&topic.cards_path())?;
        let Some(card) = cards.iter_mut().find(|c| c.id == id) else {
            return Err(AppError::NotFound(format!("找不到卡片 {id}")));
        };
        card.srs.apply(grade, Utc::now());
        let due = card.srs.due;
        let interval = card.srs.interval_days;
        store::write_jsonl(&topic.cards_path(), &cards)?;
        Ok(ToolOutput::ok(format!(
            "已记录。这张卡下次到期：{}（间隔 {:.1} 天）。",
            due.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
            interval
        )))
    }
}

// ============================================================ 任务 / 日程

pub struct TaskCreate;

#[async_trait]
impl Tool for TaskCreate {
    fn name(&self) -> &'static str {
        "task_create"
    }

    fn description(&self) -> &'static str {
        "新建一条学习计划任务（今日复习、读完某章、做完习题…）。\
         知道截止时间就填 due，能排进日程视图。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("任务标题，动词开头，例如「读完第 3 章并做完习题 3.1-3.5」"),
                "detail": str_prop("补充说明"),
                "due": str_prop("截止时间，ISO 格式如 2026-10-01 或 2026-10-01T20:00:00"),
                "priority": num_prop("1 低 / 2 中 / 3 高，默认 2"),
                "estimate_min": num_prop("预计耗时（分钟）"),
                "stage": { "type": "string", "enum": ["preview", "learn", "review", "test"], "description": "属于哪个学习阶段" },
                "module": str_prop("所属章节/子话题"),
            }),
            &["title"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("新建任务「{}」", arg_str(input, "title").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let title = arg_str_req(&input, "title")?;
        let mut task = PlanTask::new(title);
        task.detail = arg_str(&input, "detail").unwrap_or_default();
        task.priority = arg_u32(&input, "priority").unwrap_or(2).clamp(1, 3) as u8;
        task.estimate_min = arg_u32(&input, "estimate_min");
        task.module = arg_str(&input, "module");
        task.stage = arg_str(&input, "stage").as_deref().and_then(StudyStage::parse);
        task.due = arg_str(&input, "due").as_deref().and_then(parse_due);

        store::append_jsonl(&topic.tasks_path(), &task)?;
        ctx.core.emit_topics_updated(&topic);
        Ok(ToolOutput::ok(format!(
            "已加入计划：{}（id {}）{}",
            task.title,
            task.id,
            task.due
                .map(|d| format!("，截止 {}", d.with_timezone(&chrono::Local).format("%m-%d %H:%M")))
                .unwrap_or_default()
        )))
    }
}

/// 宽松解析日期：支持「2026-10-01」「2026-10-01 20:00」「10-01」「明天」这类常见写法。
pub fn parse_due(raw: &str) -> Option<chrono::DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let now = chrono::Local::now();
    let end_of_day = |d: chrono::NaiveDate| d.and_hms_opt(23, 59, 0).map(|n| n.and_utc());

    match s {
        "今天" | "today" => return end_of_day(now.date_naive()),
        "明天" | "tomorrow" => return end_of_day((now + Duration::days(1)).date_naive()),
        "后天" => return end_of_day((now + Duration::days(2)).date_naive()),
        "下周" | "下个星期" => return end_of_day((now + Duration::days(7)).date_naive()),
        _ => {}
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y/%m/%d %H:%M", "%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc());
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return end_of_day(date);
        }
    }
    // 「10-01」「10/01」这种缺年份的写法，按今年算
    let parts: Vec<&str> = s.split(['-', '/', '.']).collect();
    if parts.len() == 2 {
        if let (Ok(m), Ok(d)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            if let Some(date) = chrono::NaiveDate::from_ymd_opt(now.date_naive().year(), m, d) {
                return end_of_day(date);
            }
        }
    }
    None
}

use chrono::Datelike as _;

pub struct TaskList;

#[async_trait]
impl Tool for TaskList {
    fn name(&self) -> &'static str {
        "task_list"
    }

    fn description(&self) -> &'static str {
        "列出计划任务，默认只看未完成的。可用来回答「我今天该做什么」。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "status": { "type": "string", "enum": ["open", "todo", "doing", "done", "archived", "all"], "description": "默认 open" },
                "topic": str_prop("主题名，默认当前主题"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("列出任务（{}）", arg_str(input, "status").unwrap_or_else(|| "open".into()))
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let status = arg_str(&input, "status").unwrap_or_else(|| "open".into());
        // 不指定主题时，汇总所有主题的任务（回答「今天干什么」更有用）
        let topics: Vec<Topic> = match arg_str(&input, "topic") {
            Some(t) if !t.trim().is_empty() => vec![ctx.topic_or(Some(&t))?],
            _ => {
                let ws = ctx.core.workspace();
                ws.list()?
                    .iter()
                    .filter_map(|s| ws.load(&s.slug).ok())
                    .collect()
            }
        };

        let now = Utc::now();
        let today = chrono::Local::now().date_naive();
        let mut buckets: std::collections::BTreeMap<&str, Vec<(String, PlanTask)>> = Default::default();
        let mut total = 0;
        for t in &topics {
            for task in PlanTask::load_all(&t.tasks_path()) {
                let keep = match status.as_str() {
                    "all" => true,
                    "done" => task.status == TaskStatus::Done,
                    "archived" => task.status == TaskStatus::Archived,
                    "todo" => task.status == TaskStatus::Todo,
                    "doing" => task.status == TaskStatus::Doing,
                    _ => task.status.is_open(),
                };
                if !keep {
                    continue;
                }
                total += 1;
                let key = crate::domain::task::bucket_of(&task, today, now);
                buckets.entry(key).or_default().push((t.meta.name.clone(), task));
            }
        }

        if total == 0 {
            return Ok(ToolOutput::ok("没有符合条件的任务。"));
        }
        let mut out = format!("共 {total} 条任务：\n");
        for (key, label) in crate::domain::task::BUCKETS {
            let Some(items) = buckets.get(key) else { continue };
            out.push_str(&format!("\n【{label}】\n"));
            for (topic_name, task) in items {
                let due = task
                    .due
                    .map(|d| format!("｜{}", d.with_timezone(&chrono::Local).format("%m-%d %H:%M")))
                    .unwrap_or_default();
                let pri = match task.priority {
                    3 => "高",
                    1 => "低",
                    _ => "中",
                };
                out.push_str(&format!(
                    "- [{}]{due} {}（{}｜{}）\n  id={}\n",
                    pri,
                    task.title,
                    topic_name,
                    task.status.label(),
                    task.id
                ));
                if !task.detail.trim().is_empty() {
                    out.push_str(&format!("  {}\n", task.detail.trim()));
                }
            }
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct TaskUpdate;

#[async_trait]
impl Tool for TaskUpdate {
    fn name(&self) -> &'static str {
        "task_update"
    }

    fn description(&self) -> &'static str {
        "更新任务状态或内容（标记完成、改截止时间、调优先级）。只传要改的字段。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "id": str_prop("任务 id"),
                "status": { "type": "string", "enum": ["todo", "doing", "done", "archived"] },
                "due": str_prop("新的截止时间"),
                "priority": num_prop("1/2/3"),
                "title": str_prop("新标题"),
                "detail": str_prop("新的补充说明"),
            }),
            &["id"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "更新任务 {}{}",
            arg_str(input, "id").unwrap_or_default(),
            arg_str(input, "status")
                .map(|s| format!(" → {s}"))
                .unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let id = arg_str_req(&input, "id")?;
        let mut tasks = PlanTask::load_all(&topic.tasks_path());
        let Some(task) = tasks.iter_mut().find(|t| t.id == id) else {
            return Err(AppError::NotFound(format!("找不到任务 {id}")));
        };

        if let Some(s) = arg_str(&input, "status") {
            let st = TaskStatus::parse(&s).ok_or_else(|| AppError::invalid(format!("未知状态：{s}")))?;
            task.status = st;
            task.done_at = if st == TaskStatus::Done { Some(Utc::now()) } else { None };
        }
        if let Some(d) = arg_str(&input, "due") {
            task.due = parse_due(&d);
        }
        if let Some(p) = arg_u32(&input, "priority") {
            task.priority = p.clamp(1, 3) as u8;
        }
        if let Some(t) = arg_str(&input, "title") {
            if !t.trim().is_empty() {
                task.title = t;
            }
        }
        if let Some(d) = arg_str(&input, "detail") {
            task.detail = d;
        }
        task.updated_at = Utc::now();
        let title = task.title.clone();
        let status = task.status;

        store::write_jsonl(&topic.tasks_path(), &tasks)?;
        ctx.core.emit_topics_updated(&topic);
        Ok(ToolOutput::ok(format!("已更新任务「{title}」：{}", status.label())))
    }
}

// ============================================================ 学习会话

pub struct SessionStart;

#[async_trait]
impl Tool for SessionStart {
    fn name(&self) -> &'static str {
        "session_start"
    }

    fn description(&self) -> &'static str {
        "开始一次学习会话，并设定目标与阶段。会话会记录这段时间学了什么，结束时可以总结成笔记与卡片。\
         用户说「我们开始学 X」时调用。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("本次会话的主题，例如「特征值：从几何直觉到计算」"),
                "stage": { "type": "string", "enum": ["preview", "learn", "review", "test"], "description": "默认沿用主题当前阶段" },
                "goals": str_array_prop("本次要达成的目标，2~4 条"),
                "materials": str_array_prop("本次会用到的资料相对路径"),
            }),
            &["title"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("开始学习会话「{}」", arg_str(input, "title").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let mut topic = ctx.topic()?.clone();
        let title = arg_str_req(&input, "title")?;
        let stage = arg_str(&input, "stage")
            .as_deref()
            .and_then(StudyStage::parse)
            .unwrap_or(topic.meta.stage);

        let mut session = StudySession::new(topic.meta.id.clone(), topic.slug(), title.clone(), stage);
        session.goals = arg_str_array(&input, "goals");
        session.materials = arg_str_array(&input, "materials");
        session.chat_id = ctx.chat_id.clone();

        // 会话阶段与主题阶段同步，让后续提示词一致
        if topic.meta.stage != stage {
            topic.set_stage(stage)?;
            ctx.core.emit_topics_updated(&topic);
        }

        let dir = topic.sessions_dir();
        ensure_dir(&dir)?;
        let path: PathBuf = dir.join(format!("{}.json", session.id));
        store::write_json(&path, &session)?;
        ctx.core.set_current_session(Some(session.clone()));

        Ok(ToolOutput::ok(format!(
            "已开始【{}】会话：{title}\n目标：{}\n\n接下来按这个阶段的节奏带用户推进。",
            stage.label(),
            if session.goals.is_empty() { "（未设定）".to_string() } else { session.goals.join("；") }
        )))
    }
}

pub struct SessionFinish;

#[async_trait]
impl Tool for SessionFinish {
    fn name(&self) -> &'static str {
        "session_finish"
    }

    fn description(&self) -> &'static str {
        "结束当前学习会话并落库：写总结、要点与遗留问题。结束后 agent 应再调用 note_create 把总结写成笔记。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "summary": str_prop("3~6 句话的总结：这次搞懂了什么、卡在哪里"),
                "highlights": str_array_prop("本次的关键结论/要点"),
                "open_questions": str_array_prop("还没解决的问题，下次接着来"),
            }),
            &["summary"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, _input: &Value) -> String {
        "结束当前学习会话".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let Some(mut session) = ctx.core.current_session() else {
            return Err(AppError::invalid("当前没有进行中的学习会话"));
        };
        session.summary = arg_str(&input, "summary").unwrap_or_default();
        session.highlights = arg_str_array(&input, "highlights");
        session.open_questions = arg_str_array(&input, "open_questions");
        session.ended_at = Some(Utc::now());

        let topic = ctx.core.workspace().resolve(&session.topic_slug)?;
        let path = topic.sessions_dir().join(format!("{}.json", session.id));
        store::write_json(&path, &session)?;
        ctx.core.set_current_session(None);

        Ok(ToolOutput::ok(format!(
            "会话已保存（用时 {} 分钟，产出 {} 张卡片、{} 篇笔记）。\n建议接着用 note_create 把这份总结写成 notes/ 下的笔记，方便以后检索。",
            session.duration_minutes(),
            session.cards_created,
            session.notes_created
        )))
    }
}

pub struct SessionList;

#[async_trait]
impl Tool for SessionList {
    fn name(&self) -> &'static str {
        "session_list"
    }

    fn description(&self) -> &'static str {
        "列出主题的学习会话历史。回答「上次学到哪了」「这个月学了多久」时用。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({ "limit": num_prop("默认 20") }), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "列出学习会话".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let mut sessions: Vec<StudySession> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(topic.sessions_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "json") {
                    if let Ok(Some(s)) = store::read_json_opt::<StudySession>(&p) {
                        sessions.push(s);
                    }
                }
            }
        }
        if sessions.is_empty() {
            return Ok(ToolOutput::ok("还没有学习会话记录。"));
        }
        sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        let limit = arg_u32(&input, "limit").unwrap_or(20) as usize;
        let total_min: i64 = sessions.iter().map(|s| s.duration_minutes()).sum();
        let mut out = format!(
            "共 {} 次会话，累计 {} 小时 {} 分钟：\n\n",
            sessions.len(),
            total_min / 60,
            total_min % 60
        );
        for s in sessions.iter().take(limit) {
            out.push_str(&format!(
                "- {}｜{}｜{} 分钟｜卡片 {}\n  {}\n",
                s.started_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
                s.stage.label(),
                s.duration_minutes(),
                s.cards_created,
                if s.title.is_empty() { "（未命名）" } else { &s.title }
            ));
            if !s.summary.trim().is_empty() {
                out.push_str(&format!("  总结：{}\n", s.summary.trim()));
            }
            if !s.open_questions.is_empty() {
                out.push_str(&format!("  遗留问题：{}\n", s.open_questions.join("；")));
            }
        }
        Ok(ToolOutput::ok(out))
    }
}
