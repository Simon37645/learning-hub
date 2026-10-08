//! Agent 运行时：一轮对话从「用户说一句话」到「该落盘的都落盘」的全过程。
//!
//! 循环结构（en-US 里叫 agentic loop）：
//!
//! ```text
//! 用户消息 → 模型流式回复 ─┬─ 有工具调用 → 审批 → 执行 → 结果回灌 → 回到模型
//!                         └─ 无工具调用 → 结束
//! ```
//!
//! 设计约束：
//! - 每轮都有可中止开关，用户点「停止」立刻在流循环里生效
//! - 工具按风险分级，按配置的权限模式决定是否打断用户
//! - 上下文超预算时，从最早的「完整轮次」开始丢，绝不切断工具调用与结果的配对

pub mod attachment;
pub mod chats;
pub mod event;
pub mod message;
pub mod prompt;
pub mod provider;
pub mod registry;
pub mod tools;

use crate::agent::attachment::Attachments;
use crate::agent::event::{AgentEvent, PendingCall, Risk, ToolOutcomeView};
use crate::agent::message::{estimate_messages_tokens, ChatMessage, ContentBlock};
use crate::agent::prompt::PromptInputs;
use crate::agent::provider::{ChatRequest, StreamEvent};
use crate::agent::registry::{AgentMode, ToolCtx, ToolRegistry};
use crate::config::AppConfig;
use crate::domain::topic::Topic;
use crate::error::{AppError, AppResult};
use crate::state::AppCore;
use crate::store;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// 等用户点「允许」的最长时间，超时按拒绝处理。
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

/// 开始一轮对话的入参。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRequest {
    #[serde(default)]
    pub topic_slug: Option<String>,
    pub chat_id: String,
    pub text: String,
    /// 本轮临时指定的学习阶段（不传则用主题自带的）
    #[serde(default)]
    pub stage: Option<String>,
    /// 随消息附上的文件（相对主题目录），会写进提示词
    #[serde(default)]
    pub attachments: Vec<String>,
    /// 随消息附上的图片。字节由前端传 base64 过来，落地成 `image` 块（见 `attachment.rs`）
    #[serde(default)]
    pub images: Vec<ImageUpload>,
    /// study（学习，默认）/ studio（工坊）。老对话与不传这个字段的前端都是学习模式
    #[serde(default)]
    pub mode: AgentMode,
}

/// 一张随消息上传的图片（前端贴图/拖文件时传来）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUpload {
    /// 原文件名，只用于显示
    #[serde(default)]
    pub name: String,
    /// base64（允许带 `data:image/png;base64,` 前缀）。
    /// **类型不认这里说的**，认字节里的魔数——前端可能把 `.txt` 改名成 `.png` 传进来。
    pub data: String,
    /// 像素尺寸（前端量得到就给，用于估算 token）
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnHandleView {
    pub turn_id: String,
    pub chat_id: String,
    pub started_at_ms: i64,
}

struct TurnHandle {
    cancel: Arc<AtomicBool>,
    started: Instant,
}

/// 待审批的工具调用：request_id → (工具名, 决定回执)
type ApprovalSlot = (String, oneshot::Sender<bool>);

pub struct AgentService {
    /// 工具表。MCP 服务器是运行时才连上来的，所以这里要能动态加工具。
    tools: parking_lot::RwLock<ToolRegistry>,
    turns: Mutex<HashMap<String, TurnHandle>>,
    approvals: Mutex<HashMap<String, ApprovalSlot>>,
    /// 本次运行内「始终允许」的工具
    always_allow: Mutex<HashSet<String>>,
}

impl Default for AgentService {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentService {
    pub fn new() -> Self {
        Self {
            tools: parking_lot::RwLock::new(tools::registry()),
            turns: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            always_allow: Mutex::new(HashSet::new()),
        }
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.read().names()
    }

    /// 工具声明（给前端做能力清单面板用）。
    pub fn tools_specs(&self) -> Vec<registry::ToolSpec> {
        self.tools.read().specs()
    }

    /// 工具清单文本（降级提示词里用）。
    pub fn describe_tools(&self) -> String {
        self.tools.read().describe_for_prompt()
    }

    /// 某个模式下的工具名 / 声明 / 清单文本。
    ///
    /// 学习模式与工坊模式看到的是**不同的工具表**：工坊不出现「出卷」「写卡片」这类
    /// 需要主题的工具，而学习模式不出现发布技能/登记 MCP 的工具。
    pub fn names_for(&self, mode: AgentMode) -> Vec<String> {
        self.tools.read().names_for(mode)
    }

    pub fn specs_for(&self, mode: AgentMode) -> Vec<registry::ToolSpec> {
        self.tools.read().specs_for(mode)
    }

    pub fn describe_for(&self, mode: AgentMode) -> String {
        self.tools.read().describe_for_prompt_for(mode)
    }

