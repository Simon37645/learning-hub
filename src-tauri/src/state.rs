//! 应用运行时状态：命令层、agent 工具层、后台任务都通过它共享配置与服务。

use crate::agent::event::{AgentEvent, TopicsEvent, ViewerEvent, EVENT_AGENT, EVENT_TOAST, EVENT_TOPICS, EVENT_VIEWER};
use crate::agent::AgentService;
use crate::config::AppConfig;
use crate::domain::session::StudySession;
use crate::domain::topic::{Topic, Workspace};
use crate::error::{AppError, AppResult};
use crate::store;
use crate::viewer::{ViewerService, ViewerSnapshot};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppCore {
    pub app: AppHandle,
    pub config_path: PathBuf,
    config: RwLock<AppConfig>,
    /// 复用的 HTTP 客户端（连接池共享）
    pub http: reqwest::Client,
    pub agent: Arc<AgentService>,
    pub viewer: Arc<ViewerService>,
    /// 当前进行中的学习会话（同一时间只允许一个）
    session: RwLock<Option<StudySession>>,
    /// 沙箱白名单：工作区之外被用户批准过的目录
    approved_roots: RwLock<Vec<PathBuf>>,
    /// 已发现的技能（目录变了就 reload_skills）
    skills_cache: RwLock<Vec<crate::skills::Skill>>,
    /// MCP 服务器的连接状态
    mcp_state: RwLock<Vec<McpEntry>>,
}

/// 一个 MCP 服务器的运行状态。
pub struct McpEntry {
    pub config: crate::mcp::McpServerConfig,
    pub client: Option<Arc<crate::mcp::McpClient>>,
    pub error: Option<String>,
}

/// 给前端与工具看的 MCP 状态视图。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusEntry {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub connected: bool,
    pub server_info: String,
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

fn entry_to_status(entry: &McpEntry) -> McpStatusEntry {
    McpStatusEntry {
        name: entry.config.name.clone(),
        command: format!("{} {}", entry.config.command, entry.config.args.join(" ")).trim().to_string(),
        args: entry.config.args.clone(),
        enabled: entry.config.enabled,
        connected: entry.client.is_some(),
        server_info: entry.client.as_ref().map(|c| c.server_info()).unwrap_or_default(),
        tool_count: entry.client.as_ref().map(|c| c.tools.len()).unwrap_or(0),
        tools: entry.client.as_ref().map(|c| c.tools.iter().map(|t| t.name.clone()).collect()).unwrap_or_default(),
        error: entry.error.clone(),
    }
}

impl AppCore {
    pub fn new(
        app: AppHandle,
        config_path: PathBuf,
        config: AppConfig,
        http: reqwest::Client,
    ) -> Arc<Self> {
        let approved = config
            .agent
            .approved_roots
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        Arc::new(Self {
            app,
            config_path,
            config: RwLock::new(config),
            http,
            agent: Arc::new(AgentService::new()),
            viewer: Arc::new(ViewerService::new()),
            session: RwLock::new(None),
            approved_roots: RwLock::new(approved),
            skills_cache: RwLock::new(Vec::new()),
            mcp_state: RwLock::new(Vec::new()),
        })
    }

    /// 配置快照（返回值而不是 guard，避免跨 await 持锁）。
    pub fn config_read(&self) -> AppConfig {
        self.config.read().clone()
    }

    pub fn workspace(&self) -> Workspace {
        Workspace::new(self.config.read().workspace_root.clone())
    }

    /// 改配置并落盘。
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> AppResult<AppConfig> {
        let mut guard = self.config.write();
        f(&mut guard);
        guard.save(&self.config_path)?;
        Ok(guard.clone())
    }

    // ---------------------------------------------------------- 事件

    pub fn emit_agent(&self, ev: &AgentEvent) {
        let _ = self.app.emit(EVENT_AGENT, ev);
    }

    pub fn emit_viewer(&self, ev: ViewerEvent) {
        let _ = self.app.emit(EVENT_VIEWER, ev);
    }

    /// 查看器状态有变化（开/关/切标签）时，把全量状态推给前端。
    pub async fn emit_viewer_sync(&self) {
        let snapshot: ViewerSnapshot = self.viewer.snapshot().await;
        self.emit_viewer(ViewerEvent::Sync { snapshot });
    }

    pub fn emit_topics(&self, ev: TopicsEvent) {
        let _ = self.app.emit(EVENT_TOPICS, &ev);
    }

    pub fn emit_topics_created(&self, topic: &Topic) {
        self.emit_topics(TopicsEvent::Created {
            slug: topic.slug(),
            name: topic.meta.name.clone(),
        });
    }

    pub fn emit_topics_updated(&self, topic: &Topic) {
        self.emit_topics(TopicsEvent::Updated { slug: topic.slug() });
    }

    pub fn toast(&self, level: &str, message: impl Into<String>) {
        let _ = self.app.emit(
            EVENT_TOAST,
            serde_json::json!({ "level": level, "message": message.into() }),
        );
    }

