//! 应用运行时状态：命令层、agent 工具层、后台任务都通过它共享配置与服务。

use crate::agent::event::{AgentEvent, TopicsEvent, ViewerEvent, EVENT_AGENT, EVENT_TOAST, EVENT_TOPICS, EVENT_VIEWER};
use crate::agent::AgentService;
use crate::config::AppConfig;
use crate::domain::memory::{MemoryStore, Scope as MemoryScope};
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
    /// 网页标签的原生子 WebView（label = `web-<tab_id>`），见 `viewer::webview` 模块
    pub webviews: crate::viewer::webview::WebviewManager,
    /// 当前进行中的学习会话（同一时间只允许一个）
    session: RwLock<Option<StudySession>>,
    /// 沙箱白名单：工作区之外被用户批准过的目录
    approved_roots: RwLock<Vec<PathBuf>>,
    /// 已发现的技能（目录变了就 reload_skills）
    skills_cache: RwLock<Vec<crate::skills::Skill>>,
    /// MCP 服务器的连接状态
    mcp_state: RwLock<Vec<McpEntry>>,
    /// 长期记忆（全局 + 各主题），内存里有缓存，磁盘是唯一真相
    memory: RwLock<MemoryStore>,
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
            webviews: crate::viewer::webview::WebviewManager::new(),
            session: RwLock::new(None),
            approved_roots: RwLock::new(approved),
            skills_cache: RwLock::new(Vec::new()),
            mcp_state: RwLock::new(Vec::new()),
            memory: RwLock::new(MemoryStore::new()),
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
        let found = crate::skills::discover(Some(&cfg.workspace_root), &[], &cfg.agent.extra_skill_dirs);
        let n = found.len();
        *self.skills_cache.write() = found;
        n
    }

    /// 一个主题可见的技能目录：自己的在前，父主题的在后（父主题私有技能子主题也能用）。
    fn skill_dirs_for(&self, topic: &crate::domain::topic::Topic) -> Vec<std::path::PathBuf> {
        let ws = self.workspace();
        let mut dirs = vec![topic.dir.clone()];
        dirs.extend(ws.ancestors(topic).into_iter().map(|a| a.dir));
        dirs
    }

    /// **在某个主题下真正生效**的技能：全局 + 该主题私有 + 父主题私有，
    /// 再剪掉禁用——禁用是**沿父子链取并集**的：父主题关掉的，子主题也关。
    /// 判断只看开关，不看技能来自哪个目录（用户以后往哪儿加都适用）。
    pub fn skills_for(&self, topic: Option<&crate::domain::topic::Topic>) -> Vec<crate::skills::Skill> {
        let cfg = self.config_read();
        if !cfg.agent.skills_enabled {
            return Vec::new();
        }
        let dirs = topic.map(|t| self.skill_dirs_for(t)).unwrap_or_default();
        let all = crate::skills::discover(Some(&cfg.workspace_root), &dirs, &cfg.agent.extra_skill_dirs);

        let mut disabled = cfg.agent.disabled_skills.clone();
        if let Some(t) = topic {
            disabled.extend(t.meta.tools.disabled_skills.iter().cloned());
            for a in self.workspace().ancestors(t) {
                disabled.extend(a.meta.tools.disabled_skills.iter().cloned());
            }
        }
        crate::skills::effective(all, cfg.agent.skills_enabled, &disabled)
    }

    /// 技能在指定主题下「该不该给 agent」——面板与提示词都以此为准，
    /// 免得两处各写一套判断。返回不可用的原因（给界面显示）。
    pub fn skill_off_reason(
        &self,
        skill: &crate::skills::Skill,
        topic: Option<&crate::domain::topic::Topic>,
    ) -> Option<String> {
        let cfg = self.config_read();
        if !cfg.agent.skills_enabled {
            return Some("总开关".into());
        }
        if cfg.agent.disabled_skills.iter().any(|d| d == &skill.id) {
            return Some("global".into());
        }
        if let Some(by) = self.skill_disabled_by_topic(&skill.id, topic) {
            return Some(by.into());
        }
        None
    }

    /// 某个技能在指定主题下是否被**主题层面**关着，以及是谁关的。
    /// 返回 `None` 表示没被主题/父主题关（还要再看全局开关）。
    pub fn skill_disabled_by_topic(
        &self,
        id: &str,
        topic: Option<&crate::domain::topic::Topic>,
    ) -> Option<&'static str> {
        let t = topic?;
        if !t.meta.tools.skill_enabled(id) {
            return Some("topic");
        }
        if self
            .workspace()
            .ancestors(t)
            .iter()
            .any(|a| !a.meta.tools.skill_enabled(id))
        {
            return Some("parent");
        }
        None
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
    ///
    /// 禁用同样沿父子链取并集：父主题（整门课）关掉的服务器，学某一章时也不会冒出来。
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
        let Some(t) = topic else { return true };
        if !t.meta.tools.mcp_enabled(name) {
            return false;
        }
        !self
            .workspace()
            .ancestors(t)
            .iter()
            .any(|a| !a.meta.tools.mcp_enabled(name))
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

    /// 记忆被改动后通知前端刷新面板。
    pub fn emit_memory_changed(&self, scope: &str) {
        let _ = self.app.emit(
            crate::agent::event::EVENT_MEMORY,
            crate::agent::event::MemoryEvent {
                scope: scope.to_string(),
                action: "changed".into(),
            },
        );
    }

    // ---------------------------------------------------------- 长期记忆

    /// 记忆仓库句柄（命令层要用它做增删改）。
    pub fn memory(&self) -> &RwLock<MemoryStore> {
        &self.memory
    }

    /// 记忆功能是否开启（关掉后既不注入提示词，也不给 agent 记忆工具）。
    pub fn memory_enabled(&self) -> bool {
        self.config_read().agent.memory_enabled
    }

    /// 某个主题的记忆作用域 + 它的目录（写盘时要用）。
    ///
    /// 为什么不在这里就把记忆读进缓存：读缓存的 load 不能被调用方持有写锁时做，
    /// 分两步调用（先解析作用域，再 `memory_read`）能避免锁里再取锁。
    pub fn memory_scope_for(&self, topic: Option<&Topic>) -> (MemoryScope, Option<PathBuf>) {
        match topic {
            Some(t) => (MemoryScope::Topic(t.slug()), Some(t.dir.clone())),
            None => (MemoryScope::Global, None),
        }
    }

    /// 在**这个上下文里生效**的记忆作用域，按优先级从高到低：
    /// 本主题 → 父主题（同一门课记下的事，学某一章时照样成立）→ 全局。
    /// 元组第二项是给界面/日志看的标签（全局那份为空）。
    pub fn memory_scopes(&self, topic: Option<&Topic>) -> Vec<(MemoryScope, String)> {
        let mut out = Vec::new();
        if let Some(t) = topic {
            out.push((MemoryScope::Topic(t.slug()), t.meta.name.clone()));
            for a in self.workspace().ancestors(t) {
                out.push((MemoryScope::Topic(a.slug()), a.meta.name.clone()));
            }
        }
        out.push((MemoryScope::Global, String::new()));
        out
    }

    /// 在只读锁下读记忆（给命令层与工具用）。
    pub fn memory_read<T>(&self, f: impl FnOnce(&MemoryStore) -> T) -> T {
        f(&self.memory.read())
    }

    /// 在写锁下改记忆（增删改都要立刻落盘）。
    pub fn memory_write<T>(&self, f: impl FnOnce(&mut MemoryStore) -> AppResult<T>) -> AppResult<T> {
        let mut guard = self.memory.write();
        f(&mut guard)
    }

    /// 组装这个上下文的记忆版块（空串表示没有可注入的记忆或功能被关掉了）。
    ///
    /// 注意：持有 `parking_lot` 锁的代码块里**不能 await**，所以这里全程同步。
    pub fn memory_digest(&self, topic: Option<&Topic>) -> String {
        if !self.memory_enabled() {
            return String::new();
        }
        let cfg = self.config_read();
        let root = cfg.workspace_root.clone();
        let mut guard = self.memory.write();
        if let Err(e) = guard.load(&root) {
            eprintln!("[memory] 读取全局记忆失败：{e}");
            return String::new();
        }
        // 把本主题与父主题的记忆读进缓存（用户可能刚用编辑器改过文件）
        if let Some(t) = topic {
            let mut chain = vec![t.clone()];
            chain.extend(self.workspace().ancestors(t));
            for item in chain {
                if let Err(e) = guard.load_topic(&item.slug(), &item.dir) {
                    eprintln!("[memory] 读取主题记忆失败：{e}");
                }
            }
        }
        let scopes = self.memory_scopes(topic);
        let (text, _, _) = guard.digest(&scopes);
        text
    }

    /// 只看不记：和 [`AppCore::memory_digest`] 一样组装版块，但**不**更新使用计数。
    ///
    /// 系统提示词预览用它——预览几十次不该把「用了几次」刷成几十次。
    pub fn memory_peek(&self, topic: Option<&Topic>) -> String {
        if !self.memory_enabled() {
            return String::new();
        }
        let root = self.config_read().workspace_root.clone();
        let mut guard = self.memory.write();
        if let Err(e) = guard.load(&root) {
            eprintln!("[memory] 读取全局记忆失败：{e}");
            return String::new();
        }
        if let Some(t) = topic {
            let mut chain = vec![t.clone()];
            chain.extend(self.workspace().ancestors(t));
            for item in chain {
                if let Err(e) = guard.load_topic(&item.slug(), &item.dir) {
                    eprintln!("[memory] 读取主题记忆失败：{e}");
                }
            }
        }
        guard.digest_text(&self.memory_scopes(topic))
    }

    /// 把攒着的「记忆被用到过」落盘（切换主题、关窗口前调用）。
    pub fn memory_flush(&self) {
        if !self.memory_enabled() {
            return;
        }
        let root = self.config_read().workspace_root.clone();
        // 主题 slug 就是工作区下的一级目录名，直接拼即可（父主题与子主题都是平级目录）
        let topics: Vec<(String, PathBuf)> = self
            .workspace()
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.slug.clone(), root.join(&t.slug)))
            .collect();
        let mut guard = self.memory.write();
        if let Err(e) = guard
            .load(&root)
            .and_then(|_| guard.flush(&topics))
        {
            eprintln!("[memory] 长期记忆落盘失败：{e}");
        }
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
