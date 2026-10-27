//! 应用运行时状态：命令层、agent 工具层、后台任务都通过它共享配置与服务。

use crate::agent::event::{AgentEvent, TopicsEvent, ViewerEvent, EVENT_AGENT, EVENT_TOAST, EVENT_TOPICS, EVENT_VIEWER};
use crate::agent::AgentService;
use crate::config::AppConfig;
use crate::domain::memory::{MemoryStore, Scope as MemoryScope};
use crate::domain::session::StudySession;
use crate::domain::topic::{Topic, TopicMeta, Workspace};
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
    /// 会话落盘的顺序锁（见 bump_session_counters 的说明）
    session_persist: parking_lot::Mutex<()>,
    /// 沙箱白名单：工作区之外被用户批准过的目录
    approved_roots: RwLock<Vec<PathBuf>>,
    /// 已发现的技能（目录变了就 reload_skills）
    skills_cache: RwLock<Vec<crate::skills::Skill>>,
    /// MCP 服务器的连接状态
    mcp_state: RwLock<Vec<McpEntry>>,
    /// 长期记忆（全局 + 各主题），内存里有缓存，磁盘是唯一真相
    memory: RwLock<MemoryStore>,
    /// 移动端远程控制（迷你 HTTP 服务 + 事件扇出）。没打开时不监听任何端口
    pub remote: Arc<crate::remote::RemoteService>,
}