    /// 运行时登记外部工具（MCP 服务器接上来时用）。
    pub fn register_tools(&self, extra: Vec<Arc<dyn registry::Tool>>) -> usize {
        let mut guard = self.tools.write();
        let n = extra.len();
        for tool in extra {
            guard.register(tool);
        }
        n
    }

    /// 摘掉上一轮登记的 MCP 工具（重连前先清，避免越堆越多）。
    pub fn unregister_mcp_tools(&self) -> usize {
        self.tools.write().unregister_prefix(crate::agent::tools::mcp::PREFIX)
    }

    /// 启动一轮对话：立刻返回（含落库后的用户消息），实际工作在后台任务里跑。
    pub fn start_turn(
        self: &Arc<Self>,
        core: Arc<AppCore>,
        req: TurnRequest,
    ) -> AppResult<(String, ChatMessage)> {
        let text = req.text.trim().to_string();
        // 只贴图不打字是合法的：「这张图里的第三行是什么」可以省略
        if text.is_empty() && req.images.is_empty() {
            return Err(AppError::invalid("消息不能为空"));
        }

        // 图片先落盘（消息里只存路径）。中途失败要把已经写下的删掉：
        // 用户看到的是「发送失败」，没人会来清理那几个半途留下的文件。
        let attachments = Attachments::new(core.config_read().workspace_root.clone());
        let mut image_blocks: Vec<ContentBlock> = Vec::new();
        for up in &req.images {
            match save_image(&attachments, up) {
                Ok(block) => image_blocks.push(block),
                Err(e) => {
                    for b in &image_blocks {
                        if let Some(img) = b.as_image() {
                            attachments.remove(img.path);
                        }
                    }
                    return Err(e);
                }
            }
        }

        let turn_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.turns.lock().insert(
            turn_id.clone(),
            TurnHandle { cancel: cancel.clone(), started: Instant::now() },
        );

        // 用户消息先落库，前端拿到 id 后直接渲染，避免重复
        let body = compose_user_text(&text, &req.attachments);
        let user_msg = if image_blocks.is_empty() {
            ChatMessage::user(body)
        } else {
            ChatMessage::user_with_images(&body, image_blocks)
        };
        let topic = match req.mode {
            // 工坊没有主题：它的对话也是工作区级的
            AgentMode::Studio => None,
            AgentMode::Study => resolve_topic_opt(&core, req.topic_slug.as_deref()),
        };
        append_transcript(&core, topic.as_ref(), &req.chat_id, &user_msg)?;

        // 工坊的对话要在侧车文件里记下自己的身份，否则侧栏会把它当成首页的日常问答。
        // 这是唯一一个「必须写侧车」的情形（其它字段都是用户改过才写）。
        if req.mode == AgentMode::Studio {
            let dir = crate::agent::chats_dir_for(&core, None);
            let mut meta = crate::agent::chats::load(&dir, &req.chat_id);
            if meta.mode != AgentMode::Studio {
                meta.mode = AgentMode::Studio;
                crate::agent::chats::save(&dir, &req.chat_id, &mut meta)?;
            }
        }

        let svc = self.clone();
        let tid = turn_id.clone();
        let req2 = TurnRequest { text, ..req };
        tokio::spawn(async move {
            let started = Instant::now();
            let result = svc.run_turn(core.clone(), tid.clone(), req2, cancel).await;
            let ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(reason) => core.emit_agent(&AgentEvent::Finished {
                    turn_id: tid.clone(),
                    reason,
                    duration_ms: ms,
                    interrupts: 0,
                }),
                Err(AppError::Cancelled) => core.emit_agent(&AgentEvent::Finished {
                    turn_id: tid.clone(),
                    reason: "cancelled".into(),
                    duration_ms: ms,
                    interrupts: 1,
                }),
                Err(e) => core.emit_agent(&AgentEvent::Failed { turn_id: tid.clone(), message: e.to_string() }),
            }
            svc.turns.lock().remove(&tid);
        });

        Ok((turn_id, user_msg))
    }

