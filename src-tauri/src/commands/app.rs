//! 应用级命令：启动引导、配置读写、模型档案管理、连通性测试。

use crate::agent::prompt;
use crate::agent::provider::{self, ChatRequest, StreamEvent};
use crate::agent::registry::ToolSpec;
use crate::agent::message::ChatMessage;
use crate::config::{PermissionMode, ProviderKind, ProviderProfile, PublicConfig};
use crate::domain::topic::TopicSummary;
use crate::error::{AppError, AppResult};
use crate::state::{AppState};
use crate::store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub version: String,
    pub config: PublicConfig,
    pub topics: Vec<TopicSummary>,
    pub tools: Vec<ToolSpec>,
    pub tool_names: Vec<String>,
    pub permission_modes: Vec<PermissionModeView>,
    pub provider_kinds: Vec<ProviderKindView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionModeView {
    pub id: PermissionMode,
    pub label: String,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKindView {
    pub id: ProviderKind,
    pub label: String,
    pub default_base_url: String,
    pub default_model: String,
}

/// 一次拿全启动所需数据，避免前端串行发多个请求。
#[tauri::command]
pub async fn app_bootstrap(state: State<'_, AppState>) -> AppResult<Bootstrap> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let topics = core.workspace().list().unwrap_or_default();
    let tools = core.agent.tools_specs();
    let tool_names = core.agent.tool_names();
    core.set_window_title(None);

    Ok(Bootstrap {
        version: env!("CARGO_PKG_VERSION").to_string(),
        config: PublicConfig::from(&cfg),
        topics,
        tools,
        tool_names,
        permission_modes: vec![
            PermissionModeView {
                id: PermissionMode::Ask,
                label: PermissionMode::Ask.label().into(),
                hint: "读操作直接执行；写文件、建卡片、删除都需要你点头".into(),
            },
            PermissionModeView {
                id: PermissionMode::AutoEdit,
                label: PermissionMode::AutoEdit.label().into(),
                hint: "写操作直接执行；只有删除/覆盖才打断你".into(),
            },
            PermissionModeView {
                id: PermissionMode::Full,
                label: PermissionMode::Full.label().into(),
                hint: "全部放行，不再打断（适合信任的场景）".into(),
            },
        ],
        provider_kinds: vec![
            ProviderKindView {
                id: ProviderKind::OpenAi,
                label: ProviderKind::OpenAi.label().into(),
                default_base_url: "https://api.deepseek.com/v1".into(),
                default_model: "deepseek-chat".into(),
            },
            ProviderKindView {
                id: ProviderKind::Anthropic,
                label: ProviderKind::Anthropic.label().into(),
                default_base_url: "https://api.anthropic.com/v1".into(),
                default_model: "claude-sonnet-4-5".into(),
            },
        ],
    })
}

#[tauri::command]
pub async fn config_get(state: State<'_, AppState>) -> AppResult<PublicConfig> {
    Ok(PublicConfig::from(&state.0.config_read()))
}

/// 局部更新配置。只覆盖传了值的字段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPatch {
    pub user_name: Option<String>,
    pub workspace_root: Option<String>,
    pub active_profile_id: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub allow_web: Option<bool>,
    pub sandbox: Option<bool>,
    pub max_iterations: Option<u32>,
    pub system_prompt_extra: Option<String>,
    pub request_timeout_secs: Option<u64>,
    pub context_budget_chars: Option<usize>,
    pub home_url: Option<String>,
    pub search_engine: Option<String>,
    pub anki_url: Option<String>,
    pub anki_deck_prefix: Option<String>,
    /// 撤销某个已授权目录
    pub revoke_root: Option<String>,
    /// 清空越权白名单
    pub clear_approved_roots: Option<bool>,
    /// 主题模式：system / light / dark
    pub theme: Option<crate::config::ThemeMode>,
}