    /// 给窗口设置标题（带当前主题名）。
    pub fn set_window_title(&self, suffix: Option<&str>) {
        if let Some(win) = self.app.get_webview_window("main") {
            let title = match suffix {
                Some(s) if !s.is_empty() => format!("{s} — 学习中枢"),
                _ => "学习中枢".to_string(),
            };
            let _ = win.set_title(&title);
        }
    }

    // ---------------------------------------------------------- 技能与 MCP

    /// 当前可见的技能清单（启动时与「重扫」后刷新）。
    pub fn skills(&self) -> Vec<crate::skills::Skill> {
        self.skills_cache.read().clone()
    }

    /// 重新扫描技能目录（只扫全局来源，用于侧栏的技能清单）。
    pub fn reload_skills(&self) -> usize {
        let cfg = self.config_read();
        if !cfg.agent.skills_enabled {
            *self.skills_cache.write() = Vec::new();
            return 0;
        }
        let found = crate::skills::discover(Some(&cfg.workspace_root), None, &cfg.agent.extra_skill_dirs);
        let n = found.len();
        *self.skills_cache.write() = found;
        n
    }

    /// **在某个主题下真正生效**的技能：全局 + 该主题私有，再剪掉两级禁用。
    pub fn skills_for(&self, topic: Option<&crate::domain::topic::Topic>) -> Vec<crate::skills::Skill> {
        let cfg = self.config_read();
        if !cfg.agent.skills_enabled {
            return Vec::new();
        }
        let all = crate::skills::discover(
            Some(&cfg.workspace_root),
            topic.map(|t| t.dir.as_path()),
            &cfg.agent.extra_skill_dirs,
        );
        all.into_iter()
            .filter(|s| !cfg.agent.disabled_skills.iter().any(|d| d == &s.id))
            .filter(|s| {
                topic
                    .map(|t| t.meta.tools.skill_enabled(&s.id))
                    .unwrap_or(true)
            })
            .collect()
    }

    /// 需要连接的 MCP 服务器：全局的 + 各主题私有的（同名去重，全局优先）。
    ///
    /// 主题级的「禁用」不影响连接，只影响该主题下工具是否暴露——
    /// 这样切换主题不用重连子进程。
    pub fn mcp_servers_to_connect(&self) -> Vec<crate::mcp::McpServerConfig> {
        let cfg = self.config_read();
        let mut out: Vec<crate::mcp::McpServerConfig> = cfg.agent.mcp_servers.clone();
        if let Ok(topics) = cfg.workspace().list() {
            for summary in topics {
                let Ok(topic) = cfg.workspace().load(&summary.slug) else {
                    continue;
                };
                for extra in &topic.meta.tools.extra_mcp {
                    if !out.iter().any(|s| s.name == extra.name) {
                        out.push(extra.clone());
                    }
                }
            }
        }
        out
    }

    /// 某个 MCP 服务器在指定主题下是否可用。
    pub fn mcp_enabled_for_topic(
        &self,
        name: &str,
        topic: Option<&crate::domain::topic::Topic>,
    ) -> bool {
        let cfg = self.config_read();
        // 全局层面停用的，在哪个主题里都不生效
        if let Some(s) = cfg.agent.mcp_servers.iter().find(|s| s.name == name) {
            if !s.enabled {
                return false;
            }
        }
        topic
            .map(|t| t.meta.tools.mcp_enabled(name))
            .unwrap_or(true)
    }

    /// 工具名（mcp__服务器__工具）在当前主题下是否该暴露给模型。
    pub fn mcp_tool_allowed(
        &self,
        tool_name: &str,
        topic: Option<&crate::domain::topic::Topic>,
    ) -> bool {
        match crate::agent::tools::mcp::split_prefixed(tool_name) {
            Some((server, _)) => self.mcp_enabled_for_topic(&server, topic),
            None => true,
        }
    }