    /// 中止一轮对话。
    pub fn cancel(&self, turn_id: &str) -> bool {
        if let Some(h) = self.turns.lock().get(turn_id) {
            h.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn running_turns(&self) -> Vec<TurnHandleView> {
        let now = Instant::now();
        self.turns
            .lock()
            .iter()
            .map(|(id, h)| TurnHandleView {
                turn_id: id.clone(),
                chat_id: String::new(),
                started_at_ms: now.duration_since(h.started).as_millis() as i64,
            })
            .collect()
    }

    /// 回应用户的审批弹窗。
    pub fn respond_approval(&self, request_id: &str, allow: bool, always: bool) -> bool {
        let slot = self.approvals.lock().remove(request_id);
        match slot {
            Some((tool_name, tx)) => {
                if allow && always {
                    self.always_allow.lock().insert(tool_name);
                }
                let _ = tx.send(allow);
                true
            }
            None => false,
        }
    }

    /// 主循环。
    async fn run_turn(
        self: &Arc<Self>,
        core: Arc<AppCore>,
        turn_id: String,
        req: TurnRequest,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<String> {
        let cfg = core.config_read();
        let profile = cfg.active_profile()?.clone();
        let workspace = cfg.workspace();
        let mode = req.mode;
        // 图片附件按工作区定位（对话可以在主题之间搬，附件不跟着主题走）
        let attachments = Attachments::new(workspace.root.clone());
        let topic: Option<Topic> = match mode {
            // 工坊没有主题：它的产出物（技能 / MCP 服务器）不属于任何一门课
            AgentMode::Studio => None,
            AgentMode::Study => match req.topic_slug.as_deref() {
                Some(s) if !s.trim().is_empty() => Some(workspace.resolve(s)?),
                _ => None,
            },
        };
        // 相对路径的根：学习模式是主题目录，工坊模式是练习目录
        let studio = crate::studio::Studio::new(workspace.root.clone());
        let root = match mode {
            AgentMode::Studio => {
                studio.ensure()?;
                studio.dir()
            }
            AgentMode::Study => topic
                .as_ref()
                .map(|t| t.dir.clone())
                .unwrap_or_else(|| workspace.root.clone()),
        };

        core.emit_agent(&AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            chat_id: req.chat_id.clone(),
            topic_slug: topic.as_ref().map(|t| t.slug()),
            message_id: String::new(),
            mode: cfg.agent.permission_mode,
        });

        let mut messages = load_transcript(&core, topic.as_ref(), &req.chat_id)?;
        // 历史里已经包含刚落库的用户消息，这里不再追加

        let stage = req
            .stage
            .as_deref()
            .and_then(crate::domain::stage::StudyStage::parse)
            .or_else(|| topic.as_ref().map(|t| t.meta.stage));

        // 两套提示词：工坊那份没有主题、没有资料清单，换成「读规范 → 写文件 → 发布 → 看状态」
        let (system, system_sections) = match mode {
            AgentMode::Studio => prompt::build_studio_prompt_with_stats(&prompt::StudioInputs {
                config: &cfg,
                supports_tools: profile.supports_tools,
                tool_catalog: self.describe_for(mode),
                tool_names: self.names_for(mode),
                skill_catalog: crate::skills::catalog(&core.skills_for(None), 120),
                bench: crate::studio::DIR_REL.to_string(),
                // 工坊里没有记忆工具，但「用户是谁」仍然有用（全局那份）
                memory: core.memory_digest(None),
            }),
            AgentMode::Study => prompt::build_system_prompt_with_stats(&PromptInputs {
                config: &cfg,
                topic: topic.as_ref(),
                stage,
                supports_tools: profile.supports_tools,
                tool_catalog: self.describe_for(mode),
                tool_names: self.names_for(mode),
                skill_catalog: crate::skills::catalog(&core.skills_for(topic.as_ref()), 120),
                // 长期记忆：在这一轮开始时就装进提示词，并记下「被用到过」
                memory: core.memory_digest(topic.as_ref()),
            }),
        };
        let system = format!("{}\n\n{}", prompt::current_date_line(), system);

        // 只把「当前模式下可见」的工具给模型（工坊不给出卷、写卡片这些要主题的工具）；
        // 记忆总开关关掉时，记忆工具也要摘掉——否则模型会一次次尝试写入再被拒。
        let memory_on = cfg.agent.memory_enabled;
        let specs = if profile.supports_tools {
            self.specs_for(mode)
                .into_iter()
                .filter(|t| memory_on || !crate::agent::tools::memory::is_memory_tool(&t.name))
                .filter(|t| core.mcp_tool_allowed(&t.name, topic.as_ref()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let provider = provider::build(&profile, core.http.clone());
        let ctx = ToolCtx {
            core: core.clone(),
            topic: topic.clone(),
            root,
            mode,
            turn_id: turn_id.clone(),
            chat_id: req.chat_id.clone(),
        };

        let mut total_in = 0u32;
        let mut total_out = 0u32;
        // 真实用量（服务商报的那部分）。服务商不报时保持 0，界面按「无数据」处理。
        let mut total_cached = 0u32;
        let mut total_cache_write = 0u32;
        let mut reason = "stop".to_string();

        for iteration in 1..=cfg.agent.max_iterations.max(1) {
            if cancel.load(Ordering::SeqCst) {
                return Err(AppError::Cancelled);
            }
            core.emit_agent(&AgentEvent::Iteration {
                turn_id: turn_id.clone(),
                index: iteration,
                max: cfg.agent.max_iterations,
            });

            trim_to_budget(&mut messages, cfg.agent.context_budget_chars);

            let request = ChatRequest {
                model: profile.model.clone(),
                system: system.clone(),
                messages: messages.clone(),
                tools: specs.clone(),
                temperature: profile.temperature,
                max_tokens: profile.max_tokens,
                reasoning: profile.reasoning,
                timeout: Duration::from_secs(cfg.agent.request_timeout_secs.max(30)),
                cancel: cancel.clone(),
                attachments: attachments.clone(),
                supports_vision: profile.supports_vision,
            };
            let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
            let provider_task = {
                let provider = provider.clone();
                tokio::spawn(async move { provider.stream(request, tx).await })
            };

            let message_id = uuid::Uuid::new_v4().to_string();
            let mut acc = Accumulator::default();
            while let Some(ev) = rx.recv().await {
                match ev {
                    StreamEvent::Usage(u) => acc.usage.merge(u),
                    StreamEvent::Text(t) => {
                        acc.text.push_str(&t);
                        core.emit_agent(&AgentEvent::Delta {
                            turn_id: turn_id.clone(),
                            message_id: message_id.clone(),
                            text: t,
                            thinking: false,
                        });
                    }
                    StreamEvent::Thinking(t) => {
                        acc.thinking.push_str(&t);
                        core.emit_agent(&AgentEvent::Delta {
                            turn_id: turn_id.clone(),
                            message_id: message_id.clone(),
                            text: t,
                            thinking: true,
                        });
                    }
                    StreamEvent::ToolCall { index, id, name, args } => {
                        acc.push_tool(index, id, name, args);
                    }
                    StreamEvent::Finish(r) => acc.finish = Some(r),
                }
            }

            match provider_task.await {
                Ok(Ok(())) => {}
                Ok(Err(AppError::Cancelled)) => return Err(AppError::Cancelled),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(AppError::other(format!("模型任务异常终止：{join_err}")));
                }
            }

            let tool_calls = acc.tool_calls();
            let mut blocks: Vec<ContentBlock> = Vec::new();
            if !acc.thinking.trim().is_empty() {
                blocks.push(ContentBlock::Thinking { text: acc.thinking.clone() });
            }
            if !acc.text.is_empty() {
                blocks.push(ContentBlock::Text { text: acc.text.clone() });
            }
            for tc in &tool_calls {
                blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            if blocks.is_empty() {
                reason = "empty".into();
                break;
            }

            // 服务商报了真实用量就用真实的（里面含缓存命中），没报才退回本地估算
            let est_in = estimate_messages_tokens(&messages);
            let est_out = crate::agent::message::estimate_tokens(&acc.text)
                + crate::agent::message::estimate_tokens(&acc.thinking);
            let real = acc.usage;
            let (in_tokens, out_tokens) = if real.is_empty() {
                (est_in, est_out)
            } else {
                (real.input_tokens, real.output_tokens)
            };
            total_in += in_tokens;
            total_out += out_tokens;
            total_cached += real.cached_tokens;
            total_cache_write += real.cache_write_tokens;

            let mut assistant = ChatMessage::assistant(blocks);
            assistant.meta.model = Some(profile.model.clone());
            assistant.meta.input_tokens = Some(in_tokens);
            assistant.meta.output_tokens = Some(out_tokens);
            if !real.is_empty() {
                assistant.meta.cached_tokens = Some(real.cached_tokens);
                assistant.meta.cache_write_tokens = Some(real.cache_write_tokens);
            }
            messages.push(assistant.clone());
            append_transcript(&core, topic.as_ref(), &req.chat_id, &assistant)?;
            core.emit_agent(&AgentEvent::Message {
                turn_id: turn_id.clone(),
                message: assistant,
            });

            if tool_calls.is_empty() {
                reason = acc.finish.clone().unwrap_or_else(|| "stop".into());
                break;
            }
            reason = "tool_calls".into();

            // 顺序执行工具：学习场景下工具之间有依赖（先查再写），并行反而容易冲突
            for tc in &tool_calls {
                if cancel.load(Ordering::SeqCst) {
                    return Err(AppError::Cancelled);
                }
                let result = self.execute_tool(&core, &ctx, &turn_id, tc).await;
                let (content, is_error) = match result {
                    Ok(o) => (o.content, o.is_error),
                    Err(e) => (format!("工具执行失败：{e}"), true),
                };
                let tool_msg = ChatMessage::tool_result(tc.id.clone(), content, is_error);
                messages.push(tool_msg.clone());
                append_transcript(&core, topic.as_ref(), &req.chat_id, &tool_msg)?;
            }

            if iteration >= cfg.agent.max_iterations {
                reason = "max_iterations".into();
                break;
            }
        }

        core.emit_agent(&AgentEvent::Usage {
            turn_id: turn_id.clone(),
            input_tokens: total_in,
            output_tokens: total_out,
            cached_tokens: total_cached,
            cache_write_tokens: total_cache_write,
            context: context_breakdown(&system_sections, &messages),
        });
        Ok(reason)
    }

    /// 审批 + 执行单个工具调用，并把结果通过事件告诉前端。
    async fn execute_tool(
        &self,
        core: &Arc<AppCore>,
        ctx: &ToolCtx,
        turn_id: &str,
        tc: &ToolCall,
    ) -> AppResult<registry::ToolOutput> {
        let Some(tool) = self.tools.read().get(&tc.name) else {
            let msg = format!(
                "没有名为 {} 的工具。可用工具：{}",
                tc.name,
                self.tool_names().join("、")
            );
            core.emit_agent(&AgentEvent::ToolFinished {
                turn_id: turn_id.to_string(),
                outcome: ToolOutcomeView {
                    call_id: tc.id.clone(),
                    name: tc.name.clone(),
                    summary: format!("未知工具 {}", tc.name),
                    risk: Risk::Read,
                    ok: false,
                    preview: msg.clone(),
                    duration_ms: 0,
                    denied: false,
                },
            });
            return Ok(registry::ToolOutput::err(msg));
        };

        let summary = tool.summarize(&tc.input);
        let risk = tool.risk();

        // --- 审批 ---
        if !self.is_allowed(core, turn_id, tc, &summary, risk).await {
            let msg = format!(
                "用户拒绝执行 {}({})。请换一种方式，或向用户说明为什么需要这个操作。",
                tc.name, summary
            );
            core.emit_agent(&AgentEvent::ToolFinished {
                turn_id: turn_id.to_string(),
                outcome: ToolOutcomeView {
                    call_id: tc.id.clone(),
                    name: tc.name.clone(),
                    summary: summary.clone(),
                    risk,
                    ok: false,
                    preview: "已被用户拒绝".into(),
                    duration_ms: 0,
                    denied: true,
                },
            });
            return Ok(registry::ToolOutput::err(msg));
        }

        core.emit_agent(&AgentEvent::ToolStarted {
            turn_id: turn_id.to_string(),
            call_id: tc.id.clone(),
            name: tc.name.clone(),
            summary: summary.clone(),
            risk,
        });

        let started = Instant::now();
        let outcome = tool.run(ctx, tc.input.clone()).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let view = match &outcome {
            Ok(o) => ToolOutcomeView {
                call_id: tc.id.clone(),
                name: tc.name.clone(),
                summary,
                risk,
                ok: !o.is_error,
                preview: preview_of(&o.content),
                duration_ms,
                denied: false,
            },
            Err(e) => ToolOutcomeView {
                call_id: tc.id.clone(),
                name: tc.name.clone(),
                summary,
                risk,
                ok: false,
                preview: e.to_string(),
                duration_ms,
                denied: false,
            },
        };
        core.emit_agent(&AgentEvent::ToolFinished {
            turn_id: turn_id.to_string(),
            outcome: view,
        });

        outcome
    }

    /// 越权申请：agent 想访问工作区之外的文件，等用户点头。
    ///
    /// 批准粒度是**目录**：用户同意后整个目录长期有效，不再反复打断。
    pub async fn request_sandbox(
        &self,
        core: &Arc<AppCore>,
        turn_id: &str,
        path: &std::path::Path,
        mode: &str,
        reason: &str,
    ) -> bool {
        if !core.needs_escalation(path) {
            return true;
        }
        let root = crate::paths::grant_root(path);
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.approvals
            .lock()
            .insert(request_id.clone(), (format!("sandbox:{mode}"), tx));

        core.emit_agent(&AgentEvent::SandboxRequest {
            turn_id: turn_id.to_string(),
            request_id: request_id.clone(),
            path: path.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            mode: mode.to_string(),
            reason: reason.to_string(),
            pre_approved: false,
        });

        let allowed = match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(allow)) => allow,
            _ => {
                self.approvals.lock().remove(&request_id);
                false
            }
        };
        if allowed {
            if let Err(e) = core.approve_root(&root) {
                eprintln!("[sandbox] 记录授权失败：{e}");
            }
        }
        allowed
    }
}

/// 起一次「不带工具」的模型调用，把结果收集成完整文本返回。
///
/// 用于那些不需要对话循环、只要一段结构化输出的场景：
/// 测验主观题判分、知识库要点抽取、连通性测试。
pub async fn complete_once(
    core: &Arc<AppCore>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> AppResult<String> {
    let cfg = core.config_read();
    let profile = cfg.active_profile()?.clone();
    let provider = provider::build(&profile, core.http.clone());
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();

    let request = ChatRequest {
        model: profile.model.clone(),
        system: system.to_string(),
        messages: vec![ChatMessage::user(user)],
        tools: Vec::new(),
        temperature: 0.2,
        max_tokens,
        reasoning: profile.reasoning,
        timeout: std::time::Duration::from_secs(cfg.agent.request_timeout_secs.max(30)),
        cancel: Arc::new(AtomicBool::new(false)),
        // 这条路径只做一次结构化输出，不带图片
        attachments: Attachments::disabled(),
        supports_vision: false,
    };

    let task = tokio::spawn(async move { provider.stream(request, tx).await });
    let mut out = String::new();
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Text(t) = ev {
            out.push_str(&t);
        }
    }
    match task.await {
        Ok(Ok(())) => Ok(out),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(AppError::other(format!("模型调用异常：{e}"))),
    }
}

/// 从模型输出里抠出 JSON（它常把 JSON 包在 ``` 里或前后加解释）。
pub fn extract_json<T: serde::de::DeserializeOwned>(text: &str) -> AppResult<T> {
    let trimmed = text.trim();
    // 1) 直接就是 JSON
    if let Ok(v) = serde_json::from_str::<T>(trimmed) {
        return Ok(v);
    }
    // 2) 去掉 ```json 围栏
    let unfenced = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<T>(unfenced) {
        return Ok(v);
    }
    // 3) 取第一个 { 或 [ 到最后一个 } 或 ]
    let start = trimmed.find(['{', '[']);
    let end = trimmed.rfind(['}', ']']);
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            if let Ok(v) = serde_json::from_str::<T>(&trimmed[s..=e]) {
                return Ok(v);
            }
        }
    }
    Err(AppError::other(format!(
        "模型没有按要求返回 JSON：{}",
        provider::truncate(trimmed, 200)
    )))
}

impl AgentService {
    /// 风险 → 是否需要问用户。
    async fn is_allowed(
        &self,
        core: &Arc<AppCore>,
        turn_id: &str,
        tc: &ToolCall,
        summary: &str,
        risk: Risk,
    ) -> bool {
        let mode = core.config_read().agent.permission_mode;
        if !risk.needs_approval(mode) {
            return true;
        }
        if self.always_allow.lock().contains(&tc.name) {
            return true;
        }

        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.approvals
            .lock()
            .insert(request_id.clone(), (tc.name.clone(), tx));

        core.emit_agent(&AgentEvent::ToolApproval {
            turn_id: turn_id.to_string(),
            call: PendingCall {
                request_id: request_id.clone(),
                call_id: tc.id.clone(),
                name: tc.name.clone(),
                summary: summary.to_string(),
                risk,
                risk_label: risk.label().to_string(),
                input: tc.input.clone(),
            },
        });

        match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(allow)) => allow,
            _ => {
                self.approvals.lock().remove(&request_id);
                false
            }
        }
    }
}