#[tauri::command]
pub async fn config_patch(state: State<'_, AppState>, patch: ConfigPatch) -> AppResult<PublicConfig> {
    let core = state.0.clone();
    let old_root = core.config_read().workspace_root.clone();
    let patch_cleared_roots = patch.clear_approved_roots.unwrap_or(false);
    let cfg = core.update_config(|c| {
        if let Some(v) = patch.user_name.clone() {
            if !v.trim().is_empty() {
                c.user_name = v.trim().to_string();
            }
        }
        if let Some(v) = patch.workspace_root.clone() {
            if !v.trim().is_empty() {
                c.workspace_root = std::path::PathBuf::from(v.trim());
            }
        }
        if let Some(v) = patch.active_profile_id.clone() {
            if c.profile(&v).is_some() {
                c.active_profile_id = v;
            }
        }
        if let Some(v) = patch.permission_mode {
            c.agent.permission_mode = v;
        }
        if let Some(v) = patch.allow_web {
            c.agent.allow_web = v;
        }
        if let Some(v) = patch.sandbox {
            c.agent.sandbox = v;
        }
        if let Some(t) = patch.theme {
            c.appearance.theme = t;
        }
        if patch.clear_approved_roots.unwrap_or(false) {
            c.agent.approved_roots.clear();
        }
        if let Some(v) = patch.anki_url.clone() {
            if !v.trim().is_empty() {
                c.anki.url = v.trim().to_string();
            }
        }
        if let Some(v) = patch.anki_deck_prefix.clone() {
            c.anki.deck_prefix = v.trim().to_string();
        }
        if let Some(v) = patch.max_iterations {
            c.agent.max_iterations = v.clamp(1, 100);
        }
        if let Some(v) = patch.system_prompt_extra.clone() {
            c.agent.system_prompt_extra = v;
        }
        if let Some(v) = patch.request_timeout_secs {
            c.agent.request_timeout_secs = v.clamp(30, 1800);
        }
        if let Some(v) = patch.context_budget_chars {
            c.agent.context_budget_chars = v.clamp(2_000, 500_000);
        }
        if let Some(v) = patch.home_url.clone() {
            if !v.trim().is_empty() {
                c.viewer.home_url = v.trim().to_string();
            }
        }
        if let Some(v) = patch.search_engine.clone() {
            if !v.trim().is_empty() {
                c.viewer.search_engine = v.trim().to_string();
            }
        }
    })?;

    // 换了工作区就顺手把新目录建好，并把静态资源白名单挪过去
    if cfg.workspace_root != old_root {
        core.workspace().ensure()?;
        if let Err(e) = core.allow_asset_dir(&cfg.workspace_root) {
            eprintln!("[工作区] 放开静态资源目录失败：{e}");
        }
        core.emit_topics(crate::agent::event::TopicsEvent::Refresh);
    }
    // 清空白名单要同步到内存里的那份
    if patch_cleared_roots {
        core.reload_approved_roots();
    }
    if let Some(root) = patch.revoke_root.clone().filter(|r| !r.trim().is_empty()) {
        if let Err(e) = core.revoke_root(std::path::Path::new(&root)) {
            eprintln!("[沙箱] 撤销授权失败：{e}");
        }
    }
    Ok(PublicConfig::from(&cfg))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    /// 传了就是更新，没传就是新建
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    /// `None` 表示保持原有 key 不变；空串表示清空
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_temp")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "yes")]
    pub supports_tools: bool,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 思考强度；不传就沿用原值（新建时用默认值 = 关闭）
    #[serde(default)]
    pub reasoning: Option<crate::config::ReasoningConfig>,
}

fn default_temp() -> f32 {
    0.5
}
fn default_max_tokens() -> u32 {
    8192
}
fn yes() -> bool {
    true
}