    /// 启动（或重连）所有启用的 MCP 服务器，并把它们的工具登记进 agent。
    ///
    /// 连接失败不会中断其它服务器——一个坏插件不该拖垮整个工具集。
    pub async fn mcp_reload(&self) -> Vec<McpStatusEntry> {
        // 先关掉旧的
        let old: Vec<McpEntry> = std::mem::take(&mut *self.mcp_state.write());
        for entry in old {
            if let Some(c) = entry.client {
                c.shutdown().await;
            }
        }

        // 清掉上一轮登记的 mcp__ 工具，避免重连后越堆越多
        self.agent.unregister_mcp_tools();

        let servers = self.mcp_servers_to_connect();
        let mut entries = Vec::new();
        let mut new_tools: Vec<Arc<dyn crate::agent::registry::Tool>> = Vec::new();

        for cfg in servers {
            if !cfg.enabled {
                entries.push(McpEntry {
                    config: cfg,
                    client: None,
                    error: Some("已禁用".into()),
                });
                continue;
            }
            match crate::mcp::McpClient::connect(&cfg).await {
                Ok(client) => {
                    match client.list_tools().await {
                        Ok(tools) => {
                            for t in tools {
                                new_tools.push(Arc::new(crate::agent::tools::mcp::McpTool::new(
                                    client.clone(),
                                    t,
                                )));
                            }
                        }
                        Err(e) => eprintln!("[mcp] {} 拉取工具列表失败：{e}", cfg.name),
                    }
                    entries.push(McpEntry {
                        config: cfg.clone(),
                        client: Some(client),
                        error: None,
                    });
                }
                Err(e) => {
                    eprintln!("[mcp] {} 连接失败：{e}", cfg.name);
                    entries.push(McpEntry {
                        config: cfg.clone(),
                        client: None,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        let registered = self.agent.register_tools(new_tools);
        let status: Vec<McpStatusEntry> = entries.iter().map(entry_to_status).collect();
        *self.mcp_state.write() = entries;
        eprintln!("[mcp] 已登记 {registered} 个外部工具");
        status
    }

    pub async fn mcp_status(&self) -> Vec<McpStatusEntry> {
        self.mcp_state.read().iter().map(entry_to_status).collect()
    }

    // ---------------------------------------------------------- 沙箱

    /// 把某个目录加入 `asset://` 协议白名单（笔记里的本地图片靠它显示）。
    ///
    /// 白名单在运行时按工作区动态放开，而不是在 tauri.conf.json 里写 `**`，
    /// 这样「只能访问工作区」的约定在界面层同样成立。
    pub fn allow_asset_dir(&self, dir: &std::path::Path) -> AppResult<()> {
        self.app
            .asset_protocol_scope()
            .allow_directory(dir, true)
            .map_err(|e| AppError::other(format!("放开静态资源目录失败：{e}")))
    }

    pub fn sandbox_enabled(&self) -> bool {
        self.config.read().agent.sandbox
    }

    /// 工作区之内（所有主题都在里面）不需要任何申请。
    pub fn is_inside_workspace(&self, path: &std::path::Path) -> bool {
        crate::paths::is_within(&self.config.read().workspace_root, path)
    }

    /// 是否已被用户授权（批准过的目录及其子路径）。
    pub fn is_approved(&self, path: &std::path::Path) -> bool {
        self.approved_roots
            .read()
            .iter()
            .any(|root| crate::paths::is_within(root, path))
    }

    /// 这一次访问是否需要向用户申请。
    pub fn needs_escalation(&self, path: &std::path::Path) -> bool {
        self.sandbox_enabled() && !self.is_inside_workspace(path) && !self.is_approved(path)
    }

    /// 批准一个目录（幂等），并写进配置长期生效。
    pub fn approve_root(&self, path: &std::path::Path) -> AppResult<PathBuf> {
        let root = crate::paths::grant_root(path);
        {
            let mut guard = self.approved_roots.write();
            if !guard.iter().any(|r| r == &root) {
                guard.push(root.clone());
            }
        }
        let roots: Vec<String> = self
            .approved_roots
            .read()
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        self.update_config(|c| c.agent.approved_roots = roots)?;
        Ok(root)
    }

    pub fn revoke_root(&self, path: &std::path::Path) -> AppResult<()> {
        let target = crate::paths::grant_root(path);
        self.approved_roots.write().retain(|r| r != &target);
        let roots: Vec<String> = self
            .approved_roots
            .read()
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        self.update_config(|c| c.agent.approved_roots = roots)?;
        Ok(())
    }

    /// 配置里被清空后同步到内存白名单。
    pub fn reload_approved_roots(&self) {
        let roots = self
            .config_read()
            .agent
            .approved_roots
            .iter()
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        *self.approved_roots.write() = roots;
    }

    pub fn approved_roots(&self) -> Vec<String> {
        self.approved_roots
            .read()
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    // ---------------------------------------------------------- 学习会话

    pub fn current_session(&self) -> Option<StudySession> {
        self.session.read().clone()
    }

    pub fn set_current_session(&self, s: Option<StudySession>) {
        *self.session.write() = s;
        self.persist_session();
    }

    pub fn bump_session_counters(&self, notes: u32, cards: u32) {
        {
            let mut guard = self.session.write();
            if let Some(s) = guard.as_mut() {
                s.notes_created += notes;
                s.cards_created += cards;
            }
        }
        self.persist_session();
    }

    /// 把当前会话写回 `sessions/<id>.json`，保证异常退出也不丢。
    fn persist_session(&self) {
        let Some(s) = self.session.read().clone() else { return };
        let Ok(topic) = self.workspace().resolve(&s.topic_slug) else { return };
        let dir = topic.sessions_dir();
        if crate::paths::ensure_dir(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", s.id));
        if let Err(e) = store::write_json(&path, &s) {
            eprintln!("[session] 保存失败：{e}");
        }
    }
}

/// Tauri 的托管状态。命令里写 `State<'_, AppState>` 然后 `state.0.clone()`。
pub struct AppState(pub Arc<AppCore>);

impl Clone for AppState {
    fn clone(&self) -> Self {
        AppState(self.0.clone())
    }
}