// ============================================================ 上下文预算

/// 超预算时从最早的「完整轮次」开始丢消息。
///
/// 一轮 = 一条 user 消息 + 之后的 assistant/tool 消息。按轮丢可以保证
/// `tool_use` 与它对应的 `tool_result` 不会分离（分离会导致服务商直接报错）。
pub fn trim_to_budget(messages: &mut Vec<ChatMessage>, budget_chars: usize) -> usize {
    if budget_chars == 0 {
        return 0;
    }
    let used = |msgs: &[ChatMessage]| -> usize {
        msgs.iter()
            .flat_map(|m| m.blocks.iter())
            .map(|b| match b {
                ContentBlock::Text { text } => text.chars().count(),
                ContentBlock::Thinking { text } => text.chars().count(),
                ContentBlock::ToolUse { input, name, .. } => name.chars().count() + input.to_string().chars().count(),
                ContentBlock::ToolResult { content, .. } => content.chars().count(),
                // 图片几乎没有字符，但很占 token：按一张 1100 token 的量级折算成字符，
                // 否则贴了几十张图的对话永远不会被裁，请求会一路涨到服务商拒收。
                ContentBlock::Image { .. } => 4_000,
            })
            .sum()
    };

    let mut dropped = 0usize;
    while used(messages) > budget_chars && messages.len() > 2 {
        // 找到第二条 user 消息的位置，丢掉它之前的所有内容
        let Some(next_user) = messages
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, m)| m.role == crate::agent::message::Role::User)
            .map(|(i, _)| i)
        else {
            break;
        };
        if next_user == 0 {
            break;
        }
        messages.drain(0..next_user);
        dropped += next_user;
    }
    if dropped > 0 {
        // 插一条提示，让模型知道前面被截断了
        messages.insert(
            0,
            ChatMessage::user("（更早的对话因为长度限制已省略，如果缺少关键信息请重新询问用户。）"),
        );
    }
    dropped
}