#[tauri::command]
pub async fn profile_upsert(state: State<'_, AppState>, input: ProfileInput) -> AppResult<PublicConfig> {
    let core = state.0.clone();
    let name = input.name.trim().to_string();
    let base_url = input.base_url.trim().trim_end_matches('/').to_string();
    let model = input.model.trim().to_string();
    if name.is_empty() || base_url.is_empty() || model.is_empty() {
        return Err(AppError::invalid("名称、接口地址、模型名都不能为空"));
    }

    let cfg = core.update_config(|c| {
        let existing = input.id.as_deref().and_then(|id| c.profiles.iter_mut().find(|p| p.id == id));
        match existing {
            Some(p) => {
                p.name = name.clone();
                p.kind = input.kind;
                p.base_url = base_url.clone();
                p.model = model.clone();
                p.temperature = input.temperature.clamp(0.0, 2.0);
                p.max_tokens = input.max_tokens.clamp(256, 200_000);
                p.supports_tools = input.supports_tools;
                p.headers = input.headers.clone();
                if let Some(r) = input.reasoning {
                    p.reasoning = r;
                }
                if let Some(k) = &input.api_key {
                    p.api_key = k.trim().to_string();
                }
            }
            None => {
                let mut p = ProviderProfile::new(name.clone(), base_url.clone(), model.clone());
                p.kind = input.kind;
                p.temperature = input.temperature.clamp(0.0, 2.0);
                p.max_tokens = input.max_tokens.clamp(256, 200_000);
                p.supports_tools = input.supports_tools;
                p.headers = input.headers.clone();
                if let Some(r) = input.reasoning {
                    p.reasoning = r;
                }
                if let Some(k) = &input.api_key {
                    p.api_key = k.trim().to_string();
                }
                if c.active_profile_id.is_empty() {
                    c.active_profile_id = p.id.clone();
                }
                c.profiles.push(p);
            }
        }
    })?;
    Ok(PublicConfig::from(&cfg))
}

#[tauri::command]
pub async fn profile_delete(state: State<'_, AppState>, id: String) -> AppResult<PublicConfig> {
    let core = state.0.clone();
    let cfg = core.update_config(|c| {
        c.profiles.retain(|p| p.id != id);
        if c.active_profile_id == id {
            c.active_profile_id = c.profiles.first().map(|p| p.id.clone()).unwrap_or_default();
        }
    })?;
    Ok(PublicConfig::from(&cfg))
}

