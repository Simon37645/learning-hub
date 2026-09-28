//! 原始文件读写命令：给内置 Markdown 编辑器用的「不加工」接口。
//!
//! 与 `note_*` 的区别：`note_*` 会解析标题 / 标签 / front matter，面向「笔记」这个业务对象；
//! 这里的接口只做「读一段文本 / 写一段文本 / 写二进制」，路径仍在主题目录内，
//! 编辑器（以及将来的其它编辑类界面）用它才不会因为业务解析而丢内容。

use crate::error::{AppError, AppResult};
use crate::paths::{ensure_dir, resolve_in_root};
use crate::state::AppState;
use crate::store;
use serde::Serialize;
use tauri::State;

/// 单次读写的体积上限，防止误操作把几百 MB 的文件塞进内存。
const MAX_BYTES: u64 = 64 * 1024 * 1024;

fn resolve(state: &AppState, topic_slug: &str, path: &str) -> AppResult<std::path::PathBuf> {
    let topic = state.0.workspace().resolve(topic_slug)?;
    resolve_in_root(&topic.dir, path)
}

#[tauri::command]
pub async fn file_read_text(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
) -> AppResult<String> {
    let abs = resolve(&state, &topic_slug, &path)?;
    let meta = std::fs::metadata(&abs).map_err(|_| AppError::NotFound(format!("文件不存在：{path}")))?;
    if meta.len() > MAX_BYTES {
        return Err(AppError::invalid(format!(
            "文件太大（{}），编辑器打不开",
            crate::paths::human_size(meta.len())
        )));
    }
    store::read_text(&abs)
}

#[tauri::command]
pub async fn file_write_text(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
    content: String,
) -> AppResult<()> {
    let core = state.0.clone();
    let abs = resolve(&state, &topic_slug, &path)?;
    if let Some(parent) = abs.parent() {
        ensure_dir(parent)?;
    }
    store::atomic_write(&abs, content.as_bytes())?;
    // 笔记数/字数这类统计要跟着变
    if let Ok(topic) = core.workspace().resolve(&topic_slug) {
        core.emit_topics_updated(&topic);
    }
    Ok(())
}

/// 写二进制（粘贴进笔记的图片）。`bytes` 由前端传 number[]，图片体积小，够用。
#[tauri::command]
pub async fn file_write_binary(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
    bytes: Vec<u8>,
) -> AppResult<()> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AppError::invalid("文件太大"));
    }
    let abs = resolve(&state, &topic_slug, &path)?;
    if let Some(parent) = abs.parent() {
        ensure_dir(parent)?;
    }
    store::atomic_write(&abs, &bytes)
}

#[tauri::command]
pub async fn file_exists(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
) -> AppResult<bool> {
    Ok(resolve(&state, &topic_slug, &path).map(|p| p.exists()).unwrap_or(false))
}

/// 删除（移进回收站，可恢复）。编辑器的「清理暂存图片」用它。
#[tauri::command]
pub async fn file_delete(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
) -> AppResult<()> {
    let core = state.0.clone();
    let abs = resolve(&state, &topic_slug, &path)?;
    if !abs.exists() {
        return Ok(()); // 幂等：本来就不在，算删成功
    }
    let trash = core
        .workspace()
        .root
        .join(crate::domain::topic::DIR_INTERNAL)
        .join("trash");
    store::move_to_trash(&trash, &abs)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub path: String,
    pub size: u64,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

/// 供编辑器判断「文件是否被外部改过」用。
#[tauri::command]
pub async fn file_meta(
    state: State<'_, AppState>,
    topic_slug: String,
    path: String,
) -> AppResult<Option<FileMeta>> {
    let abs = resolve(&state, &topic_slug, &path)?;
    if !abs.exists() {
        return Ok(None);
    }
    let meta = std::fs::metadata(&abs)?;
    Ok(Some(FileMeta {
        path,
        size: meta.len(),
        modified_at: store::modified_at(&abs),
    }))
}

/// 列出一个目录下的 md 文件（编辑器切换文档用）。
#[tauri::command]
pub async fn file_list_notes(
    state: State<'_, AppState>,
    topic_slug: String,
) -> AppResult<Vec<FileMeta>> {
    let topic = state.0.workspace().resolve(&topic_slug)?;
    let mut out = Vec::new();
    for f in store::walk_files(&topic.notes_dir(), 6) {
        if f.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let meta = match std::fs::metadata(&f) {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(FileMeta {
            path: topic.rel(&f),
            size: meta.len(),
            modified_at: store::modified_at(&f),
        });
    }
    out.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    Ok(out)
}