// ============================================================ 流式累积

#[derive(Default)]
struct Accumulator {
    text: String,
    thinking: String,
    tools: Vec<ToolCall>,
    finish: Option<String>,
    /// 服务商在这条流里报的真实用量（可能为空）
    usage: provider::ProviderUsage,
}

#[derive(Debug, Clone, Default)]
struct ToolCall {
    id: String,
    name: String,
    raw_args: String,
    input: Value,
    index: usize,
}

impl Accumulator {
    fn push_tool(&mut self, index: usize, id: Option<String>, name: Option<String>, args: String) {
        let slot = match self.tools.iter_mut().find(|t| t.index == index) {
            Some(t) => t,
            None => {
                self.tools.push(ToolCall { index, ..Default::default() });
                self.tools.last_mut().expect("刚 push 过")
            }
        };
        if let Some(id) = id {
            if !id.is_empty() {
                slot.id = id;
            }
        }
        if let Some(name) = name {
            if !name.is_empty() {
                slot.name = name;
            }
        }
        slot.raw_args.push_str(&args);
    }

    fn tool_calls(&mut self) -> Vec<ToolCall> {
        self.tools.sort_by_key(|t| t.index);
        self.tools
            .iter()
            .filter(|t| !t.name.trim().is_empty())
            .map(|t| {
                let mut t = t.clone();
                if t.id.is_empty() {
                    t.id = format!("call_{}", uuid::Uuid::new_v4().simple());
                }
                t.input = parse_tool_args(&t.raw_args);
                t
            })
            .collect()
    }
}

