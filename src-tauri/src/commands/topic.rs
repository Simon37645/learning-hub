//! 主题 / 笔记 / 资料相关命令。

use crate::domain::note::{self, Note, NoteSummary};
use crate::domain::session::StudySession;
use crate::domain::topic::{Topic, TopicMatch, TopicStats, TopicSummary};
use crate::error::{AppError, AppResult};
use crate::paths::human_size;
use crate::state::AppState;
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;

/// 打开一个主题时前端需要的一切。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDetail {
    pub meta: crate::domain::topic::TopicMeta,
    pub slug: String,
    pub path: String,
    pub stats: TopicStats,
    pub notes: Vec<NoteSummary>,
    pub materials: Vec<MaterialItem>,
    pub sessions: Vec<StudySession>,
    pub chats: Vec<String>,
    /// 进行中的学习会话（若有）
    pub current_session: Option<StudySession>,
    pub readme: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialItem {
    /// 相对**所属主题**的路径（继承来的资料就是相对父主题）
    pub path: String,
    pub name: String,
    pub size: u64,
    pub size_text: String,
    pub kind: String,
    pub modified_at: DateTime<Utc>,
    /// 所属主题的 slug：打开这份资料要用它，而不是当前主题
    pub topic: String,
    /// 来源主题名；本主题自己的资料为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 继承自父主题的资料（只读，不能在这里改名/删除）
    pub inherited: bool,
}

#[tauri::command]
pub async fn topic_list(state: State<'_, AppState>) -> AppResult<Vec<TopicSummary>> {
    state.0.workspace().list()
}

#[tauri::command]
pub async fn topic_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> AppResult<Vec<TopicMatch>> {
    state.0.workspace().search(&query, limit.unwrap_or(20))
}

#[tauri::command]
pub async fn topic_create(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
    emoji: Option<String>,
    parent: Option<String>,
) -> AppResult<TopicDetail> {
    let core = state.0.clone();
    let topic = core
        .workspace()
        .create(&name, description.as_deref().unwrap_or(""), emoji, parent.as_deref())?;
    core.emit_topics_created(&topic);
    detail_of(&core, topic, true)
}

/// 改父子关系：把主题挂到另一个主题下面（或传 null 移出来）。
#[tauri::command]
pub async fn topic_set_parent(
    state: State<'_, AppState>,
    slug: String,
    parent: Option<String>,
) -> AppResult<TopicDetail> {
    let core = state.0.clone();
    let topic = core.workspace().set_parent(&slug, parent.as_deref())?;
    core.emit_topics_updated(&topic);
    detail_of(&core, topic, true)
}

/// 读主题详情。
///
/// 这里**必须带上笔记清单**：前端会在 agent 写完文件、或用户切换笔记之后
/// 重新拉一次详情来刷新界面；如果这次返回空的笔记列表，界面就会被刷成空的。
/// 笔记清单是本地目录扫描，几十毫秒的事，不值得为省这点开销埋这个坑。
#[tauri::command]
pub async fn topic_get(state: State<'_, AppState>, slug: String) -> AppResult<TopicDetail> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    detail_of(&core, topic, true)
}

/// 打开主题：记录访问时间、更新窗口标题、让前端切到工作台。
#[tauri::command]
pub async fn topic_open(state: State<'_, AppState>, slug: String) -> AppResult<TopicDetail> {
    let core = state.0.clone();
    // 换主题前把上一个主题攒着的「记忆被用到过」落盘（不然换了主题就丢了）
    core.memory_flush();
    let mut topic = core.workspace().resolve(&slug)?;
    topic.meta.last_opened_at = Some(Utc::now());
    topic.save_meta()?;
    core.update_config(|c| c.last_topic = Some(topic.slug()))?;
    core.set_window_title(Some(&topic.meta.name));
    detail_of(&core, topic, true)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub emoji: Option<String>,
    pub tags: Option<Vec<String>>,
    pub stage: Option<String>,
}

