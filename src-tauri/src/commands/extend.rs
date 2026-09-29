//! 技能与 MCP 的命令层。
//!
//! 两级作用域：
//! - **全局**：写进 `config.json`，所有主题都看得见
//! - **本主题**：写进该主题的 `topic.json`，只在这个主题生效
//!
//! 「定义」与「启用」是分开的：一个全局技能可以在某个主题里被单独关掉，
//! 但它的文件还在，别的主题照样能用。列表接口会把两级的实际状态都算好给界面。

use crate::error::{AppError, AppResult};
use crate::mcp::McpServerConfig;
use crate::state::{AppState, McpStatusEntry};
use crate::store;
use serde::{Deserialize, Serialize};
use tauri::State;

/// 作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Topic,
}

fn load_topic(state: &AppState, slug: &Option<String>) -> AppResult<Option<crate::domain::topic::Topic>> {
    match slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Ok(Some(state.0.workspace().resolve(s)?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------- 技能

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub dir: String,
    pub source: String,
    /// global / topic
    pub scope: String,
    /// 在当前上下文里是否生效
    pub enabled: bool,
    /// 被哪一级关掉的（global / topic / null），界面上说明原因
    pub disabled_by: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOverview {
    /// 总开关
    pub enabled: bool,
    /// 全局技能（含被本主题关掉的，用于展示开关状态）
    pub global: Vec<SkillEntry>,
    /// 只在当前主题里的技能
    pub topic: Vec<SkillEntry>,
    /// 当前主题名（没有主题时为 null）
    pub topic_name: Option<String>,
}

#[tauri::command]
pub async fn skills_overview(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<SkillsOverview> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let topic = load_topic(&state, &topic_slug)?;

    // 全局技能：不带主题私有目录扫一遍
    let global_all = crate::skills::discover(
        Some(&cfg.workspace_root),
        &[],
        &cfg.agent.extra_skill_dirs,
    );
    // 主题私有：自己的 + 父主题的（同一门课共用的技能，子主题也看得见）
    let topic_all = match &topic {
        Some(t) => {
            let ws = core.workspace();
            let mut dirs = vec![t.dir.clone()];
            dirs.extend(ws.ancestors(t).into_iter().map(|a| a.dir));
            crate::skills::discover(None, &dirs, &[])
        }
        None => Vec::new(),
    };

    let entry = |s: &crate::skills::Skill, scope: &str| -> SkillEntry {
        let global_off = cfg.agent.disabled_skills.iter().any(|d| d == &s.id);
        // 「本主题已关」和「父主题已关」都来自主题层，但要让用户分得清是谁关的
        let topic_off = core.skill_disabled_by_topic(&s.id, topic.as_ref());
        SkillEntry {
            id: s.id.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            dir: s.dir.clone(),
            source: s.source.clone(),
            scope: scope.to_string(),
            enabled: cfg.agent.skills_enabled && !global_off && topic_off.is_none(),
            disabled_by: if !cfg.agent.skills_enabled {
                Some("总开关".into())
            } else if global_off {
                Some("global".into())
            } else {
                topic_off.map(|s| s.to_string())
            },
            files: s.files.clone(),
        }
    };

    Ok(SkillsOverview {
        enabled: cfg.agent.skills_enabled,
        global: global_all.iter().map(|s| entry(s, "global")).collect(),
        // 主题私有里若有与全局同名的，按「主题优先」处理，不在两组里重复列出
        topic: topic_all
            .iter()
            .filter(|s| !global_all.iter().any(|g| g.id == s.id))
            .map(|s| entry(s, "topic"))
            .collect(),
        topic_name: topic.as_ref().map(|t| t.meta.name.clone()),
    })
}

/// 开关某个技能（全局或本主题）。
#[tauri::command]
pub async fn skill_set_enabled(
    state: State<'_, AppState>,
    skill_id: String,
    scope: Scope,
    enabled: bool,
    topic_slug: Option<String>,
) -> AppResult<()> {
    let core = state.0.clone();
    match scope {
        Scope::Global => {
            core.update_config(|c| {
                c.agent.disabled_skills.retain(|d| d != &skill_id);
                if !enabled {
                    c.agent.disabled_skills.push(skill_id.clone());
                }
            })?;
            core.reload_skills();
        }
        Scope::Topic => {
            let Some(mut topic) = load_topic(&state, &topic_slug)? else {
                return Err(AppError::invalid("要按主题设置开关，得先打开一个主题"));
            };
            topic.meta.tools.set_skill(&skill_id, enabled);
            topic.save_meta()?;
            core.emit_topics_updated(&topic);
        }
    }
    Ok(())
}

/// 技能总开关。
#[tauri::command]
pub async fn skills_set_enabled(state: State<'_, AppState>, enabled: bool) -> AppResult<()> {
    let core = state.0.clone();
    core.update_config(|c| c.agent.skills_enabled = enabled)?;
    core.reload_skills();
    Ok(())
}

/// 重新扫描技能目录（用户在文件管理器里加了新技能之后点一下）。
#[tauri::command]
pub async fn skills_reload(state: State<'_, AppState>) -> AppResult<usize> {
    Ok(state.0.reload_skills())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDirs {
    pub workspace: String,
    pub topic: Option<String>,
    pub user: Option<String>,
    pub extra: Vec<String>,
}

#[tauri::command]
pub async fn skills_dirs(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<SkillDirs> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let workspace = cfg
        .workspace_root
        .join(crate::domain::topic::DIR_INTERNAL)
        .join("skills");
    let _ = crate::paths::ensure_dir(&workspace);

    let topic_dir = load_topic(&state, &topic_slug)?.map(|t| {
        let d = t.dir.join(crate::domain::topic::DIR_INTERNAL).join("skills");
        let _ = crate::paths::ensure_dir(&d);
        d.to_string_lossy().to_string()
    });

    Ok(SkillDirs {
        workspace: workspace.to_string_lossy().to_string(),
        topic: topic_dir,
        user: crate::skills::home_dir()
            .map(|h| h.join(".agents").join("skills").to_string_lossy().to_string()),
        extra: cfg.agent.extra_skill_dirs.clone(),
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInput {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
}

/// 保存一份技能（从界面直接写 SKILL.md）。
#[tauri::command]
pub async fn skill_save(
    state: State<'_, AppState>,
    input: SkillInput,
    scope: Scope,
    topic_slug: Option<String>,
    overwrite: Option<bool>,
) -> AppResult<String> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let id = crate::paths::sanitize_dir_name(&input.id);
    if id.trim().is_empty() {
        return Err(AppError::invalid("技能目录名不能为空"));
    }

    let base = match scope {
        Scope::Global => cfg
            .workspace_root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("skills"),
        Scope::Topic => {
            let topic = load_topic(&state, &topic_slug)?
                .ok_or_else(|| AppError::invalid("要写到主题里，得先打开一个主题"))?;
            topic
                .dir
                .join(crate::domain::topic::DIR_INTERNAL)
                .join("skills")
        }
    };

    let dir = base.join(&id);
    crate::paths::ensure_dir(&dir)?;
    let file = dir.join(crate::skills::SKILL_FILE);
    if file.exists() && !overwrite.unwrap_or(false) {
        return Err(AppError::invalid(format!("技能 {id} 已存在")));
    }
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        input.name.trim(),
        input.description.trim(),
        input.body.trim()
    );
    store::atomic_write(&file, content.as_bytes())?;
    core.reload_skills();
    Ok(file.to_string_lossy().to_string())
}

// ---------------------------------------------------------------- MCP

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEntryView {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub scope: String,
    /// 全局启用状态（主题级条目恒为 true）
    pub enabled: bool,
    /// 在当前主题下是否生效
    pub enabled_here: bool,
    pub connected: bool,
    pub server_info: String,
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOverview {
    pub global: Vec<McpEntryView>,
    pub topic: Vec<McpEntryView>,
    pub topic_name: Option<String>,
}

#[tauri::command]
pub async fn mcp_overview(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<McpOverview> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let topic = load_topic(&state, &topic_slug)?;
    let status = core.mcp_status().await;

    let status_of = |name: &str| status.iter().find(|s| s.name == name).cloned();

    let build = |server: &McpServerConfig, scope: &str| -> McpEntryView {
        let st: Option<McpStatusEntry> = status_of(&server.name);
        let enabled_here = core.mcp_enabled_for_topic(&server.name, topic.as_ref());
        McpEntryView {
            name: server.name.clone(),
            command: format!("{} {}", server.command, server.args.join(" ")).trim().to_string(),
            args: server.args.clone(),
            scope: scope.to_string(),
            enabled: server.enabled,
            enabled_here,
            connected: st.as_ref().map(|s| s.connected).unwrap_or(false),
            server_info: st.as_ref().map(|s| s.server_info.clone()).unwrap_or_default(),
            tool_count: st.as_ref().map(|s| s.tool_count).unwrap_or(0),
            tools: st.as_ref().map(|s| s.tools.clone()).unwrap_or_default(),
            error: st.as_ref().and_then(|s| s.error.clone()),
        }
    };

    let topic_servers: Vec<McpServerConfig> = topic
        .as_ref()
        .map(|t| t.meta.tools.extra_mcp.clone())
        .unwrap_or_default();

    Ok(McpOverview {
        global: cfg.agent.mcp_servers.iter().map(|s| build(s, "global")).collect(),
        topic: topic_servers.iter().map(|s| build(s, "topic")).collect(),
        topic_name: topic.as_ref().map(|t| t.meta.name.clone()),
    })
}

#[tauri::command]
pub async fn mcp_status(state: State<'_, AppState>) -> AppResult<Vec<McpStatusEntry>> {
    Ok(state.0.mcp_status().await)
}

#[tauri::command]
pub async fn mcp_reload(state: State<'_, AppState>) -> AppResult<Vec<McpStatusEntry>> {
    Ok(state.0.mcp_reload().await)
}

/// 增删改一个 MCP 服务器（可指定作用域）。
#[tauri::command]
pub async fn mcp_upsert(
    state: State<'_, AppState>,
    server: McpServerConfig,
    scope: Scope,
    topic_slug: Option<String>,
    original_name: Option<String>,
) -> AppResult<Vec<McpStatusEntry>> {
    let core = state.0.clone();
    let mut server = server;
    server.name = server.name.trim().to_string();
    if server.name.is_empty() || server.command.trim().is_empty() {
        return Err(AppError::invalid("服务器名与启动命令都不能为空"));
    }

    match scope {
        Scope::Global => {
            core.update_config(|c| {
                let key = original_name.clone().unwrap_or_else(|| server.name.clone());
                match c.agent.mcp_servers.iter_mut().find(|s| s.name == key) {
                    Some(slot) => *slot = server.clone(),
                    None => c.agent.mcp_servers.push(server.clone()),
                }
            })?;
        }
        Scope::Topic => {
            let Some(mut topic) = load_topic(&state, &topic_slug)? else {
                return Err(AppError::invalid("要加到主题里，得先打开一个主题"));
            };
            let key = original_name.clone().unwrap_or_else(|| server.name.clone());
            match topic.meta.tools.extra_mcp.iter_mut().find(|s| s.name == key) {
                Some(slot) => *slot = server.clone(),
                None => topic.meta.tools.extra_mcp.push(server.clone()),
            }
            topic.save_meta()?;
            core.emit_topics_updated(&topic);
        }
    }
    Ok(core.mcp_reload().await)
}

#[tauri::command]
pub async fn mcp_delete(
    state: State<'_, AppState>,
    name: String,
    scope: Scope,
    topic_slug: Option<String>,
) -> AppResult<Vec<McpStatusEntry>> {
    let core = state.0.clone();
    match scope {
        Scope::Global => {
            core.update_config(|c| c.agent.mcp_servers.retain(|s| s.name != name))?;
        }
        Scope::Topic => {
            let Some(mut topic) = load_topic(&state, &topic_slug)? else {
                return Err(AppError::invalid("要按主题删除，得先打开一个主题"));
            };
            topic.meta.tools.extra_mcp.retain(|s| s.name != name);
            topic.save_meta()?;
            core.emit_topics_updated(&topic);
        }
    }
    Ok(core.mcp_reload().await)
}

/// 在某一级开关服务器：全局级改 `enabled` 字段，主题级记进禁用列表。
#[tauri::command]
pub async fn mcp_set_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
    scope: Scope,
    topic_slug: Option<String>,
) -> AppResult<Vec<McpStatusEntry>> {
    let core = state.0.clone();
    match scope {
        Scope::Global => {
            core.update_config(|c| {
                if let Some(s) = c.agent.mcp_servers.iter_mut().find(|s| s.name == name) {
                    s.enabled = enabled;
                }
            })?;
            Ok(core.mcp_reload().await)
        }
        Scope::Topic => {
            let Some(mut topic) = load_topic(&state, &topic_slug)? else {
                return Err(AppError::invalid("要按主题设置开关，得先打开一个主题"));
            };
            topic.meta.tools.set_mcp(&name, enabled);
            topic.save_meta()?;
            core.emit_topics_updated(&topic);
            // 主题级开关只影响暴露给模型的工具，不用重连
            Ok(core.mcp_status().await)
        }
    }
}