/// 组装「上下文构成」：系统提示词各版块 + 对话消息 + 工具结果。
///
/// 只给界面画比例用：数字是**按字符估**的，和真实用量不会完全相等，
/// 但足够回答「这轮输入为什么这么大」——是资料清单、工具说明还是历史消息。
fn context_breakdown(
    system_sections: &[prompt::PromptSectionStat],
    messages: &[ChatMessage],
) -> Vec<crate::agent::event::ContextPart> {
    use crate::agent::event::ContextPart;
    let est = crate::agent::message::estimate_tokens;

    let mut parts: Vec<ContextPart> = system_sections
        .iter()
        .map(|s| ContextPart { label: s.label.to_string(), tokens: s.tokens })
        .collect();

    let (mut msg_tokens, mut tool_tokens) = (0u32, 0u32);
    for m in messages {
        for b in &m.blocks {
            match b {
                ContentBlock::ToolResult { content, .. } => tool_tokens += est(content),
                ContentBlock::Text { text } | ContentBlock::Thinking { text } => {
                    msg_tokens += est(text)
                }
                ContentBlock::Image { width, height, .. } => {
                    msg_tokens += crate::agent::attachment::estimate_image_tokens(*width, *height)
                }
                ContentBlock::ToolUse { input, .. } => msg_tokens += est(&input.to_string()),
            }
        }
    }
    parts.push(ContextPart { label: "对话消息".into(), tokens: msg_tokens });
    if tool_tokens > 0 {
        parts.push(ContextPart { label: "工具结果".into(), tokens: tool_tokens });
    }
    parts
}

