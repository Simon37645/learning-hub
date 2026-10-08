//! 外观主题的命令层：列出、切换、新建/覆盖、删除、打开目录。
//!
//! 主题文件是磁盘上的普通 JSON（见 `theme.rs` 的格式说明），所以这里的命令都很薄：
//! 扫地、找出当前生效的那份、把用户改过的写回去。

use crate::config::PublicConfig;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::theme::{Theme, VAR_DOCS};
use serde::Serialize;
use tauri::State;

/// 界面上「可用的变量」帮助列表的一项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeVarDoc {
    pub name: String,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemesOverview {
    pub themes: Vec<Theme>,
    /// 配置里记着的自定义主题 id（可能已经找不到文件了，界面要能说明这件事）
    pub active: Option<String>,
    /// **当前真正生效**的那份。找不到文件 / 文件读不了时是 None（界面退回内置配色）
    pub applied: Option<Theme>,
    pub workspace_dir: String,
    pub user_dir: Option<String>,
    /// 可用的变量清单（给「自己写一份」的界面当参考）
    pub vars: Vec<ThemeVarDoc>,
}

fn overview_of(core: &crate::state::AppCore) -> ThemesOverview {
    let cfg = core.config_read();
    let themes = crate::theme::discover(&cfg.workspace_root);
    let active = cfg.appearance.custom_theme.clone();
    let applied = active
        .as_deref()
        .and_then(|id| crate::theme::find(&themes, id));
    ThemesOverview {
        themes,
        active,
        applied,
        workspace_dir: crate::theme::workspace_dir(&cfg.workspace_root)
            .to_string_lossy()
            .to_string(),
        user_dir: crate::skills::home_dir()
            .map(|h| h.join(".learning-hub").join("themes").to_string_lossy().to_string()),
        vars: VAR_DOCS
            .iter()
            .map(|(name, hint)| ThemeVarDoc {
                name: name.to_string(),
                hint: hint.to_string(),
            })
            .collect(),
    }
}

#[tauri::command]
pub async fn themes_overview(state: State<'_, AppState>) -> AppResult<ThemesOverview> {
    let core = state.0.clone();
    Ok(overview_of(&core))
}

/// 切换自定义主题。`id` 传空 / null 表示回到内置配色。
#[tauri::command]
pub async fn theme_set_active(
    state: State<'_, AppState>,
    id: Option<String>,
) -> AppResult<PublicConfig> {
    let core = state.0.clone();
    let wanted = id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(want) = &wanted {
        let themes = crate::theme::discover(&core.config_read().workspace_root);
        if crate::theme::find(&themes, want).is_none() {
            return Err(AppError::NotFound(format!(
                "没有可用的主题「{want}」——文件可能在，但读不了或者一个变量都没有"
            )));
        }
    }
    let cfg = core.update_config(|c| {
        c.appearance.custom_theme = wanted;
    })?;
    Ok(PublicConfig::from(&cfg))
}

/// 保存一份主题（新建或覆盖同 id 的工作区主题）。
#[tauri::command]
pub async fn theme_save(state: State<'_, AppState>, theme: Theme) -> AppResult<ThemesOverview> {
    let core = state.0.clone();
    let root = core.config_read().workspace_root.clone();
    let path = crate::theme::save(&root, &theme)?;
    core.toast("success", format!("主题已保存：{}", path.file_name().unwrap_or_default().to_string_lossy()));
    Ok(overview_of(&core))
}

/// 删掉一份**工作区里的**主题（进回收站）。用户目录里的主题不从这里删——
/// 那是用户自己的文件，界面只提供「打开目录」。
#[tauri::command]
pub async fn theme_delete(state: State<'_, AppState>, id: String) -> AppResult<ThemesOverview> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let path = crate::theme::workspace_dir(&cfg.workspace_root).join(format!("{id}.json"));
    if !path.is_file() {
        return Err(AppError::NotFound(format!(
            "工作区里没有 {id}.json —— 只删得掉工作区里的主题，用户目录里的请到那边的目录里删"
        )));
    }
    let trash = cfg
        .workspace_root
        .join(crate::domain::topic::DIR_INTERNAL)
        .join("trash");
    crate::store::move_to_trash(&trash, &path)?;
    // 删掉的正好是当前生效的那份：配置里也清掉，否则界面会一直说「主题没找到」
    if cfg.appearance.custom_theme.as_deref() == Some(id.as_str()) {
        core.update_config(|c| c.appearance.custom_theme = None)?;
    }
    core.toast("info", "主题已移入工作区的 .hub/trash（可手动恢复）");
    Ok(overview_of(&core))
}

/// 在文件管理器里打开主题目录（工作区 / 用户目录）。
#[tauri::command]
pub async fn theme_open_dir(state: State<'_, AppState>, user: Option<bool>) -> AppResult<String> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let dir = if user.unwrap_or(false) {
        let home = crate::skills::home_dir()
            .ok_or_else(|| AppError::other("找不到用户目录（USERPROFILE / HOME 都没设）"))?;
        home.join(".learning-hub").join("themes")
    } else {
        crate::theme::workspace_dir(&cfg.workspace_root)
    };
    crate::paths::ensure_dir(&dir)?;
    crate::commands::app::open_in_os(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}