#[tauri::command]
pub async fn topic_update(
    state: State<'_, AppState>,
    slug: String,
    patch: TopicPatch,
) -> AppResult<TopicDetail> {
    let core = state.0.clone();
    let mut topic = core.workspace().resolve(&slug)?;

    if let Some(d) = patch.description.clone() {
        topic.meta.description = d;
    }
    if let Some(e) = patch.emoji.clone() {
        topic.meta.emoji = if e.trim().is_empty() { None } else { Some(e) };
    }
    if let Some(t) = patch.tags.clone() {
        topic.meta.tags = t;
    }
    if let Some(s) = patch.stage.as_deref() {
        if let Some(stage) = crate::domain::stage::StudyStage::parse(s) {
            topic.meta.stage = stage;
        }
    }

    // 改名要连目录一起改，否则「一个主题就是一个目录」的约定会破
    if let Some(new_name) = patch.name.clone() {
        let new_name = new_name.trim().to_string();
        if !new_name.is_empty() && new_name != topic.meta.name {
            let parent = topic
                .dir
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| AppError::other("主题目录没有父目录"))?;
            let target = crate::paths::unique_child(&parent, &crate::paths::sanitize_dir_name(&new_name));
            std::fs::rename(&topic.dir, &target)?;
            topic.dir = target;
            topic.meta.name = new_name;
        }
    }

    topic.save_meta()?;
    core.emit_topics_updated(&topic);
    detail_of(&core, topic, true)
}

/// 删除主题：移进工作区内的回收站，不真删。
///
/// 子主题一起进回收站——它们本来就是为这门课建的章节，
/// 留下孤零零的子主题既看不懂也找不回来。回收站里的目录名带时间戳，
/// 需要时手动搬回工作区即可恢复（父子关系记在各自的 topic.json 里，不会丢）。
#[tauri::command]
pub async fn topic_delete(state: State<'_, AppState>, slug: String) -> AppResult<String> {
    let core = state.0.clone();
    let ws = core.workspace();
    let topic = ws.resolve(&slug)?;
    let children = ws.descendants(&topic);
    let trash = ws.root.join(crate::domain::topic::DIR_INTERNAL).join("trash");

    let dest = store::move_to_trash(&trash, &topic.dir)?;
    core.emit_topics(crate::agent::event::TopicsEvent::Deleted {
        slug: topic.slug(),
    });
    for child in children {
        match store::move_to_trash(&trash, &child.dir) {
            Ok(_) => core.emit_topics(crate::agent::event::TopicsEvent::Deleted {
                slug: child.slug(),
            }),
            // 个别子主题搬不动（被占用等）不该让整次删除失败：父主题已经进去了，
            // 剩下的报给用户让他手动处理，比回滚一半更清楚。
            Err(e) => eprintln!("[topic] 子主题 {} 移入回收站失败：{e}", child.dir.display()),
        }
    }
    Ok(dest.to_string_lossy().to_string())
}

fn detail_of(core: &std::sync::Arc<crate::state::AppCore>, topic: Topic, with_notes: bool) -> AppResult<TopicDetail> {
    let stats = topic.stats()?;
    let ancestors = core.workspace().ancestors(&topic);
    let notes = if with_notes { collect_notes(&topic).unwrap_or_default() } else { Vec::new() };
    let materials = collect_materials(&topic, &ancestors).unwrap_or_default();
    let sessions = collect_sessions(&topic).unwrap_or_default();
    let chats = crate::agent::list_transcripts(core, Some(&topic.slug())).unwrap_or_default();
    let readme = store::read_text_opt(&topic.dir.join("README.md")).ok().flatten();

    Ok(TopicDetail {
        meta: topic.meta.clone(),
        slug: topic.slug(),
        path: topic.dir.to_string_lossy().to_string(),
        stats,
        notes,
        materials,
        sessions,
        chats,
        current_session: core.current_session(),
        readme,
    })
}

fn collect_notes(topic: &Topic) -> AppResult<Vec<NoteSummary>> {
    let mut items = Vec::new();
    for f in store::walk_files(&topic.notes_dir(), 4) {
        if f.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let Ok(text) = store::read_text_capped(&f, 256 * 1024) else { continue };
        let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        items.push(NoteSummary::from_file(topic.rel(&f), &text, size, store::modified_at(&f)));
    }
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(items)
}

