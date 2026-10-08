//! 工坊（Studio）的命令层：面板要的位置与规范文档、打开练习目录。
//!
//! 工坊本身没有新的数据模型——它的产物是技能目录与 MCP 配置，
//! 那两样各自已经有面板（`commands/extend.rs`）。这里只补「练习目录在哪儿」
//! 与「内置规范文档长什么样」，让界面能把这件事讲清楚。

use crate::error::AppResult;
use crate::state::AppState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioSpec {
    /// 传给 `spec_read` 的名字
    pub doc: String,
    pub title: String,
    pub hint: String,
    pub chars: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioInfo {
    /// 练习目录（工坊模式下相对路径的根）
    pub dir: String,
    /// 发布后的正式位置
    pub skills_dir: String,
    pub mcp_dir: String,
    pub specs: Vec<StudioSpec>,
    /// 工坊模式下 agent 能用的工具（界面用来解释「它能做什么」）
    pub tools: Vec<String>,
    pub workspace_root: String,
}

#[tauri::command]
pub async fn studio_info(state: State<'_, AppState>) -> AppResult<StudioInfo> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let studio = crate::studio::Studio::new(cfg.workspace_root.clone());
    // 顺手把练习目录建好：界面上的开场白会写「agent 只能写这个目录」，
    // 那句话说出口时目录就该在（否则用户去资源管理器里找一个不存在的路径）。
    if let Err(e) = studio.ensure() {
        eprintln!("[工坊] 创建练习目录失败：{e}");
    }
    Ok(StudioInfo {
        dir: studio.dir().to_string_lossy().to_string(),
        skills_dir: studio.skills_dir().to_string_lossy().to_string(),
        mcp_dir: studio.mcp_dir().to_string_lossy().to_string(),
        specs: vec![
            StudioSpec {
                doc: "skill".into(),
                title: "技能规范".into(),
                hint: "SKILL.md 的格式、description 怎么写、怎么发布".into(),
                chars: crate::studio::SPEC_SKILL.chars().count(),
            },
            StudioSpec {
                doc: "mcp".into(),
                title: "MCP 规范".into(),
                hint: "stdio + 换行分帧的最小实现、Python/Node 骨架、注册参数".into(),
                chars: crate::studio::SPEC_MCP.chars().count(),
            },
        ],
        tools: core.agent.names_for(crate::agent::registry::AgentMode::Studio),
        workspace_root: cfg.workspace_root.to_string_lossy().to_string(),
    })
}

/// 读一份规范文档（面板里的「预览」）。
#[tauri::command]
pub async fn studio_spec(state: State<'_, AppState>, doc: String) -> AppResult<String> {
    let core = state.0.clone();
    let studio = crate::studio::Studio::new(core.config_read().workspace_root.clone());
    studio.spec(&doc)
}

/// 建好并打开练习目录。
#[tauri::command]
pub async fn studio_open_dir(state: State<'_, AppState>, ensure: Option<bool>) -> AppResult<String> {
    let core = state.0.clone();
    let studio = crate::studio::Studio::new(core.config_read().workspace_root.clone());
    // 从侧栏点进来时目录可能还不存在（第一次用工坊），顺手建好
    if ensure.unwrap_or(true) {
        studio.ensure()?;
    }
    crate::commands::app::open_in_os(&studio.dir())?;
    Ok(studio.dir().to_string_lossy().to_string())
}