fn parse_tool_args(raw: &str) -> Value {
    let s = raw.trim();
    if s.is_empty() {
        return Value::Object(Default::default());
    }
    match serde_json::from_str::<Value>(s) {
        // 参数必须是对象；模型偶尔会给数组或字符串，这时当作空参数让工具自己报缺参
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(other) => {
            eprintln!(
                "[agent] 工具参数不是对象，已忽略：{}",
                crate::agent::provider::truncate(&other.to_string(), 120)
            );
            Value::Object(Default::default())
        }
        Err(e) => {
            eprintln!(
                "[agent] 工具参数 JSON 解析失败：{e} | {}",
                crate::agent::provider::truncate(s, 200)
            );
            Value::Object(Default::default())
        }
    }
}

// ============================================================ 落地工具函数

fn compose_user_text(text: &str, attachments: &[String]) -> String {    if attachments.is_empty() {
        return text.to_string();
    }
    format!(
        "{text}\n\n（用户随消息附上了这些资料：{}）",
        attachments.join("、")
    )
}

fn preview_of(content: &str) -> String {
    content.chars().take(400).collect()
}

/// 把一张上传的图解码、落盘，转成消息里的 `image` 块。
///
/// 尺寸只用于估算 token，所以先夹到合理范围：前端量出来的数字也可能因为
/// 图片还没加载完而是 0 或 -1，那种脏数据会把估算撑到上限，反而看不出真正的开销。
fn save_image(attachments: &Attachments, up: &ImageUpload) -> AppResult<ContentBlock> {
    let data = crate::agent::attachment::b64_decode(&up.data)?;
    let stored = attachments.save(&up.name, &data)?;
    Ok(ContentBlock::Image {
        path: stored.rel,
        media_type: stored.media_type,
        name: up.name.trim().chars().take(120).collect(),
        bytes: stored.bytes,
        width: up.width.filter(|w| (1..=20000).contains(w)),
        height: up.height.filter(|h| (1..=20000).contains(h)),
    })
}

