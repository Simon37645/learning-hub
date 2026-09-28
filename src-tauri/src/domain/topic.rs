//! 主题（Topic）= 工作区里的一个目录。这是整个应用的组织单位。
//!
//! 目录结构（对用户完全开放，可以直接用编辑器/vscode 改）：
//!
//! ```text
//! <工作区根>/
//! └── 线性代数/                 ← 主题名就是目录名
//!     ├── topic.json           ← 主题元数据（阶段、标签、简介）
//!     ├── README.md            ← 主题说明，agent 会把它当背景资料
//!     ├── notes/               ← 学习笔记（Markdown）
//!     ├── materials/           ← 资料：PDF / 网页存档 / 图片 / 数据
//!     ├── cards/cards.jsonl    ← Anki 卡片 + 间隔重复状态
//!     ├── plan/tasks.jsonl     ← 日程与任务
//!     ├── sessions/            ← 学习会话（预习/学习/复习/测验）
//!     └── .hub/                ← 应用内部状态：对话记录、图表缓存
//!         └── chats/<id>.jsonl
//! ```

use crate::domain::card::Card;
use crate::domain::stage::StudyStage;
use crate::domain::task::{PlanTask, TaskStatus};
use crate::error::{AppError, AppResult, IoContext};
use crate::paths::{ensure_dir, rel_in_root, sanitize_dir_name, unique_child};
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const TOPIC_FILE: &str = "topic.json";
pub const DIR_NOTES: &str = "notes";
pub const DIR_MATERIALS: &str = "materials";
pub const DIR_CARDS: &str = "cards";
pub const DIR_PLAN: &str = "plan";
pub const DIR_SESSIONS: &str = "sessions";
pub const DIR_QUIZZES: &str = "quizzes";
/// 讲解方案与演示页（讲解模式用）
pub const DIR_LESSONS: &str = "lessons";
/// 用户自己丢进来的讲义（与 agent 生成的笔记分开，方便一眼看出「老师划的重点」）
pub const DIR_KB: &str = "kb";
pub const DIR_INTERNAL: &str = ".hub";
pub const CARDS_REL: &str = "cards/cards.jsonl";
pub const TASKS_REL: &str = "plan/tasks.jsonl";

/// 主题级的工具开关。
///
/// 设计取舍：技能与 MCP 服务器的**定义**可以放在全局（配置里），
/// 但「在这个主题里要不要用它」应当能单独控制——
/// 比如文献检索的 MCP 只在写论文的主题里开，别的时候不占上下文。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicTools {
    /// 本主题里关掉的技能（按技能 id）
    #[serde(default)]
    pub disabled_skills: Vec<String>,
    /// 本主题里关掉的 MCP 服务器（按服务器名）
    #[serde(default)]
    pub disabled_mcp: Vec<String>,
    /// 只属于本主题的 MCP 服务器
    #[serde(default)]
    pub extra_mcp: Vec<crate::mcp::McpServerConfig>,
}

impl TopicTools {
    pub fn skill_enabled(&self, id: &str) -> bool {
        !self.disabled_skills.iter().any(|d| d == id)
    }
    pub fn mcp_enabled(&self, name: &str) -> bool {
        !self.disabled_mcp.iter().any(|d| d == name)
    }
    pub fn set_skill(&mut self, id: &str, enabled: bool) {
        self.disabled_skills.retain(|d| d != id);
        if !enabled {
            self.disabled_skills.push(id.to_string());
        }
    }
    pub fn set_mcp(&mut self, name: &str, enabled: bool) {
        self.disabled_mcp.retain(|d| d != name);
        if !enabled {
            self.disabled_mcp.push(name.to_string());
        }
    }
}

/// topic.json 里的内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicMeta {
    /// 稳定 id：目录改名也不会断掉引用。
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stage: StudyStage,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_opened_at: Option<DateTime<Utc>>,
    /// 主题级的技能 / MCP 开关
    #[serde(default)]
    pub tools: TopicTools,
}

impl TopicMeta {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let now = store::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            emoji: None,
            description: description.into(),
            tags: Vec::new(),
            stage: StudyStage::default(),
            created_at: now,
            updated_at: now,
            last_opened_at: None,
            tools: TopicTools::default(),
        }
    }
}

