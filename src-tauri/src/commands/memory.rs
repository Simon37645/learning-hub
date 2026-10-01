//! 长期记忆的命令层。
//!
//! 内存里的那份缓存在 `AppCore` 上（见 `state.rs`），这里只做「校验参数 → 调领域逻辑 →
//! 组装视图」，顺手发一条 `hub://memory` 事件让界面上的面板刷新。
//!
//! 作用域只有两级：全局（跨主题）与主题（含父主题继承）。
//! 与技能/MCP 一样，**定义与开关分开**：记忆一直在磁盘上，
//! 总开关只决定「要不要注入提示词、要不要给 agent 记忆工具」。

use crate::domain::memory::{Memory, MemoryKind, MemoryOverview, MAX_PER_SCOPE};
use crate::domain::topic::Topic;
use crate::domain::MemoryScope;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

/// 作用域参数：前端传 "global" 或 "topic"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeArg {
    Global,
    Topic,
}

fn load_topic(state: &AppState, slug: &Option<String>) -> AppResult<Option<Topic>> {
    match slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Ok(Some(state.0.workspace().resolve(s)?)),
        None => Ok(None),
    }
}

/// 解析 (作用域, 主题目录)。主题作用域必须能定位到一个主题。
fn resolve_scope(
    state: &AppState,
    scope: MemoryScopeArg,
    topic_slug: &Option<String>,
) -> AppResult<(MemoryScope, Option<std::path::PathBuf>, Option<Topic>)> {
    match scope {
        MemoryScopeArg::Global => Ok((MemoryScope::Global, None, None)),
        MemoryScopeArg::Topic => {
            let topic = load_topic(state, topic_slug)?
                .ok_or_else(|| AppError::invalid("要读写主题记忆，得先打开一个主题"))?;
            let dir = topic.dir.clone();
            Ok((MemoryScope::Topic(topic.slug()), Some(dir), Some(topic)))
        }
    }
}