pub fn resolve_topic_opt(core: &AppCore, slug: Option<&str>) -> Option<Topic> {
    let s = slug?.trim();
    if s.is_empty() {
        return None;
    }
    core.workspace().resolve(s).ok()
}

/// 对话记录的落盘路径：有主题就放主题内，没有就放工作区的 `_hub/`。
pub fn transcript_path(core: &AppCore, topic: Option<&Topic>, chat_id: &str) -> PathBuf {
    match topic {
        Some(t) => t.chat_path(chat_id),
        None => core
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("chats")
            .join(format!("{chat_id}.jsonl")),
    }
}

fn load_transcript(core: &AppCore, topic: Option<&Topic>, chat_id: &str) -> AppResult<Vec<ChatMessage>> {
    let path = transcript_path(core, topic, chat_id);
    let mut msgs = store::read_jsonl::<ChatMessage>(&path)?;
    // 只取最近一段，防止历史无限增长
    const MAX_RESTORE: usize = 400;
    if msgs.len() > MAX_RESTORE {
        msgs.drain(0..msgs.len() - MAX_RESTORE);
    }
    Ok(msgs)
}

fn append_transcript(
    core: &AppCore,
    topic: Option<&Topic>,
    chat_id: &str,
    msg: &ChatMessage,
) -> AppResult<()> {
    let path = transcript_path(core, topic, chat_id);
    store::append_jsonl(&path, msg)
}

/// 整份重写一条对话（分叉时写新文件用；普通追加仍走 [`append_transcript`]）。
pub fn write_transcript(
    core: &AppCore,
    topic: Option<&Topic>,
    chat_id: &str,
    messages: &[ChatMessage],
) -> AppResult<()> {
    let path = transcript_path(core, topic, chat_id);
    store::write_jsonl(&path, messages)
}

/// 读取一段对话记录（给前端展示历史）。
pub fn read_transcript(core: &AppCore, topic_slug: Option<&str>, chat_id: &str) -> AppResult<Vec<ChatMessage>> {
    let topic = resolve_topic_opt(core, topic_slug);
    load_transcript(core, topic.as_ref(), chat_id)
}

/// 某个主题（或工作区首页）的对话记录目录。
pub fn chats_dir_for(core: &AppCore, topic_slug: Option<&str>) -> std::path::PathBuf {
    match resolve_topic_opt(core, topic_slug) {
        Some(t) => t.chats_dir(),
        None => core
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("chats"),
    }
}

/// 列出某个主题下已有的对话 id（按最近修改排序）。
pub fn list_transcripts(core: &AppCore, topic_slug: Option<&str>) -> AppResult<Vec<String>> {
    let dir = chats_dir_for(core, topic_slug);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut items: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                .filter_map(|e| {
                    let stem = e.path().file_stem()?.to_string_lossy().to_string();
                    let modified = e.metadata().ok()?.modified().ok()?;
                    Some((stem, modified))
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(items.into_iter().map(|(s, _)| s).collect())
}

/// 供前端展示的配置摘要（不含密钥）。
pub fn config_summary(cfg: &AppConfig) -> String {
    format!(
        "模型 {}｜权限 {}｜联网 {}｜上下文预算 {} 字",
        cfg.active_profile().map(|p| p.model.clone()).unwrap_or_else(|_| "（未配置）".into()),
        cfg.agent.permission_mode.label(),
        if cfg.agent.allow_web { "开" } else { "关" },
        cfg.agent.context_budget_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::Role;

    #[test]
    fn tool_args_parse_gracefully() {
        assert_eq!(parse_tool_args(""), Value::Object(Default::default()));
        assert_eq!(parse_tool_args("{bad").as_object().unwrap().len(), 0);
        assert_eq!(parse_tool_args("{\"a\":1}")["a"], 1);
    }

    #[test]
    fn accumulator_merges_deltas() {
        let mut acc = Accumulator::default();
        acc.push_tool(0, Some("call_1".into()), Some("fs_read".into()), "{\"pa".into());
        acc.push_tool(0, None, None, "th\":\"a.md\"}".into());
        let calls = acc.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "fs_read");
        assert_eq!(calls[0].input["path"], "a.md");
    }

    #[test]
    fn trimming_drops_whole_turns() {
        let mut msgs = vec![
            ChatMessage::new(Role::User, vec![ContentBlock::text("x".repeat(500))]),
            ChatMessage::assistant_text("y".repeat(500)),
            ChatMessage::new(Role::User, vec![ContentBlock::text("短".to_string())]),
        ];
        let dropped = trim_to_budget(&mut msgs, 200);
        assert!(dropped >= 2);
        // 最后一条用户消息必须保留
        assert!(msgs.last().unwrap().text().contains("短"));
    }
}