/// 主题的统计信息（侧栏/工作台展示用，按需扫描计算）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicStats {
    pub notes: usize,
    pub materials: usize,
    pub cards: usize,
    pub cards_due: usize,
    pub tasks_open: usize,
    pub tasks_done: usize,
    pub sessions: usize,
    /// 待复习的最近到期时间（用于排序提醒）
    pub next_due: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicSummary {
    pub meta: TopicMeta,
    /// 相对工作区根的目录名
    pub slug: String,
    /// 绝对路径（前端只用于展示，不做文件访问）
    pub path: String,
    pub stats: TopicStats,
}

/// 一个已打开的主题：元数据 + 根目录。
#[derive(Debug, Clone)]
pub struct Topic {
    pub meta: TopicMeta,
    pub dir: PathBuf,
}

impl Topic {
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(TOPIC_FILE)
    }
    pub fn notes_dir(&self) -> PathBuf {
        self.dir.join(DIR_NOTES)
    }
    pub fn materials_dir(&self) -> PathBuf {
        self.dir.join(DIR_MATERIALS)
    }
    pub fn cards_path(&self) -> PathBuf {
        self.dir.join(CARDS_REL)
    }
    pub fn tasks_path(&self) -> PathBuf {
        self.dir.join(TASKS_REL)
    }
    pub fn sessions_dir(&self) -> PathBuf {
        self.dir.join(DIR_SESSIONS)
    }
    /// 测验试卷与作答记录
    pub fn quizzes_dir(&self) -> PathBuf {
        self.dir.join(DIR_QUIZZES)
    }
    /// 知识库资料目录（讲义、课件）
    pub fn kb_dir(&self) -> PathBuf {
        self.dir.join(DIR_KB)
    }
    /// 讲解方案目录
    pub fn lessons_dir(&self) -> PathBuf {
        self.dir.join(DIR_LESSONS)
    }
    pub fn internal_dir(&self) -> PathBuf {
        self.dir.join(DIR_INTERNAL)
    }
    /// 对话记录目录（每轮对话一个 jsonl）。
    pub fn chats_dir(&self) -> PathBuf {
        self.internal_dir().join("chats")
    }
    pub fn chat_path(&self, chat_id: &str) -> PathBuf {
        self.chats_dir().join(format!("{chat_id}.jsonl"))
    }

    pub fn save_meta(&mut self) -> AppResult<()> {
        self.meta.updated_at = store::now();
        store::write_json(&self.meta_path(), &self.meta)
    }

    pub fn set_stage(&mut self, stage: StudyStage) -> AppResult<()> {
        self.meta.stage = stage;
        self.save_meta()
    }

    pub fn rel(&self, path: &Path) -> String {
        rel_in_root(&self.dir, path)
    }

    /// 目录名（工作区内的唯一标识）。
    pub fn slug(&self) -> String {
        self.dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.meta.name.clone())
    }

    pub fn stats(&self) -> AppResult<TopicStats> {
        let notes = store::walk_files(&self.notes_dir(), 4).len();
        let materials = store::walk_files(&self.materials_dir(), 2).len();

        let cards: Vec<Card> = store::read_jsonl(&self.cards_path())?;
        let now = store::now();
        let due: Vec<&Card> = cards.iter().filter(|c| c.srs.due <= now).collect();

        let tasks: Vec<PlanTask> = store::read_jsonl(&self.tasks_path())?;
        let tasks_open = tasks.iter().filter(|t| t.status.is_open()).count();
        let tasks_done = tasks.iter().filter(|t| t.status == TaskStatus::Done).count();

        let sessions = std::fs::read_dir(self.sessions_dir())
            .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).count())
            .unwrap_or(0);

        Ok(TopicStats {
            notes,
            materials,
            cards: cards.len(),
            cards_due: due.len(),
            tasks_open,
            tasks_done,
            sessions,
            next_due: due.iter().map(|c| c.srs.due).min(),
        })
    }
}