/// 记忆面板要的全部数据：全局、本主题、从父主题继承的，以及「实际会注入多少」。
#[tauri::command]
pub async fn memory_overview(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<MemoryOverview> {
    let core = state.0.clone();
    let topic = load_topic(&state, &topic_slug)?;

    // 先把要展示的记忆读进缓存（用户可能刚用编辑器改过 jsonl）
    let root = core.config_read().workspace_root.clone();
    core.memory_write(|store| {
        store.load(&root)?;
        if let Some(t) = topic.as_ref() {
            let mut chain = vec![t.clone()];
            chain.extend(core.workspace().ancestors(t));
            for item in chain {
                store.load_topic(&item.slug(), &item.dir)?;
            }
        }
        Ok(())
    })?;

    // 父主题的 slug 列表（继承来的记忆只读展示，提醒用户「要改去父主题改」）
    let ancestors: Vec<String> = topic
        .as_ref()
        .map(|t| core.workspace().ancestors(t).into_iter().map(|a| a.slug()).collect())
        .unwrap_or_default();

    let enabled = core.memory_enabled();
    let view = core.memory_read(|store| match topic.as_ref() {
        Some(t) => store.overview(enabled, Some((&t.slug(), &t.meta.name, ancestors.clone()))),
        None => store.overview(enabled, None),
    });
    Ok(view)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInput {
    /// 有 id 就是修改，没有就是新增
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    pub content: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// 新增或修改一条记忆（界面上手写/编辑用）。
#[tauri::command]
pub async fn memory_upsert(
    state: State<'_, AppState>,
    input: MemoryInput,
    scope: MemoryScopeArg,
    topic_slug: Option<String>,
) -> AppResult<MemoryOverview> {
    let core = state.0.clone();
    let (scope, dir, topic) = resolve_scope(&state, scope, &topic_slug)?;
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::invalid("记忆内容不能为空"));
    }
    let kind = input
        .kind
        .as_deref()
        .and_then(MemoryKind::parse)
        .unwrap_or_default();

    let id = input.id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    match id {
        Some(id) => {
            // 编辑表单里备注是可见字段，所以这里「给了就写」——包括清空成空串
            let note = input.note.clone();
            core.memory_write(|store| {
                store.load(&core.config_read().workspace_root.clone())?;
                store.patch(
                    &scope,
                    dir.as_deref(),
                    id,
                    Some(kind),
                    Some(content),
                    input.pinned,
                    note,
                )?;
                Ok(())
            })?;
        }
        None => {
            let mut item = Memory::new(kind, content);
            item.note = input.note.unwrap_or_default().trim().to_string();
            item.source = input.source.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            item.pinned = input.pinned.unwrap_or(false);
            core.memory_write(|store| {
                store.load(&core.config_read().workspace_root.clone())?;
                store.upsert(&scope, dir.as_deref(), item)?;
                Ok(())
            })?;
        }
    }

    let where_text = match topic.as_ref() {
        Some(t) => format!("主题「{}」", t.meta.name),
        None => "全局".to_string(),
    };
    core.emit_memory_changed(&where_text);
    memory_overview(state, topic_slug.clone()).await
}

/// 删掉一条记忆。
#[tauri::command]
pub async fn memory_delete(
    state: State<'_, AppState>,
    id: String,
    scope: MemoryScopeArg,
    topic_slug: Option<String>,
) -> AppResult<MemoryOverview> {
    let core = state.0.clone();
    let (scope, dir, topic) = resolve_scope(&state, scope, &topic_slug)?;
    let ids = vec![id];
    let removed = core.memory_write(|store| {
        store.load(&core.config_read().workspace_root.clone())?;
        store.forget(&scope, dir.as_deref(), &ids, None)
    })?;
    if removed == 0 {
        return Err(AppError::NotFound("这条记忆已经不在磁盘上了".into()));
    }
    let where_text = match topic.as_ref() {
        Some(t) => format!("主题「{}」", t.meta.name),
        None => "全局".to_string(),
    };
    core.emit_memory_changed(&where_text);
    memory_overview(state, topic_slug).await
}

/// 钉住 / 取消钉住（钉住的永远优先注入）。
#[tauri::command]
pub async fn memory_set_pinned(
    state: State<'_, AppState>,
    id: String,
    pinned: bool,
    scope: MemoryScopeArg,
    topic_slug: Option<String>,
) -> AppResult<MemoryOverview> {
    let core = state.0.clone();
    let (scope, dir, _) = resolve_scope(&state, scope, &topic_slug)?;
    core.memory_write(|store| {
        store.load(&core.config_read().workspace_root.clone())?;
        store.patch(&scope, dir.as_deref(), &id, None, None, Some(pinned), None)?;
        Ok(())
    })?;
    core.emit_memory_changed("记忆");
    memory_overview(state, topic_slug).await
}

/// 清空某一级（全局或本主题）的记忆。危险操作，界面上要二次确认。
#[tauri::command]
pub async fn memory_clear(
    state: State<'_, AppState>,
    scope: MemoryScopeArg,
    topic_slug: Option<String>,
) -> AppResult<usize> {
    let core = state.0.clone();
    let (scope, dir, _) = resolve_scope(&state, scope, &topic_slug)?;
    let n = core.memory_write(|store| {
        store.load(&core.config_read().workspace_root.clone())?;
        store.clear(&scope, dir.as_deref())
    })?;
    if n > 0 {
        core.emit_memory_changed("记忆");
    }
    Ok(n)
}

/// 记忆总开关。关掉后不注入提示词、也不给 agent 记忆工具，但已有记忆仍可查看编辑。
#[tauri::command]
pub async fn memory_set_enabled(
    state: State<'_, AppState>,
    enabled: bool,
    topic_slug: Option<String>,
) -> AppResult<MemoryOverview> {
    let core = state.0.clone();
    core.update_config(|c| c.agent.memory_enabled = enabled)?;
    core.emit_memory_changed("总开关");
    memory_overview(state, topic_slug).await
}

/// 记忆的分类元数据（界面用它画下拉框，不用在前端再写一份中文名）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryKindInfo {
    pub id: String,
    pub label: String,
    pub hint: String,
}

#[tauri::command]
pub async fn memory_kinds() -> AppResult<Vec<MemoryKindInfo>> {
    Ok(MemoryKind::ALL
        .iter()
        .map(|k| MemoryKindInfo {
            id: k.slug().to_string(),
            label: k.label().to_string(),
            hint: k.hint().to_string(),
        })
        .collect())
}

/// 记忆文件的路径（面板上「打开目录」用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPaths {
    pub global: String,
    pub topic: Option<String>,
    pub limit: usize,
}

#[tauri::command]
pub async fn memory_paths(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<MemoryPaths> {
    let core = state.0.clone();
    let root = core.config_read().workspace_root.clone();
    let topic = load_topic(&state, &topic_slug)?;
    Ok(MemoryPaths {
        global: crate::domain::memory::global_path(&root).to_string_lossy().to_string(),
        topic: topic
            .as_ref()
            .map(|t| crate::domain::memory::topic_path(&t.dir).to_string_lossy().to_string()),
        limit: MAX_PER_SCOPE,
    })
}
