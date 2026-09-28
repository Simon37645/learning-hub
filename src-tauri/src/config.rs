//! 应用配置：用户档案、模型接入点（可多套）、agent 权限、工作区位置。
//!
//! 存放位置由 Tauri 决定（Windows 上是 `%APPDATA%\com.learninghub.desktop\config.json`）。
//! API Key 明文存在本机配置里 —— 这是本地个人应用的取舍，别把这份文件提交到公开仓库。

use crate::error::{AppError, AppResult};
use crate::store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 1;

/// 模型服务商协议族。差异只在「请求/响应长什么样」，其余逻辑共用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI `/chat/completions` 兼容（DeepSeek、Moonshot、通义、硅基流动、Ollama、vLLM…）
    #[default]
    OpenAi,
    /// Anthropic `/v1/messages`
    Anthropic,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI 兼容",
            ProviderKind::Anthropic => "Anthropic",
        }
    }
}

/// 一套接入配置 = 一个「模型档案」，界面上就是模型选择器里的一项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    /// 显示名，例如 "DeepSeek 官方"
    pub name: String,
    #[serde(default)]
    pub kind: ProviderKind,
    /// 例如 https://api.deepseek.com/v1 （不需要带 /chat/completions）
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// 额外请求头（自建网关常用）
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 是否支持函数调用（不支持时 agent 自动降级为「纯文本 + 指令」模式）
    #[serde(default = "yes")]
    pub supports_tools: bool,
}

fn default_temperature() -> f32 {
    0.5
}
fn default_max_tokens() -> u32 {
    8192
}
fn yes() -> bool {
    true
}

impl ProviderProfile {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            kind: ProviderKind::OpenAi,
            base_url: base_url.into(),
            api_key: String::new(),
            model: model.into(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            headers: BTreeMap::new(),
            supports_tools: true,
        }
    }

    /// 拼出真正要 POST 的地址：容忍用户填 /v1、/v1/、甚至完整的 /chat/completions。
    pub fn chat_url(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") || base.ends_with("/messages") {
            return base.to_string();
        }
        match self.kind {
            ProviderKind::OpenAi => format!("{base}/chat/completions"),
            ProviderKind::Anthropic => format!("{base}/messages"),
        }
    }

    /// 给界面看的脱敏信息。
    pub fn key_hint(&self) -> String {
        let k = self.api_key.trim();
        if k.is_empty() {
            return String::new();
        }
        if k.chars().count() <= 8 {
            return "••••".into();
        }
        let head: String = k.chars().take(4).collect();
        let tail: String = k.chars().skip(k.chars().count() - 4).collect();
        format!("{head}…{tail}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// 读操作放行，写/删都要点头（默认）
    #[default]
    Ask,
    /// 读与写放行，只有删除/覆盖才打断
    AutoEdit,
    /// 完全访问：不再打断
    Full,
}

impl PermissionMode {
    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Ask => "每次确认",
            PermissionMode::AutoEdit => "自动编辑",
            PermissionMode::Full => "完全访问",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" | "每次确认" => Some(PermissionMode::Ask),
            "auto_edit" | "自动编辑" => Some(PermissionMode::AutoEdit),
            "full" | "完全访问" => Some(PermissionMode::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    #[serde(default)]
    pub permission_mode: PermissionMode,
    /// 单轮对话里最多允许的「模型↔工具」往返次数
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// 追加到系统提示词末尾的自定义指令
    #[serde(default)]
    pub system_prompt_extra: String,
    /// 是否允许 agent 联网抓取网页
    #[serde(default = "yes")]
    pub allow_web: bool,
    /// 请求超时（秒）
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
    /// 每次启动/切换主题时注入的上下文预算（字符数），防止把主题读爆
    #[serde(default = "default_context_budget")]
    pub context_budget_chars: usize,
    /// 沙箱：agent 默认只能碰工作区里的文件；要出去必须向你申请
    #[serde(default = "yes")]
    pub sandbox: bool,
    /// 你批准过的「工作区之外」的目录。批准一次，整个目录长期有效
    #[serde(default)]
    pub approved_roots: Vec<String>,
    /// 是否加载技能（SKILL.md）
    #[serde(default = "yes")]
    pub skills_enabled: bool,
    /// 额外的技能目录（除了工作区 .hub/skills 与用户目录）
    #[serde(default)]
    pub extra_skill_dirs: Vec<String>,
    /// MCP 服务器列表（全局）
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
    /// 全局层面禁用的技能（按 id）
    #[serde(default)]
    pub disabled_skills: Vec<String>,
}

fn default_max_iterations() -> u32 {
    24
}
fn default_timeout() -> u64 {
    180
}
fn default_context_budget() -> usize {
    24_000
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            permission_mode: PermissionMode::default(),
            max_iterations: default_max_iterations(),
            system_prompt_extra: String::new(),
            allow_web: true,
            request_timeout_secs: default_timeout(),
            context_budget_chars: default_context_budget(),
            sandbox: true,
            approved_roots: Vec::new(),
            skills_enabled: true,
            extra_skill_dirs: Vec::new(),
            mcp_servers: Vec::new(),
            disabled_skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerConfig {
    /// 内置浏览器新标签页的默认主页
    #[serde(default = "default_home")]
    pub home_url: String,
    /// agent 联网搜索使用的引擎（目前支持 duckduckgo / bing）
    #[serde(default = "default_engine")]
    pub search_engine: String,
}

fn default_home() -> String {
    "https://www.bing.com".into()
}
fn default_engine() -> String {
    "duckduckgo".into()
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            home_url: default_home(),
            search_engine: default_engine(),
        }
    }
}

/// 主题模式。默认跟随系统，但用户可以强制明亮——
/// 系统的深色模式有时候并不适合阅读长文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "跟随系统",
            ThemeMode::Light => "明亮",
            ThemeMode::Dark => "深色",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceConfig {
    #[serde(default)]
    pub theme: ThemeMode,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self { theme: ThemeMode::System }
    }
}

