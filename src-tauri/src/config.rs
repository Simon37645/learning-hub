//! 应用配置：用户档案、模型接入点（可多套）、agent 权限、工作区位置。
//!
//! 存放位置由 Tauri 决定（Windows 上是 `%APPDATA%\com.learninghub.desktop\config.json`）。
//! API Key 明文存在本机配置里 —— 这是本地个人应用的取舍，别把这份文件提交到公开仓库。

use crate::error::{AppError, AppResult};
use crate::store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 3;

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

/// 思考强度。对应各家「让模型想多久」的参数：
/// OpenAI 系的 `reasoning_effort`、Anthropic 的 `thinking.budget_tokens`。
///
/// 默认关闭：这个字段不是所有服务商都认，只有在用户明确打开后才往请求里加，
/// 免得平白把请求写坏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Off,
    Low,
    Medium,
    High,
    Max,
}

impl ReasoningEffort {
    pub fn label(self) -> &'static str {
        match self {
            ReasoningEffort::Off => "关闭",
            ReasoningEffort::Low => "低",
            ReasoningEffort::Medium => "中",
            ReasoningEffort::High => "高",
            ReasoningEffort::Max => "最高",
        }
    }

    pub fn is_on(self) -> bool {
        !matches!(self, ReasoningEffort::Off)
    }

    /// OpenAI 系的取值（只有 low/medium/high 三档，max 归到 high）
    pub fn openai_value(self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            _ => "high",
        }
    }

    /// Anthropic 的思考预算（token）。官方要求至少 1024，且必须小于 max_tokens。
    pub fn thinking_budget(self) -> u32 {
        match self {
            ReasoningEffort::Off => 0,
            ReasoningEffort::Low => 1024,
            ReasoningEffort::Medium => 4096,
            ReasoningEffort::High => 12_000,
            ReasoningEffort::Max => 32_000,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "关闭" | "none" => Some(ReasoningEffort::Off),
            "low" | "低" => Some(ReasoningEffort::Low),
            "medium" | "mid" | "中" => Some(ReasoningEffort::Medium),
            "high" | "高" => Some(ReasoningEffort::High),
            "max" | "最高" => Some(ReasoningEffort::Max),
            _ => None,
        }
    }
}

/// 用哪种写法把「思考强度」发出去。
///
/// 各家扩展不统一：OpenAI 系认 `reasoning_effort`，通义千问认 `enable_thinking`，
/// Anthropic 认 `thinking`。默认按协议自动选，认不出来的服务商可以关掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStyle {
    /// 按协议自动：OpenAI 兼容 → reasoning_effort；Anthropic → thinking
    #[default]
    Auto,
    /// 只发 OpenAI 的 reasoning_effort
    OpenaiEffort,
    /// 只发 Anthropic 的 thinking
    AnthropicThinking,
    /// 发 enable_thinking（通义千问等）
    QwenThinking,
    /// 什么都不发
    None,
}

impl ReasoningStyle {
    pub fn label(self) -> &'static str {
        match self {
            ReasoningStyle::Auto => "自动",
            ReasoningStyle::OpenaiEffort => "reasoning_effort",
            ReasoningStyle::AnthropicThinking => "thinking",
            ReasoningStyle::QwenThinking => "enable_thinking",
            ReasoningStyle::None => "不发",
        }
    }
}

/// 一个档案的思考强度设置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningConfig {
    #[serde(default)]
    pub effort: ReasoningEffort,
    #[serde(default)]
    pub style: ReasoningStyle,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            effort: ReasoningEffort::Off,
            style: ReasoningStyle::Auto,
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
    /// 是否支持图片输入（多模态）。默认开——绝大多数现代模型都认，
    /// 只有当用户明确知道自己在用一个纯文本模型时才该关掉。
    /// 关掉后不是「拒绝收图」，而是**把图降级成一行文字**再发：见 provider::resolve_image。
    #[serde(default = "yes")]
    pub supports_vision: bool,
    /// 思考强度（默认关闭）
    #[serde(default)]
    pub reasoning: ReasoningConfig,
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
            supports_vision: true,
            reasoning: ReasoningConfig::default(),
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
    /// 模型连接中断后的自动重连上限。0 = 关闭，失败就直接报错。
    /// 网络不稳时（尤其长流式输出）一断就丢整轮太浪费，重试成本只是再发一次请求。
    #[serde(default = "default_reconnect_max")]
    pub reconnect_max_attempts: u32,
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
    /// 长期记忆：把「用户是什么样的人、以后要注意什么」记下来，每轮注入提示词。
    /// 关掉后既不注入也不给 agent 记忆工具（面板里还能看和编辑已有的记忆）
    #[serde(default = "yes")]
    pub memory_enabled: bool,
}