/// 资料清单 = 本主题的 + 祖先主题的（只读继承）。
///
/// 典型场景：一门课的资料（讲义、真题）放在父主题里，学某一章时不必重新导入，
/// 打开时用「所属主题 + 主题内相对路径」定位，所以这里每项都带上 `topic`。
fn collect_materials(topic: &Topic, ancestors: &[Topic]) -> AppResult<Vec<MaterialItem>> {
    let mut items: Vec<MaterialItem> = Vec::new();

    let push = |owner: &Topic, origin: Option<String>, inherited: bool, items: &mut Vec<MaterialItem>| {
        for f in store::walk_files(&owner.materials_dir(), 4) {
            let meta = std::fs::metadata(&f).ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let ext = f
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            items.push(MaterialItem {
                path: owner.rel(&f),
                name: f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                size,
                size_text: human_size(size),
                kind: ext,
                modified_at: store::modified_at(&f),
                topic: owner.slug(),
                origin: origin.clone(),
                inherited,
            });
        }
    };

    push(topic, None, false, &mut items);
    for a in ancestors {
        push(a, Some(a.meta.name.clone()), true, &mut items);
    }
    // 自己的资料排在前面，继承的按来源顺序跟在后面；组内按修改时间
    items.sort_by(|a, b| {
        a.inherited
            .cmp(&b.inherited)
            .then(a.origin.cmp(&b.origin))
            .then(a.modified_at.cmp(&b.modified_at))
    });
    Ok(items)
}


fn collect_sessions(topic: &Topic) -> AppResult<Vec<StudySession>> {
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

// ---------------------------------------------------------------- 笔记

#[tauri::command]
pub async fn note_list(
    state: State<'_, AppState>,
    slug: String,
    query: Option<String>,
) -> AppResult<Vec<NoteSummary>> {
    let topic = state.0.workspace().resolve(&slug)?;
    let mut items = collect_notes(&topic)?;
    if let Some(q) = query.map(|q| q.to_lowercase()).filter(|q| !q.trim().is_empty()) {
        items.retain(|n| {
            n.title.to_lowercase().contains(&q)
                || n.tags.iter().any(|t| t.to_lowercase().contains(&q))
                || n.excerpt.to_lowercase().contains(&q)
        });
    }
    Ok(items)
}

#[tauri::command]
pub async fn note_get(state: State<'_, AppState>, slug: String, path: String) -> AppResult<Note> {
    let topic = state.0.workspace().resolve(&slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &path)?;
    if !abs.exists() {
        return Err(AppError::NotFound(format!("笔记不存在：{path}")));
    }
    let content = store::read_text(&abs)?;
    let stem = abs.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let title = note::extract_title(&content, &stem);
    let tags = note::parse_front_matter(&content).tags;
    Ok(Note {
        path: topic.rel(&abs),
        title,
        tags,
        content,
        updated_at: store::modified_at(&abs),
    })
}

#[tauri::command]
pub async fn note_save(
    state: State<'_, AppState>,
    slug: String,
    path: String,
    content: String,
) -> AppResult<NoteSummary> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &path)?;
    store::atomic_write(&abs, content.as_bytes())?;
    core.emit_topics_updated(&topic);
    let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
    Ok(NoteSummary::from_file(
        topic.rel(&abs),
        &content,
        size,
        store::modified_at(&abs),
    ))
}

#[tauri::command]
pub async fn note_create(
    state: State<'_, AppState>,
    slug: String,
    title: String,
    dir: Option<String>,
) -> AppResult<Note> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let target_dir = match dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(sub) => {
            store::ensure_dir(&topic.notes_dir())?;
            crate::paths::resolve_in_root(&topic.notes_dir(), sub)?
        }
        None => topic.notes_dir(),
    };
    store::ensure_dir(&target_dir)?;
    let stem = crate::paths::sanitize_dir_name(&title);
    let abs = crate::paths::unique_child(&target_dir, &format!("{stem}.md"));
    let content = format!(
        "---\ntitle: {title}\ncreated: {}\n---\n\n# {title}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    );
    store::atomic_write(&abs, content.as_bytes())?;
    core.emit_topics_updated(&topic);
    Ok(Note {
        path: topic.rel(&abs),
        title,
        tags: Vec::new(),
        content,
        updated_at: store::modified_at(&abs),
    })
}