/// AnkiConnect 相关配置。Anki 桌面版装上 AnkiConnect 插件后，
/// 我们就能把卡片直接灌进牌组，把调度交给 Anki 自己管。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiConfig {
    /// AnkiConnect 地址
    #[serde(default = "default_anki_url")]
    pub url: String,
    /// 同步过去的牌组名前缀，留空则直接用主题名
    #[serde(default)]
    pub deck_prefix: String,
}

fn default_anki_url() -> String {
    crate::anki::DEFAULT_URL.to_string()
}

impl Default for AnkiConfig {
    fn default() -> Self {
        Self {
            url: default_anki_url(),
            deck_prefix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    /// 主界面问候语里的名字
    #[serde(default = "default_user_name")]
    pub user_name: String,
    /// 主题目录的根
    pub workspace_root: PathBuf,
    /// 当前使用的模型档案
    pub active_profile_id: String,
    pub profiles: Vec<ProviderProfile>,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub viewer: ViewerConfig,
    #[serde(default)]
    pub anki: AnkiConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    /// 上次打开的主题（下次启动直接回到那里）
    #[serde(default)]
    pub last_topic: Option<String>,
    /// 最近用过的模型档案，界面上做「最近使用」排序
    #[serde(default)]
    pub recent_profiles: Vec<String>,
}

fn default_user_name() -> String {
    "旅行者".into()
}

impl AppConfig {
    /// 内置的起步配置：默认走 DeepSeek（国内可直连、便宜、支持工具调用）。
    pub fn bootstrap(workspace_root: PathBuf) -> Self {
        let mut deepseek = ProviderProfile::new("DeepSeek", "https://api.deepseek.com/v1", "deepseek-chat");
        deepseek.supports_tools = true;

        let mut anthropic = ProviderProfile::new("Claude", "https://api.anthropic.com/v1", "claude-sonnet-4-5");
        anthropic.kind = ProviderKind::Anthropic;

        let mut ollama = ProviderProfile::new("Ollama（本地）", "http://127.0.0.1:11434/v1", "qwen3:8b");
        ollama.supports_tools = false;

        let active = deepseek.id.clone();
        Self {
            version: CONFIG_VERSION,
            user_name: default_user_name(),
            workspace_root,
            active_profile_id: active,
            profiles: vec![deepseek, anthropic, ollama],
            agent: AgentConfig::default(),
            viewer: ViewerConfig::default(),
            anki: AnkiConfig::default(),
            appearance: AppearanceConfig::default(),
            last_topic: None,
            recent_profiles: Vec::new(),
        }
    }

    pub fn load(path: &Path, default_workspace: PathBuf) -> AppResult<Self> {
        match store::read_json_opt::<AppConfig>(path)? {
            Some(mut cfg) => {
                cfg.migrate();
                if cfg.profiles.is_empty() {
                    cfg.profiles = Self::bootstrap(default_workspace.clone()).profiles;
                }
                if cfg.active_profile_id.is_empty() {
                    cfg.active_profile_id = cfg.profiles[0].id.clone();
                }
                Ok(cfg)
            }
            None => {
                let cfg = Self::bootstrap(default_workspace);
                store::write_json(path, &cfg)?;
                Ok(cfg)
            }
        }
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        store::write_json(path, self)
    }

    /// 旧版本配置补齐字段。每次加字段都可以在这里写迁移。
    fn migrate(&mut self) {
        if self.version < CONFIG_VERSION {
            self.version = CONFIG_VERSION;
        }
        if self.user_name.trim().is_empty() {
            self.user_name = default_user_name();
        }
    }

    pub fn profile(&self, id: &str) -> Option<&ProviderProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> AppResult<&ProviderProfile> {
        self.profile(&self.active_profile_id)
            .or_else(|| self.profiles.first())
            .ok_or_else(|| AppError::invalid("还没有配置任何模型，请到「设置」里添加"))
    }

    pub fn workspace(&self) -> crate::domain::topic::Workspace {
        crate::domain::topic::Workspace::new(self.workspace_root.clone())
    }
}

/// 发给前端的配置视图：**不含 api key**，只给「有没有配」和脱敏提示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub version: u32,
    pub user_name: String,
    pub workspace_root: String,
    pub active_profile_id: String,
    pub profiles: Vec<PublicProfile>,
    pub agent: AgentConfig,
    pub viewer: ViewerConfig,
    pub anki: AnkiConfig,
    pub appearance: AppearanceConfig,
    pub last_topic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicProfile {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub kind_label: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub supports_tools: bool,
    pub has_api_key: bool,
    pub key_hint: String,
}

impl From<&AppConfig> for PublicConfig {
    fn from(c: &AppConfig) -> Self {
        Self {
            version: c.version,
            user_name: c.user_name.clone(),
            workspace_root: c.workspace_root.to_string_lossy().to_string(),
            active_profile_id: c.active_profile_id.clone(),
            profiles: c
                .profiles
                .iter()
                .map(|p| PublicProfile {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    kind: p.kind,
                    kind_label: p.kind.label().to_string(),
                    base_url: p.base_url.clone(),
                    model: p.model.clone(),
                    temperature: p.temperature,
                    max_tokens: p.max_tokens,
                    supports_tools: p.supports_tools,
                    has_api_key: !p.api_key.trim().is_empty(),
                    key_hint: p.key_hint(),
                })
                .collect(),
            agent: c.agent.clone(),
            viewer: c.viewer.clone(),
            anki: c.anki.clone(),
            appearance: c.appearance.clone(),
            last_topic: c.last_topic.clone(),
        }
    }
}