fn default_max_iterations() -> u32 {
    24
}
fn default_timeout() -> u64 {
    180
}
fn default_reconnect_max() -> u32 {
    10
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
            reconnect_max_attempts: default_reconnect_max(),
            context_budget_chars: default_context_budget(),
            sandbox: true,
            approved_roots: Vec::new(),
            skills_enabled: true,
            extra_skill_dirs: Vec::new(),
            mcp_servers: Vec::new(),
            disabled_skills: Vec::new(),
            memory_enabled: true,
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

/// 远程监听在哪儿。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteBind {
    /// 监听所有网卡：同一个 WiFi 下的手机 / 平板直接能连（默认，扫码就能用）
    #[default]
    Lan,
    /// 只听本机：给「前面有反代或隧道」的用法（端口不对外暴露）
    Local,
}

impl RemoteBind {
    pub fn addr(self) -> &'static str {
        match self {
            RemoteBind::Lan => "0.0.0.0",
            RemoteBind::Local => "127.0.0.1",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RemoteBind::Lan => "局域网",
            RemoteBind::Local => "仅本机",
        }
    }
}

/// 移动端远程控制。
///
/// 这是一条**能从别的设备操作本机 agent** 的通道（手机上发的消息真的由这台电脑
/// 执行：读写文件、跑工具都在本机），所以默认关闭，打开后才监听端口，且每个
/// `/api/*` 请求都要带令牌。
///
/// 三种可达性各管一段，互不依赖：
/// - 同一个 WiFi：直接连 `http://<局域网 IP>:<port>`
/// - 自己有公网服务器：填 `relay`，应用主动连出去建隧道（不用在路由器上做端口映射）
/// - 前面已有反代 / 隧道（nginx、cloudflared、frp）：填 `public_url`，
///   二维码与「复制链接」就用那个域名（端口仍听本机）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    #[serde(default)]
    pub bind: RemoteBind,
    /// 访问令牌，拼在链接里（`?t=`）。空串时首次启动自动生成
    #[serde(default)]
    pub token: String,
    /// 对外地址，例如 `https://hub.example.com` 或 `http://1.2.3.4:8787`。
    /// 填了它，二维码与「复制链接」用它——隧道/反代对外是什么地址只有用户知道
    #[serde(default)]
    pub public_url: String,
    /// 中继服务器地址（`host:port`）。填了就主动连出去建隧道，
    /// 让手机从公网访问这台机器，而不需要在路由器上做端口映射
    #[serde(default)]
    pub relay: String,
    /// 中继口令（要和服务器上 relay.mjs 的 `--token` 一致）。
    /// 刻意与访问令牌分开：访问令牌会出现在 URL 与浏览器历史里，
    /// 拿它当隧道口令等于把「连进隧道」的资格也一起给了出去
    #[serde(default)]
    pub relay_token: String,
    /// 中继服务器的 SSH 端口 / 用户名（**只记这两个，不记密码**）。
    /// 一键安装要重装时不用再敲一遍；密码只在点安装那一次用，不落盘。
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// 服务器公钥指纹（首次连接记住，之后变了就拒绝——见 remote::deploy 的 TOFU 说明）
    #[serde(default)]
    pub host_key: String,
}

fn default_ssh_port() -> u16 {
    22
}

/// 默认端口：**刻意选一个不常见的**。
///
/// 8787 是 RStudio Server 的默认端口（用户点名提过），5000/8000/8080/3000 这类
/// 更是满地都是。这个号段在 IANA 的私有/动态范围（49152-65535）里，
/// 没有注册服务占用，也不会跟常见的开发服务器撞上。
fn default_remote_port() -> u16 {
    54713
}

/// 旧默认端口（0.5.0-beta.1 用过）。用户没改过的话迁移到新默认值——
/// 那个端口会和本机的 RStudio Server 撞车。
pub const LEGACY_REMOTE_PORT: u16 = 8787;

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_remote_port(),
            bind: RemoteBind::default(),
            token: String::new(),
            public_url: String::new(),
            relay: String::new(),
            relay_token: String::new(),
            ssh_port: default_ssh_port(),
            ssh_user: String::new(),
            host_key: String::new(),
        }
    }
}