/// 工作区：主题目录的集合，根目录可配置。
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 首次使用时把目录骨架建好，并放一份说明文件。
    pub fn ensure(&self) -> AppResult<()> {
        ensure_dir(&self.root)?;
        ensure_dir(&self.root.join(DIR_INTERNAL))?;
        let readme = self.root.join("README.md");
        if !readme.exists() {
            store::atomic_write(
                &readme,
                "# 学习中枢 · 工作区\n\n\
                 这个目录里的每个子目录就是一个「学习主题」。\n\
                 直接用编辑器改里面的 Markdown / PDF 都没问题，应用下次打开会重新扫描。\n\n\
                 - `notes/` 学习笔记\n\
                 - `materials/` 资料（PDF、网页存档、图片）
                 - `kb/` 讲义与课件（agent 会优先按这里的重点讲）\n\
                 - `cards/cards.jsonl` 记忆卡片\n\
                 - `plan/tasks.jsonl` 日程任务\n\
                 - `sessions/` 学习会话\n"
                    .as_bytes(),
            )?;
        }
        Ok(())
    }

    pub fn topic_dir(&self, slug: &str) -> PathBuf {
        self.root.join(slug)
    }

    /// 扫描工作区，列出所有主题（按最近打开排序）。
    pub fn list(&self) -> AppResult<Vec<TopicSummary>> {
        self.ensure()?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root).ctx(self.root.display())? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let slug = name.clone();
            let meta = self.read_or_adopt_meta(&path, &slug)?;
            let topic = Topic { meta, dir: path.clone() };
            let stats = topic.stats().unwrap_or_default();
            out.push(TopicSummary {
                meta: topic.meta,
                slug,
                path: path.to_string_lossy().to_string(),
                stats,
            });
        }
        out.sort_by(|a, b| {
            let ka = a.meta.last_opened_at.unwrap_or(a.meta.updated_at);
            let kb = b.meta.last_opened_at.unwrap_or(b.meta.updated_at);
            kb.cmp(&ka)
        });
        Ok(out)
    }

    /// 读 topic.json；没有就「认领」这个目录（补一份元数据），
    /// 这样用户手动放进来的文件夹也能直接当主题用。
    fn read_or_adopt_meta(&self, dir: &Path, slug: &str) -> AppResult<TopicMeta> {
        let meta_path = dir.join(TOPIC_FILE);
        if let Some(meta) = store::read_json_opt::<TopicMeta>(&meta_path)? {
            return Ok(meta);
        }
        let meta = TopicMeta::new(slug.to_string(), "");
        store::write_json(&meta_path, &meta)?;
        Ok(meta)
    }

    pub fn load(&self, slug: &str) -> AppResult<Topic> {
        let dir = self.topic_dir(slug);
        if !dir.is_dir() {
            return Err(AppError::NotFound(format!("主题目录不存在：{slug}")));
        }
        let meta = self.read_or_adopt_meta(&dir, slug)?;
        Ok(Topic { meta, dir })
    }

    /// 按 id / 目录名 / 显示名 找主题。agent 传进来的引用都走这里。
    pub fn resolve(&self, needle: &str) -> AppResult<Topic> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Err(AppError::invalid("主题引用为空"));
        }
        let direct = self.topic_dir(needle);
        if direct.is_dir() {
            return self.load(needle);
        }
        let all = self.list()?;
        if let Some(s) = all.iter().find(|s| s.meta.id == needle) {
            return self.load(&s.slug);
        }
        let lower = needle.to_lowercase();
        if let Some(s) = all.iter().find(|s| s.meta.name.to_lowercase() == lower) {
            return self.load(&s.slug);
        }
        if let Some(s) = all.iter().find(|s| s.meta.name.to_lowercase().contains(&lower)) {
            return self.load(&s.slug);
        }
        Err(AppError::NotFound(format!("找不到主题「{needle}」")))
    }

    /// 新建主题：建目录骨架 + topic.json + README 模板。
    pub fn create(&self, name: &str, description: &str, emoji: Option<String>) -> AppResult<Topic> {
        self.ensure()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::invalid("主题名不能为空"));
        }
        let slug = sanitize_dir_name(name);
        // 同名目录已存在时自动加序号，保证「一个主题一个目录」
        let dir = if self.topic_dir(&slug).exists() {
            unique_child(&self.root, &slug)
        } else {
            self.topic_dir(&slug)
        };

        for sub in [
            DIR_NOTES,
            DIR_MATERIALS,
            DIR_CARDS,
            DIR_PLAN,
            DIR_SESSIONS,
            DIR_QUIZZES,
            DIR_KB,
            DIR_LESSONS,
            DIR_INTERNAL,
        ] {
            ensure_dir(&dir.join(sub))?;
        }

        let mut meta = TopicMeta::new(name, description);
        meta.emoji = emoji;
        meta.last_opened_at = Some(store::now());
        let topic = Topic { meta, dir };

        store::write_json(&topic.meta_path(), &topic.meta)?;
        store::atomic_write(&topic.dir.join("README.md"), readme_template(name, description).as_bytes())?;
        // 空文件先建好，之后的追加/重写都更简单
        if !topic.cards_path().exists() {
            store::write_jsonl(&topic.cards_path(), &Vec::<Card>::new())?;
        }
        if !topic.tasks_path().exists() {
            store::write_jsonl(&topic.tasks_path(), &Vec::<PlanTask>::new())?;
        }
        Ok(topic)
    }

    /// 主题搜索：名称、简介、标签、以及笔记正文里的命中。
    pub fn search(&self, query: &str, limit: usize) -> AppResult<Vec<TopicMatch>> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for summary in self.list()? {
            let meta = &summary.meta;
            let mut hits: Vec<String> = Vec::new();
            if meta.name.to_lowercase().contains(&q) {
                hits.push("主题名".into());
            }
            if meta.description.to_lowercase().contains(&q) {
                hits.push("简介".into());
            }
            if meta.tags.iter().any(|t| t.to_lowercase().contains(&q)) {
                hits.push("标签".into());
            }

            // 正文命中：扫 notes/ 里的 Markdown
            if let Ok(dir) = self.load(&summary.slug) {
                for f in store::walk_files(&dir.notes_dir(), 4).into_iter().take(400) {
                    if !store::is_texty(&f) {
                        continue;
                    }
                    if let Ok(text) = store::read_text_capped(&f, 256 * 1024) {
                        if let Some(snippet) = first_hit_snippet(&text, &q) {
                            hits.push(format!("{}: {snippet}", dir.rel(&f)));
                            if hits.len() > 6 {
                                break;
                            }
                        }
                    }
                }
            }

            if !hits.is_empty() {
                let score = score_match(&q, &summary.meta);
                out.push(TopicMatch { summary, hits, score });
            }
        }
        out.sort_by(|a, b| b.score.cmp(&a.score));
        out.truncate(limit);
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicMatch {
    pub summary: TopicSummary,
    pub hits: Vec<String>,
    pub score: i32,
}