#[tauri::command]
pub async fn note_delete(state: State<'_, AppState>, slug: String, path: String) -> AppResult<String> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &path)?;
    if !abs.exists() {
        return Err(AppError::NotFound(format!("笔记不存在：{path}")));
    }
    let trash = core.workspace().root.join(crate::domain::topic::DIR_INTERNAL).join("trash");
    let dest = store::move_to_trash(&trash, &abs)?;
    core.emit_topics_updated(&topic);
    Ok(dest.to_string_lossy().to_string())
}

// ---------------------------------------------------------------- 资料

#[tauri::command]
pub async fn material_list(state: State<'_, AppState>, slug: String) -> AppResult<Vec<MaterialItem>> {
    let ws = state.0.workspace();
    let topic = ws.resolve(&slug)?;
    let ancestors = ws.ancestors(&topic);
    collect_materials(&topic, &ancestors)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbRebuildResult {
    pub files: usize,
    pub chunks: usize,
    /// 有页码的文件数（PDF 走了前端的分页缓存）
    pub paged: usize,
}

/// 重建知识库索引（资料页面的按钮）。
///
/// 页数来自前端 pdf.js 的逐页文本缓存（`<主题>/.hub/pdf-pages/`），
/// 所以检索结果能带「第 N 页」，agent 引用时照抄页码即可定位。
#[tauri::command]
pub async fn kb_rebuild(state: State<'_, AppState>, slug: String) -> AppResult<KbRebuildResult> {
    let core = state.0.clone();
    let ws = core.workspace();
    let topic = ws.resolve(&slug)?;
    let inherited = ws.inherited_dirs(&topic);
    let index = crate::kb::build(&topic.dir, &inherited, true).await?;
    core.emit_topics_updated(&topic);
    Ok(KbRebuildResult {
        files: index.files.len(),
        chunks: index.chunks.len(),
        paged: index.files.iter().filter(|f| f.chunks_with_pages > 0).count(),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialImportResult {
    pub imported: Vec<MaterialItem>,
    pub skipped: Vec<String>,
    pub dir: String,
}

/// 把外部文件复制进 `materials/`（同名时自动加序号）。
#[tauri::command]
pub async fn material_import(
    state: State<'_, AppState>,
    slug: String,
    sources: Vec<String>,
    subdir: Option<String>,
) -> AppResult<MaterialImportResult> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&slug)?;
    let target_dir = match subdir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(sub) => {
            store::ensure_dir(&topic.materials_dir())?;
            crate::paths::resolve_in_root(&topic.materials_dir(), sub)?
        }
        None => topic.materials_dir(),
    };
    store::ensure_dir(&target_dir)?;

    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    for src in &sources {
        let src_path = std::path::Path::new(src);
        if !src_path.is_file() {
            skipped.push(src.clone());
            continue;
        }
        let name = src_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "imported".into());
        let dest = crate::paths::unique_child(&target_dir, &name);
        match std::fs::copy(src_path, &dest) {
            Ok(_) => {
                let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                imported.push(MaterialItem {
                    path: topic.rel(&dest),
                    name: dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    size,
                    size_text: human_size(size),
                    kind: dest
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default(),
                    modified_at: store::modified_at(&dest),
                    topic: topic.slug(),
                    origin: None,
                    inherited: false,
                });
            }
            Err(e) => {
                eprintln!("[material] 复制失败 {}：{e}", src);
                skipped.push(src.clone());
            }
        }
    }
    core.emit_topics_updated(&topic);
    Ok(MaterialImportResult {
        imported,
        skipped,
        dir: topic.rel(&target_dir),
    })
}