/// 中继地址（`host:port`）→ 公网入口地址（`http://host:port`）。
///
/// 手机要访问的就是中继那个端口：它既是隧道的入口，也是网页的入口
/// （中继按首行区分「电脑的握手」与「来访的浏览器」）。所以用户装好中继之后，
/// 二维码可以直接用它，不必再填「对外地址」——那一步是留给有反代的人的。
pub fn relay_base(relay: &str) -> Option<String> {
    let t = relay.trim();
    if t.is_empty() {
        return None;
    }
    // 允许用户填 host / host:port / 带协议的地址
    if t.starts_with("http://") || t.starts_with("https://") {
        return Some(t.trim_end_matches('/').to_string());
    }
    let (host, port) = match t.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (t, None),
    };
    Some(match port {
        Some(p) => format!("http://{host}:{p}"),
        None => format!("http://{host}:{}", default_remote_port()),
    })
}

impl RemoteConfig {
    /// 新令牌：32 位十六进制（uuid v4 去掉横线）。够长、好复制、不用引 base64 依赖。
    pub fn fresh_token() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    /// 缺令牌就补上（首次打开、或用户手动清空后）。
    pub fn ensure_tokens(&mut self) -> bool {
        let mut changed = false;
        if self.token.trim().is_empty() {
            self.token = Self::fresh_token();
            changed = true;
        }
        if self.relay_token.trim().is_empty() {
            self.relay_token = Self::fresh_token();
            changed = true;
        }
        changed
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
    /// 用户自定义主题的 id（`<工作区>/.hub/themes/<id>.json` 或用户目录同名文件）。
    /// `None` = 用内置的明亮/深色配色。找不到这个 id 时也按内置配色走——
    /// 主题文件被删掉不该让界面变成一片没有颜色的白板。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_theme: Option<String>,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self { theme: ThemeMode::System, custom_theme: None }
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
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub remote: RemoteConfig,
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
            appearance: AppearanceConfig::default(),
            remote: RemoteConfig::default(),
            last_topic: None,
            recent_profiles: Vec::new(),
        }
    }

    pub fn load(path: &Path, default_workspace: PathBuf) -> AppResult<Self> {
        match store::read_json_opt::<AppConfig>(path)? {
            Some(mut cfg) => {
                let migrated = cfg.migrate();
                if cfg.profiles.is_empty() {
                    cfg.profiles = Self::bootstrap(default_workspace.clone()).profiles;
                }
                if cfg.active_profile_id.is_empty() {
                    cfg.active_profile_id = cfg.profiles[0].id.clone();
                }
                // 迁移里补出来的东西**要落盘**：最要紧的是远程控制的令牌——
                // 它只活在内存里的话，每次重启都换一套，用户刚扫的二维码就失效了。
                if migrated {
                    if let Err(e) = store::write_json(path, &cfg) {
                        eprintln!("[配置] 迁移后写盘失败（不影响本次运行）：{e}");
                    }
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
    /// 返回「有没有改过」——调用方据此决定要不要写回磁盘。
    fn migrate(&mut self) -> bool {
        let before = (self.version, self.user_name.clone(), self.remote.token.clone());
        if self.version < CONFIG_VERSION {
            self.version = CONFIG_VERSION;
        }
        if self.user_name.trim().is_empty() {
            self.user_name = default_user_name();
        }
        // 令牌在用户手动清空 / 老配置里是空的，读配置时补上
        self.remote.ensure_tokens();
        // 8787 是 RStudio Server 的端口，用户没改过的话换掉（改过的保持不动）
        if self.remote.port == LEGACY_REMOTE_PORT {
            self.remote.port = default_remote_port();
        }
        (self.version, self.user_name.clone(), self.remote.token.clone()) != before
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
    pub appearance: AppearanceConfig,
    /// 远程控制的配置（含令牌：这是本机界面，用户要靠它拼链接与二维码）
    pub remote: RemoteConfig,
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
    pub supports_vision: bool,
    pub reasoning: ReasoningConfig,
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
                    supports_vision: p.supports_vision,
                    reasoning: p.reasoning,
                    has_api_key: !p.api_key.trim().is_empty(),
                    key_hint: p.key_hint(),
                })
                .collect(),
            agent: c.agent.clone(),
            viewer: c.viewer.clone(),
            appearance: c.appearance.clone(),
            remote: c.remote.clone(),
            last_topic: c.last_topic.clone(),
        }
    }
}