fn score_match(q: &str, meta: &TopicMeta) -> i32 {
    let name = meta.name.to_lowercase();
    let mut score = 0;
    if name == q {
        score += 100;
    } else if name.starts_with(q) {
        score += 60;
    } else if name.contains(q) {
        score += 40;
    }
    if meta.tags.iter().any(|t| t.to_lowercase().contains(q)) {
        score += 20;
    }
    score
}

/// 在正文里找第一处命中并截一段上下文。
fn first_hit_snippet(text: &str, q: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let idx = lower.find(q)?;
    let start = idx.saturating_sub(40);
    let end = (idx + q.len() + 60).min(text.len());
    // 对齐到字符边界，避免切碎 UTF-8
    let mut s = start;
    while s > 0 && !text.is_char_boundary(s) {
        s -= 1;
    }
    let mut e = end;
    while e < text.len() && !text.is_char_boundary(e) {
        e += 1;
    }
    Some(text[s..e].replace('\n', " ").trim().to_string())
}

fn readme_template(name: &str, description: &str) -> String {
    format!(
        "# {name}\n\n{description}\n\n\
         > 这份 README 会被 agent 当作主题背景资料读取，可以随手补充：\n\
         > 学习目标、参考书目、考试时间、自己的基础水平……\n\n\
         ## 学习目标\n\n- \n\n## 参考资料\n\n- \n\n## 备注\n\n- \n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_resolve() {
        let tmp = std::env::temp_dir().join(format!("lh-test-{}", uuid::Uuid::new_v4()));
        let ws = Workspace::new(&tmp);
        let t = ws.create("线性代数", "考试用", None).unwrap();
        assert!(t.dir.join("notes").is_dir());
        assert!(t.dir.join("cards/cards.jsonl").exists());

        let found = ws.resolve("线性代数").unwrap();
        assert_eq!(found.meta.id, t.meta.id);
        let by_id = ws.resolve(&t.meta.id).unwrap();
        assert_eq!(by_id.meta.name, "线性代数");

        let list = ws.list().unwrap();
        assert_eq!(list.len(), 1);
        let s = ws.search("线性", 10).unwrap();
        assert_eq!(s.len(), 1);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