/// 只改思考强度（对话栏里那个芯片用，不必打开设置页）。
#[tauri::command]
pub async fn profile_set_reasoning(
    state: State<'_, AppState>,
    profile_id: String,
    effort: crate::config::ReasoningEffort,
    style: Option<crate::config::ReasoningStyle>,
) -> AppResult<PublicConfig> {
    let core = state.0.clone();
    let cfg = core.update_config(|c| {
        if let Some(p) = c.profiles.iter_mut().find(|p| p.id == profile_id) {
            p.reasoning.effort = effort;
            if let Some(st) = style {
                p.reasoning.style = st;
            }
        }
    })?;
    Ok(PublicConfig::from(&cfg))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTestResult {
    pub ok: bool,
    pub message: String,
    pub latency_ms: u64,
}

/// 连通性测试：发一句「ping」，看能不能流式拿回内容。
#[tauri::command]
pub async fn profile_test(state: State<'_, AppState>, id: String) -> AppResult<ProfileTestResult> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let profile = cfg
        .profile(&id)
        .cloned()
        .ok_or_else(|| AppError::NotFound(format!("找不到模型档案 {id}")))?;
    if profile.api_key.trim().is_empty() && profile.base_url.contains("api.") {
        return Ok(ProfileTestResult {
            ok: false,
            message: "还没有填 API Key".into(),
            latency_ms: 0,
        });
    }

    let provider = provider::build(&profile, core.http.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let req = ChatRequest {
        model: profile.model.clone(),
        system: "你是一个连通性测试端点，收到任何输入都只回复两个字：正常".into(),
        messages: vec![ChatMessage::user("ping")],
        tools: Vec::new(),
        temperature: 0.0,
        max_tokens: 32,
        reasoning: crate::config::ReasoningConfig::default(),
        timeout: Duration::from_secs(45),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let started = Instant::now();
    let task = tokio::spawn(async move { provider.stream(req, tx).await });
    let mut text = String::new();
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Text(t) = ev {
            text.push_str(&t);
        }
    }
    let latency = started.elapsed().as_millis() as u64;
    match task.await {
        Ok(Ok(())) => Ok(ProfileTestResult {
            ok: true,
            message: if text.trim().is_empty() {
                format!("连接成功（{}），但返回内容为空", profile.model)
            } else {
                format!("连接成功：{}", text.trim())
            },
            latency_ms: latency,
        }),
        Ok(Err(e)) => Ok(ProfileTestResult { ok: false, message: e.to_string(), latency_ms: latency }),
        Err(e) => Ok(ProfileTestResult {
            ok: false,
            message: format!("请求任务异常：{e}"),
            latency_ms: latency,
        }),
    }
}

/// 预览当前系统提示词，方便用户/开发者理解 agent 看到了什么。
#[tauri::command]
pub async fn prompt_preview(state: State<'_, AppState>, topic_slug: Option<String>) -> AppResult<String> {
    let core = state.0.clone();
    let cfg = core.config_read();
    let topic = crate::agent::resolve_topic_opt(&core, topic_slug.as_deref());
    let profile = cfg.active_profile().ok();
    Ok(prompt::build_system_prompt(&prompt::PromptInputs {
        config: &cfg,
        topic: topic.as_ref(),
        stage: topic.as_ref().map(|t| t.meta.stage),
        supports_tools: profile.map(|p| p.supports_tools).unwrap_or(false),
        tool_catalog: core.agent.describe_tools(),
        tool_names: core.agent.tool_names(),
        skill_catalog: crate::skills::catalog(&core.skills_for(topic.as_ref()), 120),
    }))
}

/// 工作区信息（给设置页展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub exists: bool,
    pub topic_count: usize,
    pub disk_usage_text: String,
}

#[tauri::command]
pub async fn workspace_info(state: State<'_, AppState>) -> AppResult<WorkspaceInfo> {
    let core = state.0.clone();
    let ws = core.workspace();
    ws.ensure()?;
    let mut bytes = 0u64;
    for f in store::walk_files(&ws.root, 12) {
        bytes += std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
    }
    Ok(WorkspaceInfo {
        root: ws.root.to_string_lossy().to_string(),
        exists: ws.root.is_dir(),
        topic_count: ws.list().map(|l| l.len()).unwrap_or(0),
        disk_usage_text: crate::paths::human_size(bytes),
    })
}

/// 打开工作区/主题目录（交给系统文件管理器）。
#[tauri::command]
pub async fn reveal_in_explorer(state: State<'_, AppState>, topic_slug: Option<String>) -> AppResult<String> {
    let core = state.0.clone();
    let path = match topic_slug.as_deref() {
        Some(slug) if !slug.trim().is_empty() => core.workspace().resolve(slug)?.dir,
        _ => {
            let ws = core.workspace();
            ws.ensure()?;
            ws.root
        }
    };
    open_in_os(&path)?;
    Ok(path.to_string_lossy().to_string())
}

/// 用系统默认程序打开文件（PDF 用外部阅读器看时很有用）。
#[tauri::command]
pub async fn open_with_system(state: State<'_, AppState>, topic_slug: String, path: String) -> AppResult<()> {
    let core = state.0.clone();
    let topic = core.workspace().resolve(&topic_slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &path)?;
    if !abs.exists() {
        return Err(AppError::NotFound(format!("文件不存在：{path}")));
    }
    open_in_os(&abs)
}

#[cfg(target_os = "windows")]
fn open_in_os(path: &std::path::Path) -> AppResult<()> {
    if path.is_dir() {
        std::process::Command::new("explorer")
            .arg(path.as_os_str())
            .spawn()
            .map_err(|e| AppError::other(format!("调用资源管理器失败：{e}")))?;
    } else {
        // start 是 cmd 内建命令，第一个空引号是「窗口标题」占位参数
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()
            .map_err(|e| AppError::other(format!("调用默认程序失败：{e}")))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_in_os(path: &std::path::Path) -> AppResult<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map_err(|e| AppError::other(format!("调用系统打开失败：{e}")))?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_in_os(path: &std::path::Path) -> AppResult<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map_err(|e| AppError::other(format!("调用系统打开失败：{e}")))?;
    Ok(())
}

/// 让前端知道当前 AppCore 的配置对象是不是最新的（调试用）。
#[tauri::command]
pub async fn config_summary(state: State<'_, AppState>) -> AppResult<String> {
    Ok(crate::agent::config_summary(&state.0.config_read()))
}