/// 一个 MCP 服务器的运行状态。
pub struct McpEntry {
    pub config: crate::mcp::McpServerConfig,
    pub client: Option<Arc<crate::mcp::McpClient>>,
    pub error: Option<String>,
    /// 这个服务器「最近发生了什么」（stderr 尾巴 + 启动/退出事件）。
    /// 连不上的服务器也有——那正是最需要看日志的时候。
    pub log: crate::mcp::McpLog,
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
    /// 排查用：这个服务器自己的日志尾巴（stderr 等）
    pub log: Vec<String>,
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
        log: entry.log.tail(),
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
        // 会话恢复要在 config 被移进锁之前做（它只读配置，不碰运行时状态）
        let recovered_session = recover_active_session(&config);
        Arc::new(Self {
            app,
            config_path,
            config: RwLock::new(config),
            http,
            agent: Arc::new(AgentService::new()),
            viewer: Arc::new(ViewerService::new()),
            webviews: crate::viewer::webview::WebviewManager::new(),
            session: RwLock::new(recovered_session),
            session_persist: parking_lot::Mutex::new(()),
            approved_roots: RwLock::new(approved),
            skills_cache: RwLock::new(Vec::new()),
            mcp_state: RwLock::new(Vec::new()),
            memory: RwLock::new(MemoryStore::new()),
            remote: Arc::new(crate::remote::RemoteService::new()),
        })
    }

    /// 配置快照（返回值而不是 guard，避免跨 await 持锁）。
    pub fn config_read(&self) -> AppConfig {
        self.config.read().clone()
    }

    /// 现建一个 HTTP 客户端，给**模型请求**用（对话、一次性调用、连通性测试）。
    ///
    /// 为什么模型请求不共用启动时那个 `http`：reqwest **只在建客户端时读一次**
    /// 系统代理，之后不再刷新。用户开着代理启动、中途关掉代理（或反过来）时，
    /// 那个客户端会一直往已经死掉的代理端口里送请求——症状是「网络请求失败：
    /// error sending request」，重启应用才能恢复。现建的成本只是一次连接池重建
    /// （每次对话轮/点一次测试才付一次），换来的是「发请求那一刻的系统代理」。
    ///
    /// 「请求超时」（配置里的 `request_timeout_secs`）在这里落成**空闲超时**
    /// （read_timeout）：首 token 迟迟不来、或流中途停住，超时照旧生效；
    /// 但健康的耗时长流（思考 + 满速输出，超过 180 秒很正常）不再被总超时
    /// 在流中途掐死、然后被当成网络抖动整轮重发（每次都重新计费）。
    /// 总时长仍有 `DEFAULT_TIMEOUT_SECS`（10 分钟）兜底。
    pub fn http_fresh(&self) -> AppResult<reqwest::Client> {
        let idle = self.config_read().agent.request_timeout_secs.max(30);
        crate::net::client_with_idle(crate::net::DEFAULT_TIMEOUT_SECS, idle)
    }

    pub fn workspace(&self) -> Workspace {
        Workspace::new(self.config.read().workspace_root.clone())
    }

    /// 工作区界面快照的落盘位置：`<工作区>/.hub/ui-state.json`。
    ///
    /// 跟着工作区走而不是放在应用配置目录——内置浏览器标签里全是
    /// 工作区内的相对引用，换一个工作区恢复另一份标签没有意义。
    pub fn ui_state_path(&self) -> PathBuf {
        self.config_read()
            .workspace_root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("ui-state.json")
    }

    /// 改配置并落盘。**先写盘成功再提交进内存**：反过来的话，保存失败（磁盘满 /
    /// 安全软件锁文件）时命令报了错，运行态却已经变了样，重启又全回退。
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> AppResult<AppConfig> {
        let mut next = self.config.read().clone();
        f(&mut next);
        next.save(&self.config_path)?;
        *self.config.write() = next;
        Ok(self.config.read().clone())
    }

    // ---------------------------------------------------------- 事件

    /// **所有**给界面的事件都从这里出去：本地 webview 一份，连着的手机一份。
    ///
    /// 为什么收成一处：远程的手机页面跑的是桌面端同一份前端（见 `remote/mod.rs`），
    /// 它认的事件名和桌面端完全一样，所以「发事件」和「扇给远程」本来就是同一件事。
    /// 散在各个业务模块里各写一遍的话，新加的事件迟早会漏掉手机那一份，
    /// 而且那种漏是**静默**的（手机上就是不更新，没有任何报错）。
    pub fn emit_event<T: serde::Serialize + Clone>(&self, name: &str, payload: T) {
        let _ = self.app.emit(name, payload.clone());
        // 没有订阅者时 broadcast::send 只做一个原子读就返回，桌面端不吃亏。
        self.remote.push_event(name, &payload);
    }

    pub fn emit_agent(&self, ev: &AgentEvent) {
        self.emit_event(EVENT_AGENT, ev.clone());
    }

    pub fn emit_viewer(&self, ev: ViewerEvent) {
        self.emit_event(EVENT_VIEWER, ev);
    }

    /// 查看器状态有变化（开/关/切标签）时，把全量状态推给前端。
    pub async fn emit_viewer_sync(&self) {
        let snapshot: ViewerSnapshot = self.viewer.snapshot().await;
        self.emit_viewer(ViewerEvent::Sync { snapshot });
    }

    /// 启动时给恢复出来的**演示页**标签重新发通行证。
    ///
    /// 通行证（`lhpage://` 的 token → 目录）只活在内存里，重启后旧的 URL 一律失效——
    /// 不补这一步，用户开机会看到一个「通行证无效」的页面。判据是标签的 `path`：
    /// 本地 `.html` 都能当演示页跑（打开时就是这么登记的），所以按文件重算即可。
    pub async fn republish_page_urls(&self) {
        let ws = self.config_read().workspace();
        for tab in self.viewer.list().await {
            let (Some(slug), Some(rel)) = (tab.topic_slug.clone(), tab.path.clone()) else {
                continue;
            };
            let Ok(topic) = ws.resolve(&slug) else { continue };
            let Ok(abs) = crate::paths::resolve_in_root(&topic.dir, &rel) else { continue };
            if !crate::viewer::pages::is_page_file(&abs) {
                continue;
            }
            if let Some(url) = crate::viewer::pages::pages().url_for_file(&abs) {
                if tab.url.as_deref() != Some(url.as_str()) {
                    self.viewer.set_url(&tab.id, url).await;
                }
            }
        }
    }

    pub fn emit_topics(&self, ev: TopicsEvent) {
        self.emit_event(EVENT_TOPICS, ev);
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
        let message = message.into();
        self.emit_event(
            EVENT_TOAST,
            serde_json::json!({ "level": level, "message": message }),
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
                    log: crate::mcp::McpLog::new(),
                });
                continue;
            }
            // 名字要拼进工具名 `mcp__<名>__<工具>`，服务商对工具名有硬性格式规则
            //（`^[a-zA-Z0-9_-]{1,64}$`，违反的请求整体 400）。手动添加与导入两条路
            // 都会拦，但配置文件可以被直接手改——这里兜住最后一道：宁可这台服务器
            // 连不上、原因写在面板上，也不能让每一轮带工具的请求都 400。
            if !crate::agent::tools::mcp::is_server_name_valid(&cfg.name) {
                let why = "服务器名不合法：只能用英文字母、数字、下划线、连字符（不能带 __），\
                           不超过 40 个字符——它要拼进工具名 mcp__<名>__<工具>，模型服务商只接受这种格式"
                    .to_string();
                eprintln!("[mcp] {} {why}", cfg.name);
                let log = crate::mcp::McpLog::new();
                log.push(why.clone());
                entries.push(McpEntry {
                    config: cfg,
                    client: None,
                    error: Some(why),
                    log,
                });
                continue;
            }
            // 日志先建好再连：连不上也要把「为什么」留下来（stderr / 退出码 / 命令解析）
            let log = crate::mcp::McpLog::new();
            match crate::mcp::McpClient::connect(&cfg, log.clone()).await {
                Ok(client) => {
                    match client.list_tools().await {
                        Ok(tools) => {
                            let mut seen: std::collections::HashSet<String> =
                                std::collections::HashSet::new();
                            for t in tools {
                                // 工具名做「救能救的」清洗（点/空格/中文换下划线、截到 64 字符）；
                                // 清洗后放不下或撞名的只丢这一个工具，并把决定写进日志
                                match crate::agent::tools::mcp::exposed_tool_name(&cfg.name, &t.name)
                                {
                                    None => {
                                        let why = format!(
                                            "工具「{}」拼上前缀后超过 {} 字符上限，没有登记",
                                            t.name,
                                            crate::agent::tools::mcp::TOOL_NAME_MAX
                                        );
                                        log.push(why.clone());
                                        eprintln!("[mcp] {} {why}", cfg.name);
                                    }
                                    Some(exposed) => {
                                        // 撞名（清洗后同名）只登记先到的那个，把决定写进日志
                                        if seen.insert(exposed) {
                                            if let Some(tool) =
                                                crate::agent::tools::mcp::McpTool::try_new(
                                                    client.clone(),
                                                    t,
                                                )
                                            {
                                                new_tools.push(Arc::new(tool));
                                            }
                                        } else {
                                            let why = format!(
                                                "工具「{}」清洗后与别的工具重名，只登记先到的那个",
                                                t.name
                                            );
                                            log.push(why.clone());
                                            eprintln!("[mcp] {} {why}", cfg.name);
                                        }
                                    }
                                }
                            }
                            entries.push(McpEntry {
                                config: cfg.clone(),
                                client: Some(client),
                                error: None,
                                log,
                            });
                        }
                        // 握手过了但工具清单拉不到 → 这个服务器没法用。
                        // 以前这里只 eprintln 一句、服务器照样标成「已连接」，
                        // 界面上就变成「连上了但 0 个工具」，谁都不知道为什么。
                        Err(e) => {
                            let why = format!("已连接，但 tools/list 失败：{e}");
                            log.push(why.clone());
                            eprintln!("[mcp] {} {why}", cfg.name);
                            client.shutdown().await;
                            entries.push(McpEntry {
                                config: cfg.clone(),
                                client: None,
                                error: Some(why),
                                log,
                            });
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[mcp] {} 连接失败：{e}", cfg.name);
                    entries.push(McpEntry {
                        config: cfg.clone(),
                        client: None,
                        error: Some(e.to_string()),
                        log,
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
        self.emit_event(
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
        // None 的落盘由结束方负责（session_finish 写完再置 None）；这里只管 Some
        if let Some(ref s) = s {
            self.persist_session(s);
        }
        *self.session.write() = s;
    }

    /// 开始新会话前收尾旧的：把还挂在内存里的进行中会话补上 ended_at 落盘。
    /// 不收尾的话旧文件永远缺 ended_at，session_list 里会一直挂着一条
    /// 「进行中」的僵尸会话（重复 session_start 就会攒出一堆）。
    pub fn close_active_session(&self, note: &str) -> Option<StudySession> {
        let _order = self.session_persist.lock();
        let mut s = self.session.write().take()?;
        s.ended_at = Some(chrono::Utc::now());
        if s.summary.is_empty() {
            s.summary = note.to_string();
        }
        self.write_session_file(&s);
        Some(s)
    }

    /// 结束当前会话：`f` 里补总结 / ended_at 等字段，然后落盘、摘指针。
    ///
    /// 三步必须在**同一把顺序锁里**完成：中途放开的话，一次并发 bump 会拿着
    /// 还没 ended_at 的旧快照把刚写好的「已结束」文件盖回「进行中」，
    /// 下次启动就把总结完的会话当进行中的复活了。
    pub fn finish_current_session(
        &self,
        f: impl FnOnce(&mut StudySession),
    ) -> Option<StudySession> {
        let _order = self.session_persist.lock();
        let mut s = self.session.write().take()?;
        f(&mut s);
        self.write_session_file(&s);
        Some(s)
    }

    pub fn bump_session_counters(&self, notes: u32, cards: u32) {
        // 快照必须**在顺序锁里**取：两次并发 bump 若只在落盘时排队，
        // 后落盘的可能是先取的旧快照，把先落盘的新计数盖回去
        let _order = self.session_persist.lock();
        let updated = {
            let mut guard = self.session.write();
            guard.as_mut().map(|s| {
                s.notes_created += notes;
                s.cards_created += cards;
                s.clone()
            })
        };
        if let Some(s) = updated {
            self.write_session_file(&s);
        }
    }

    /// 把会话写回 `sessions/<id>.json`，保证异常退出也不丢。
    ///
    /// 按 **topic_id** 定位主题而不是会话里记的 topic_slug：目录名会随主题改名
    /// 一起变，按旧 slug 找不到是小事，掉进名字模糊匹配会把会话写进别的主题。
    ///
    /// **不带锁**——调用方必须已持有 `session_persist`（见各调用点）；
    /// 公开入口是 [`Self::persist_session`]。
    fn write_session_file(&self, s: &StudySession) {
        let ws = self.workspace();
        let topic = match ws.resolve_id(&s.topic_id) {
            Ok(t) => t,
            // 主题没了（被删）：不能把会话文件写到任何别的地方，只能放弃落盘
            Err(_) => return,
        };
        let dir = topic.sessions_dir();
        if crate::paths::ensure_dir(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", s.id));
        if let Err(e) = store::write_json(&path, s) {
            eprintln!("[session] 保存失败：{e}");
        }
    }

    /// 会话落盘的唯一公开入口：所有对同一份 `sessions/<id>.json` 的写入
    /// 都从这把顺序锁过，保证「谁最后写，谁的快照就是最新的」。
    pub fn persist_session(&self, s: &StudySession) {
        let _order = self.session_persist.lock();
        self.write_session_file(s);
    }
}

/// 启动时找回「进行中」的学习会话。
///
/// 当前会话指针只存内存，进程重启（崩溃 / tauri dev 重编译 / 关掉再开）后是
/// None，但 `sessions/<id>.json` 在会话进行中就一直落着盘（ended_at 为空）。
/// 不恢复的话 session_finish 会说「没有进行中的会话」，用户当场看到模型只好
/// 把总结写成笔记、会话记录从此缺一个收尾。取 started_at 最新的那条。
fn recover_active_session(config: &AppConfig) -> Option<StudySession> {
    let root = &config.workspace_root;
    // 只读 topic.json 拿 slug（list() 会为每个主题扫统计，还是启动路径；
    // 「认领」野目录的副作用也不该在启动时发生）——缺 topic.json 的目录直接跳过
    let mut best: Option<StudySession> = None;
    let Ok(topics) = std::fs::read_dir(root) else { return None };
    for entry in topics.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(Some(_meta)) = store::read_json_opt::<TopicMeta>(&dir.join(crate::domain::topic::TOPIC_FILE))
        else {
            continue;
        };
        let Ok(rd) = std::fs::read_dir(dir.join(crate::domain::topic::DIR_SESSIONS)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x == "json") {
                continue;
            }
            let Ok(Some(s)) = store::read_json_opt::<StudySession>(&p) else { continue };
            if s.ended_at.is_some() {
                continue;
            }
            if best.as_ref().map_or(true, |b| b.started_at < s.started_at) {
                best = Some(s);
            }
        }
    }
    best
}

/// Tauri 的托管状态。命令里写 `State<'_, AppState>` 然后 `state.0.clone()`。
pub struct AppState(pub Arc<AppCore>);

impl Clone for AppState {
    fn clone(&self) -> Self {
        AppState(self.0.clone())
    }
}
