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

/// 每 150ms 看一眼「停止」标志（与流阶段的轮询同节奏）。
///
/// cancel 是个 AtomicBool，没有可 select 的信号量，等待方（审批弹窗、工具执行）
/// 都用这个轮询包装把自己挂进 select。150ms 与流阶段一致：用户点「停止」后
/// 半秒内收尾，感知上就是「立刻」。
async fn wait_cancel(cancel: &AtomicBool) {
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if cancel.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// 深度研究轮的工具循环下限：研究是「计划 → 多轮搜索/阅读 → 反思 → 报告」的
/// 长循环（流程借鉴 dzhng/deep-research 的 breadth/depth 反馈式研究、
/// GPT Researcher 与 stanford-oval/storm 的计划-报告结构，均为 MIT 协议；
/// 只借思路没引代码），常规的每轮上限（默认 8 次往返）跑不完一次像样的研究。
/// 取「不小于 40」而不是固定值：用户手动调大过的上限照常生效。
pub const DEEP_RESEARCH_MIN_ITERATIONS: u32 = 40;

pub fn effective_max_iterations(base: u32, deep_research: bool) -> u32 {
    if deep_research {
        base.max(DEEP_RESEARCH_MIN_ITERATIONS)
    } else {
        base
    }
}

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
    /// 深度研究轮：前端用「/研究 」前缀触发。注入研究协议提示词（计划 → 多轮
    /// 检索与阅读 → 反思补漏 → 带来源的结构化报告），并放宽工具循环轮数上限。
    #[serde(default)]
    pub deep_research: bool,
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
    /// 这轮属于哪条对话。chat_delete / chat_move / chat_fork 靠它判断
    /// 「这条对话还有没有回合正在落库」（那几个动作必须等回合收尾，见 commands/agent.rs）。
    chat_id: String,
    /// 这轮属于哪个主题（工坊回合为 None）。topic_update / topic_delete
    /// 这类主题级整理靠它判断「这个主题下还有没有回合在落库」。
    topic_slug: Option<String>,
}

/// 待审批的工具调用：request_id → (工具名, 决定回执)
type ApprovalSlot = (String, oneshot::Sender<bool>);

/// 审批槽的 Drop 守卫：槽位插进 approvals 表之后交给它保管，无论哪条退出路径
///（用户回执 / 超时 / 用户点「停止」/ 整个工具 future 在 await 点被就地丢弃）
/// 都把条目摘掉。
///
/// 为什么必须用 Drop：摘槽散在各个等待分支里盖不住「丢弃」——工具 future 里
/// 深处 await 的 request_sandbox，在 execute_tool 的 select 因取消而放弃工具时
/// **连同它自己的清理分支一起被丢弃**，而且外层的 wait_cancel 相位恒领先内层
///（早创建早到期），内层先醒的概率趋近于零。以前只有 is_allowed（在丢弃点
/// 之外等待）能靠分支摘干净。respond_approval 先摘的场合 guard 再摘是 no-op。
struct ApprovalSlotGuard<'a> {
    request_id: String,
    slots: &'a Mutex<HashMap<String, ApprovalSlot>>,
}

impl Drop for ApprovalSlotGuard<'_> {
    fn drop(&mut self) {
        self.slots.lock().remove(&self.request_id);
    }
}

pub struct AgentService {
    /// 工具表。MCP 服务器是运行时才连上来的，所以这里要能动态加工具。
    tools: parking_lot::RwLock<ToolRegistry>,
    turns: Mutex<HashMap<String, TurnHandle>>,
    approvals: Mutex<HashMap<String, ApprovalSlot>>,
    /// 本次运行内「始终允许」的工具
    always_allow: Mutex<HashSet<String>>,
    /// 回合注册与「对话文件整理」互斥的门。
    ///
    /// start_turn 的「插句柄 + 落库用户消息」与 chat_delete / chat_move / chat_fork
    /// 的「查在跑 + 搬文件」都要**整段持有**这把锁：两边都没有 await，同步锁就能把
    /// check-then-act 窗口关死——光靠时点检查（beta.3）只能收窄，「删除方检查通过 →
    /// start_turn 插进来并落库 → 文件被搬走」的交错仍会把删除的对话复活成残件。
    gate: parking_lot::Mutex<()>,
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
            gate: parking_lot::Mutex::new(()),
        }
    }

    /// 拿「回合注册 / 对话整理」互斥门（见字段说明）。
    ///
    /// 调用方必须把「检查 + 落库/搬文件」整段包在 guard 作用域里；guard 不许跨
    /// await（这几条路径本来就没有 await，start_turn 与那几个命令都是同步段落）。
    pub fn chat_exclusive(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.gate.lock()
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

        // 拿互斥门到函数结束：句柄登记、主题解析、用户消息落库、工坊侧车是一个
        // 整体，期间 chat_delete / chat_move / chat_fork 不得插进文件操作（见
        // `gate` 字段说明——这些命令也持同一把门）。
        let _gate = self.chat_exclusive();

        // 同一条对话不许同时跑两个回合。前端的 sendInFlight/streaming 只是**本
        // 实例**的状态：远程手机页是同一份 dist 的独立 JS 实例，SSE 断开窗口
        // （息屏、切网是常态）里收不到桌面回合的 TurnStarted，就会在桌面回合
        // 进行中再发一枪——两个 run_turn 并发 load/append 同一份 jsonl，回复与
        // 提问错位，落盘行交错还会把 tool_use/tool_result 的配对拆散，之后
        // load_transcript 把这段历史发出去就是服务商 400。后端是所有实例的
        // 唯一咽喉，守卫必须在这里。
        if self.is_chat_running(&req.chat_id) {
            return Err(AppError::invalid(
                "这条对话还有一轮正在进行（也可能在手机等另一端），等它跑完再发",
            ));
        }

        // 主题先解析：Study 模式带了 slug 但解析失败（主题刚被改名/删除，前端还持
        // 旧 slug）要**报错**，消息不落库。与 run_turn 同口径——以前这里把 NotFound
        // 吞成 None，用户消息静默落进工作区级 `.hub/chats/`（首页日常问答的位置），
        // 而同一请求的回合又报「找不到主题」失败：消息落错位置，这条对话的历史
        // 从此劈成主题目录与工作区两个文件，两份界面各只能读到一半。
        let topic = match req.mode {
            // 工坊没有主题：它的对话也是工作区级的
            AgentMode::Studio => None,
            AgentMode::Study => match req.topic_slug.as_deref() {
                Some(s) if !s.trim().is_empty() => {
                    let workspace = core.workspace();
                    Some(workspace.resolve(s).map_err(|e| {
                        AppError::other(format!(
                            "这条对话所属的主题打不开了（{e}）。它可能刚被改名或删除，\
                             请回到主题列表重新打开后再发。"
                        ))
                    })?)
                }
                _ => None,
            },
        };

        let turn_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.turns.lock().insert(
            turn_id.clone(),
            TurnHandle {
                cancel: cancel.clone(),
                started: Instant::now(),
                // 句柄要在用户消息落库**之前**就位：登记晚了的话，「发出去的瞬间就删」
                // 这类操作能从窗口里钻过去
                chat_id: req.chat_id.clone(),
                // 主题级的整理（改名/删主题）要能问「这个主题下有没有回合在跑」
                topic_slug: topic.as_ref().map(|t| t.slug()),
            },
        );

        // 用户消息先落库，前端拿到 id 后直接渲染，避免重复
        let body = compose_user_text(&text, &req.attachments);
        let user_msg = if image_blocks.is_empty() {
            ChatMessage::user(body)
        } else {
            ChatMessage::user_with_images(&body, image_blocks)
        };
        // 插了句柄之后的两步落库都可能失败（磁盘满 / 权限）；任务还没 spawn，
        // 收尾清理不在后台那段里——失败就得自己把句柄摘掉，否则 turns 表里
        // 永远挂着一个取消不掉的幽灵回合。
        let topic_ref = topic.as_ref();
        if let Err(e) = append_transcript(&core, topic_ref, &req.chat_id, &user_msg) {
            self.turns.lock().remove(&turn_id);
            return Err(e);
        }

        // 工坊的对话要在侧车文件里记下自己的身份，否则侧栏会把它当成首页的日常问答。
        // 这是唯一一个「必须写侧车」的情形（其它字段都是用户改过才写）。
        if req.mode == AgentMode::Studio {
            let dir = crate::agent::chats_dir_for(&core, None);
            let mut meta = crate::agent::chats::load(&dir, &req.chat_id);
            if meta.mode != AgentMode::Studio {
                meta.mode = AgentMode::Studio;
                if let Err(e) = crate::agent::chats::save(&dir, &req.chat_id, &mut meta) {
                    self.turns.lock().remove(&turn_id);
                    return Err(e);
                }
            }
        }

        let svc = self.clone();
        let tid = turn_id.clone();
        let req2 = TurnRequest { text, ..req };
        tokio::spawn(async move {
            let started = Instant::now();
            let result = svc.run_turn(core.clone(), tid.clone(), req2, cancel).await;
            let ms = started.elapsed().as_millis() as u64;
            // 句柄先摘再发事件：run_turn 已经返回，不会再有任何落库，这之后
            // chat_delete / chat_move 就该被放行。反过来的话，「轮次刚结束就删」
            // 会撞上句柄还没摘的窗口，白吃一个「正在进行中」的报错。
            svc.turns.lock().remove(&tid);
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
                chat_id: h.chat_id.clone(),
                started_at_ms: now.duration_since(h.started).as_millis() as i64,
            })
            .collect()
    }

    /// 某个主题（其名下任意对话）是否还有回合正在跑。
    ///
    /// topic_update / topic_delete 动的是整个主题目录：回合里留的快照目录
    /// 消姍后，下一次追加会把目录原样建回来（append_jsonl 开头就是
    /// create_dir_all）——改名把历史从中间勈成两份，删除的主题以半截
    /// 残件复活并被自动认领成新主题。
    pub fn is_topic_running(&self, slug: &str) -> bool {
        self.turns
            .lock()
            .values()
            .any(|h| h.topic_slug.as_deref() == Some(slug))
    }

    /// 某条对话是否还有回合正在跑（落库进行中）。
    ///
    /// 删除 / 搬动 / 分叉一条对话之前必须先问这一句：回合是「每条消息现场追加」的，
    /// 中途把文件拿走的话，下一次追加会在原路径把文件重新建出来——删除的对话
    /// 「复活」成只有尾巴的残件，搬移则在源主题留下同 id 的半截副本。
    pub fn is_chat_running(&self, chat_id: &str) -> bool {
        self.turns.lock().values().any(|h| h.chat_id == chat_id)
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

        let max_iterations = effective_max_iterations(cfg.agent.max_iterations, req.deep_research);

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
        // （两者都要可变：深度研究轮会把协议追加进系统提示词并登记一个版块）
        let (mut system, mut system_sections) = match mode {
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
        // 深度研究：把研究协议追加到系统提示词末尾（含检索工具的可用性与降级
        // 说明），并作为独立版块进「上下文构成」，让用户看得到这部分开销。
        // 可用性在这里判一次（kb_search 是 Study 专属工具且要开着主题；
        // 联网要总闸 + DeepSeek 配置都就绪——两侧判断都和工具表实际给的一致）。
        if req.deep_research {
            let web = cfg.agent.allow_web && cfg.deepseek.effective();
            let kb = matches!(mode, AgentMode::Study) && topic.is_some();
            if !web && !kb {
                // 两侧都不可用：协议会让模型在报告里说明，但用户不该等到
                // 报告写完才知道——这里先给一条能立刻看见的中文提示
                core.toast(
                    "warn",
                    "深度研究：联网搜索没配置（设置 → 联网搜索），当前也没有可检索的本地知识库，\
                     这一轮只能基于模型已有知识作答，结论请自行核实。",
                );
            }
            let protocol = prompt::deep_research_protocol(prompt::ResearchTools {
                web_search: web,
                kb_search: kb,
            });
            system_sections.push(prompt::PromptSectionStat {
                label: "深度研究协议".into(),
                tokens: crate::agent::message::estimate_tokens(&protocol),
            });
            system = format!("{}\n\n{}", system, protocol);
        }

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
        let provider = provider::build(&profile, core.http_fresh()?);
        let ctx = ToolCtx {
            core: core.clone(),
            topic: topic.clone(),
            root,
            mode,
            turn_id: turn_id.clone(),
            chat_id: req.chat_id.clone(),
            cancel: cancel.clone(),
        };

        let mut total_in = 0u32;
        let mut total_out = 0u32;
        // 真实用量（服务商报的那部分）。服务商不报时保持 0，界面按「无数据」处理。
        let mut total_cached = 0u32;
        let mut total_cache_write = 0u32;
        let mut reason = "stop".to_string();

        for iteration in 1..=max_iterations.max(1) {
            if cancel.load(Ordering::SeqCst) {
                return Err(AppError::Cancelled);
            }
            core.emit_agent(&AgentEvent::Iteration {
                turn_id: turn_id.clone(),
                index: iteration,
                max: max_iterations,
            });

            trim_to_budget(&mut messages, cfg.agent.context_budget_chars);

            let message_id = uuid::Uuid::new_v4().to_string();
            // 网络抖动自动重连：请求失败且属于「临时性错误」时原样再发，上限由设置
            // 控制（0 = 关闭）。断线前已经吐到界面上的半截话要先擦掉（StreamReset），
            // 否则重连后的增量会接在旧文本后面出现重复；这轮还没有任何东西落库，
            // 所以整个重来是安全的。
            let mut acc;
            let mut attempt: u32 = 0;
            loop {
                attempt += 1;
                let request = ChatRequest {
                    model: profile.model.clone(),
                    system: system.clone(),
                    messages: messages.clone(),
                    tools: specs.clone(),
                    temperature: profile.temperature,
                    max_tokens: profile.max_tokens,
                    reasoning: profile.reasoning,
                    cancel: cancel.clone(),
                    attachments: attachments.clone(),
                    supports_vision: profile.supports_vision,
                };
                let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
                let provider_task = {
                    let provider = provider.clone();
                    tokio::spawn(async move { provider.stream(request, tx).await })
                };

                let mut streamed = false;
                acc = Accumulator::default();
                // 消费事件的同时每 150ms 查一次「停止」：服务商首 token 迟迟不来
                // 或流中途卡住时，纯被动等下一个事件会让「停止」按钮要等满整个
                // 请求超时（默认 180s）才生效——这正是用户最想立刻停下来的场景。
                let outcome = loop {
                    tokio::select! {
                        biased;
                        ev = rx.recv() => match ev {
                            Some(StreamEvent::Usage(u)) => acc.usage.merge(u),
                            Some(StreamEvent::Text(t)) => {
                                acc.text.push_str(&t);
                                streamed = true;
                                core.emit_agent(&AgentEvent::Delta {
                                    turn_id: turn_id.clone(),
                                    message_id: message_id.clone(),
                                    text: t,
                                    thinking: false,
                                });
                            }
                            Some(StreamEvent::Thinking { index, text }) => {
                                acc.thinking_part(index).text.push_str(&text);
                                streamed = true;
                                core.emit_agent(&AgentEvent::Delta {
                                    turn_id: turn_id.clone(),
                                    message_id: message_id.clone(),
                                    text,
                                    thinking: true,
                                });
                            }
                            Some(StreamEvent::ToolCall { index, id, name, args }) => {
                                acc.push_tool(index, id, name, args);
                            }
                            Some(StreamEvent::ThinkingSignature { index, sig }) => {
                                acc.thinking_part(index).signature = Some(sig);
                            }
                            // Anthropic 的 redacted_thinking：服务端把这段思考隐去了，
                            // 只给一段加密 data。不展示（没有明文），但要攒下来落库并
                            // 原样回传——丢了之后这条对话在思考模式下会被 400 卡死。
                            Some(StreamEvent::RedactedThinking { index, data }) => {
                                acc.redacted.push((index, data));
                            }
                            Some(StreamEvent::Finish(r)) => acc.finish = Some(r),
                            // 通道关闭 = provider 任务结束
                            None => break provider_task.await,
                        },
                        _ = tokio::time::sleep(Duration::from_millis(150)) => {
                            if cancel.load(Ordering::SeqCst) {
                                // 流停滞时任务自己醒不过来（要等下一个 chunk 才会看
                                // cancel 标志），直接 abort 掉
                                provider_task.abort();
                                break Ok(Err(AppError::Cancelled));
                            }
                        }
                    }
                };
                let retryable = match &outcome {
                    Ok(Ok(())) | Ok(Err(AppError::Cancelled)) | Err(_) => false,
                    Ok(Err(e)) => is_transient(e),
                };
                if retryable && attempt <= cfg.agent.reconnect_max_attempts {
                    if streamed {
                        core.emit_agent(&AgentEvent::StreamReset { turn_id: turn_id.clone() });
                    }
                    // 退避着等：0.5s 起步，封顶 5s；等的时候用户可能点了「停止」
                    let wait = Duration::from_millis((500 * attempt as u64).min(5000));
                    tokio::time::sleep(wait).await;
                    if cancel.load(Ordering::SeqCst) {
                        return Err(AppError::Cancelled);
                    }
                    core.toast(
                        "info",
                        format!(
                            "模型连接中断，自动重连（第 {attempt}/{} 次）",
                            cfg.agent.reconnect_max_attempts
                        ),
                    );
                    continue;
                }

                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(AppError::Cancelled)) => return Err(AppError::Cancelled),
                    Ok(Err(e)) => return Err(e),
                    Err(join_err) => {
                        return Err(AppError::other(format!("模型任务异常终止：{join_err}")));
                    }
                }
                break;
            }

            let tool_calls = acc.tool_calls();
            let mut blocks: Vec<ContentBlock> = Vec::new();
            // 思考块按原顺序逐块落库，各带各的签名（没有签名的跳过：带不上
            // 有效签名的思考块回放只会被服务商拒收）。redacted_thinking 没有
            // 明文与签名，但官方要求**原样传回**——按内容块序号与普通思考块
            // 合并排序，保持服务商给出的原始顺序。
            let mut thought_idx: Vec<usize> = acc.thinking.iter().map(|(i, _)| *i).collect();
            thought_idx.extend(acc.redacted.iter().map(|(i, _)| *i));
            thought_idx.sort_unstable();
            thought_idx.dedup();
            for i in thought_idx {
                if let Some((_, part)) = acc.thinking.iter().find(|(j, _)| *j == i) {
                    if part.text.trim().is_empty() {
                        continue;
                    }
                    blocks.push(ContentBlock::Thinking {
                        text: part.text.clone(),
                        signature: part.signature.clone(),
                    });
                } else if let Some((_, data)) = acc.redacted.iter().find(|(j, _)| *j == i) {
                    blocks.push(ContentBlock::RedactedThinking { data: data.clone() });
                }
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
                + acc
                    .thinking
                    .iter()
                    .map(|(_, p)| crate::agent::message::estimate_tokens(&p.text))
                    .sum::<u32>();
            let real = acc.usage;
            // 报了总量但缺一半（Anthropic 形网关只在收尾时报 output、或
            // prompt_tokens 报 0）的那一半退回本地估算——「没报」不能落成假 0，
            // 界面浮层会把「输入 0」当真数字显示
            let (in_tokens, out_tokens) = if real.is_empty() {
                (est_in, est_out)
            } else {
                (
                    if real.input_tokens == 0 { est_in } else { real.input_tokens },
                    if real.output_tokens == 0 { est_out } else { real.output_tokens },
                )
            };
            total_in += in_tokens;
            total_out += out_tokens;
            total_cached += real.cached_tokens;
            total_cache_write += real.cache_write_tokens;

            let mut assistant = ChatMessage::assistant(blocks);
            assistant.meta.model = Some(profile.model.clone());
            // 前端据此给报告（该轮最后一条 assistant）挂「存为笔记」入口
            if req.deep_research {
                assistant.meta.deep_research = Some(true);
            }
            assistant.meta.input_tokens = Some(in_tokens);
            assistant.meta.output_tokens = Some(out_tokens);
            if !real.is_empty() {
                // 缓存明细只在**真的报了**时才落：没报落 None（界面上那一轮就不
                // 计入命中率），以前落 Some(0) 会被当成「命中 0%」把整条对话的
                // 命中率假性拉低（Ollama / vLLM / 不透传明细的网关全中）
                assistant.meta.cached_tokens = real.cache_reported.then_some(real.cached_tokens);
                assistant.meta.cache_write_tokens =
                    real.cache_reported.then_some(real.cache_write_tokens);
            }
            messages.push(assistant.clone());
            append_transcript(&core, topic.as_ref(), &req.chat_id, &assistant)?;
            core.emit_agent(&AgentEvent::Message {
                turn_id: turn_id.clone(),
                chat_id: req.chat_id.clone(),
                message: assistant,
            });

            if tool_calls.is_empty() {
                reason = acc.finish.clone().unwrap_or_else(|| "stop".into());
                break;
            }
            reason = "tool_calls".into();

            // 顺序执行工具：学习场景下工具之间有依赖（先查再写），并行反而容易冲突
            for (call_idx, tc) in tool_calls.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    // 用户点了「停止」：给剩余的工具调用补上「已中止」的结果再退出。
                    // tool_use 与 tool_result 必须成对落库——配对被切断的话，这条对话
                    // 之后每一次请求都会被服务商 400（历史里有 tool_use 没有回应）。
                    for rest in &tool_calls[call_idx..] {
                        let tool_msg = ChatMessage::tool_result_with(
                            rest.id.clone(),
                            "已被用户中止".to_string(),
                            true,
                            Vec::new(),
                        );
                        messages.push(tool_msg.clone());
                        append_transcript(&core, topic.as_ref(), &req.chat_id, &tool_msg)?;
                    }
                    return Err(AppError::Cancelled);
                }
                let result = self.execute_tool(&core, &ctx, &turn_id, tc, &cancel).await;
                let (content, is_error, images) = match result {
                    Ok(o) => (o.content, o.is_error, o.images),
                    Err(e) => (format!("工具执行失败：{e}"), true, Vec::new()),
                };
                let tool_msg = ChatMessage::tool_result_with(tc.id.clone(), content, is_error, images);
                messages.push(tool_msg.clone());
                append_transcript(&core, topic.as_ref(), &req.chat_id, &tool_msg)?;
            }

            if iteration >= max_iterations {
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
    ///
    /// `cancel` 是这一轮的停止标志：等待审批与工具执行两个阶段都会盯着它
    /// （见下面两处的 select）——否则「停止」在流阶段立刻生效、到了工具阶段
    /// 却要干等审批超时（最长 600s）或工具自然结束（run_command 最长 600s），
    /// 用户看到的就是「点了停止没反应」。
    async fn execute_tool(
        &self,
        core: &Arc<AppCore>,
        ctx: &ToolCtx,
        turn_id: &str,
        tc: &ToolCall,
        cancel: &Arc<AtomicBool>,
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
                    images: Vec::new(),
                },
            });
            return Ok(registry::ToolOutput::err(msg));
        };

        let summary = tool.summarize(&tc.input);
        let risk = tool.risk();

        // --- 审批 ---
        if !self.is_allowed(core, turn_id, tc, &summary, risk, cancel).await {
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
                    images: Vec::new(),
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
        // 工具执行本身感知不到「停止」（Tool::run 没有 cancel 通道，ToolCtx 也不带），
        // 在这里包一层轮询：点了停止就把工具 future 就地丢弃——run_command / MCP
        // 调用这类长工具不再拖满自己的超时（最长 600 秒）才收尾。被丢弃的工具可能
        // 留下半截副作用（落盘类工具走 atomic_write，最多多一个临时文件）；
        // tool_result 以「已被用户中止」落库，tool_use/tool_result 配对完好，
        // 回合在下一个工具开头的检查点补齐剩余调用并正常收尾。
        let fut = tool.run(ctx, tc.input.clone());
        tokio::pin!(fut);
        let outcome = loop {
            tokio::select! {
                biased;
                out = &mut fut => break out,
                _ = wait_cancel(cancel) => {
                    break Ok(registry::ToolOutput::err("已被用户中止"));
                }
            }
        };
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
                images: o.images.clone(),
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
                images: Vec::new(),
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
    #[allow(clippy::too_many_arguments)]
    pub async fn request_sandbox(
        &self,
        core: &Arc<AppCore>,
        turn_id: &str,
        path: &std::path::Path,
        mode: &str,
        reason: &str,
        cancel: &AtomicBool,
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
        // 摘槽交给守卫：这个 future 嵌在工具 run 的深处，execute_tool 在取消时
        // 会把工具 future **连同这里的清理分支一起丢弃**——靠分支摘槽盖不住
        // 丢弃路径（「沙箱弹窗开着时点停止」泄漏槽位的根因），Drop 在被丢弃的
        // future 里照样执行。
        let _slot = ApprovalSlotGuard { request_id: request_id.clone(), slots: &self.approvals };

        core.emit_agent(&AgentEvent::SandboxRequest {
            turn_id: turn_id.to_string(),
            request_id: request_id.clone(),
            path: path.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            mode: mode.to_string(),
            reason: reason.to_string(),
            pre_approved: false,
        });

        // 等待也要盯着「停止」（与 is_allowed 同一套）。摘槽不在这里做：
        // guard 兜住包括「被丢弃」在内的所有退出路径。
        let allowed = tokio::select! {
            biased;
            _ = wait_cancel(cancel) => false,
            r = tokio::time::timeout(APPROVAL_TIMEOUT, rx) => match r {
                Ok(Ok(allow)) => allow,
                _ => false,
            },
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
    let provider = provider::build(&profile, core.http_fresh()?);
    let max_attempts = cfg.agent.reconnect_max_attempts;

    let request = ChatRequest {
        model: profile.model.clone(),
        system: system.to_string(),
        messages: vec![ChatMessage::user(user)],
        tools: Vec::new(),
        temperature: 0.2,
        max_tokens,
        // **不**带档案的思考强度：判分/抽取这类一次性结构化输出不需要思考，而
        // Anthropic 的思考预算计入 max_tokens——判分入口传 4096 时思考被夹到
        // 4095、正文只剩极限空间，回复近空 → extract_json 报「没有按要求返回
        // JSON」，判分必失败（profile_test 同款坑，那边连 ReasoningConfig 都换了默认值）。
        reasoning: crate::config::ReasoningConfig::default(),
        cancel: Arc::new(AtomicBool::new(false)),
        // 这条路径只做一次结构化输出，不带图片
        attachments: Attachments::disabled(),
        supports_vision: false,
    };

    // 与对话主循环同一套重连策略；这里不流式上屏，重试只需要把已收的文本丢掉重来
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
        let task = {
            let provider = provider.clone();
            let request = request.clone();
            tokio::spawn(async move { provider.stream(request, tx).await })
        };
        let mut out = String::new();
        while let Some(ev) = rx.recv().await {
            if let StreamEvent::Text(t) = ev {
                out.push_str(&t);
            }
        }
        let outcome = task.await;
        let retryable = match &outcome {
            Ok(Ok(())) | Ok(Err(AppError::Cancelled)) | Err(_) => false,
            Ok(Err(e)) => is_transient(e),
        };
        if retryable && attempt <= max_attempts {
            let wait = std::time::Duration::from_millis((500 * attempt as u64).min(5000));
            tokio::time::sleep(wait).await;
            continue;
        }
        return match outcome {
            Ok(Ok(())) => Ok(out),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(AppError::other(format!("模型调用异常：{e}"))),
        };
    }
}

/// 这类错误值得自动重连：网络层抖动（连接被重置 / 超时 / TLS 失败 / 流中途断掉）
/// 与服务端临时过载（429 限流、5xx、Anthropic 的 overloaded）。
/// 鉴权失败、参数不对重试多少次结果都一样，不在重连之列。
fn is_transient(e: &AppError) -> bool {
    match e {
        AppError::Http(_) => true,
        AppError::Provider(msg) => {
            if msg.contains(provider::STREAM_CUT) {
                return true;
            }
            if let Some(rest) = msg.strip_prefix("HTTP ") {
                let code: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                return code == "408" || code == "429" || code.starts_with('5');
            }
            msg.to_lowercase().contains("overloaded")
        }
        _ => false,
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
    ///
    /// `cancel`：这一轮的停止标志。等待审批的 select 里必须有这个分支——
    /// 不然用户在弹窗开着时点「停止」，回合要干等 APPROVAL_TIMEOUT（600 秒）
    /// 超时按拒绝处理才收尾，侧栏转圈不停、「停止」看上去点了没反应。
    /// 取消按**拒绝**处理（回合随后的工具循环检查点会补齐 tool_result 并收尾），
    /// 同时把弹窗从审批表里摘掉，之后用户再点弹窗会落空（前端本来就会收起它）。
    async fn is_allowed(
        &self,
        core: &Arc<AppCore>,
        turn_id: &str,
        tc: &ToolCall,
        summary: &str,
        risk: Risk,
        cancel: &Arc<AtomicBool>,
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
        // 摘槽交给守卫：任何退出路径（回执/超时/取消）都兜得住
        let _slot = ApprovalSlotGuard { request_id: request_id.clone(), slots: &self.approvals };

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

        let decision = tokio::select! {
            biased;
            // 用户点了「停止」：立刻按拒绝处理，别干等超时（见函数说明）
            _ = wait_cancel(cancel) => false,
            r = rx => match r {
                Ok(allow) => allow,
                // 回执端没了（slot 被别的路径摘掉）：按拒绝处理
                Err(_) => false,
            },
            _ = tokio::time::sleep(APPROVAL_TIMEOUT) => false,
        };
        decision
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
                ContentBlock::Thinking { text, .. } => text.chars().count(),
                ContentBlock::RedactedThinking { data } => data.chars().count(),
                ContentBlock::ToolUse { input, name, .. } => name.chars().count() + input.to_string().chars().count(),
                ContentBlock::ToolResult { content, images, .. } => {
                    content.chars().count()
                        // 工具附带的图（PDF 页截图）和用户贴图一样占 token，
                        // 按 4000 字符折算，否则看了几十页图的对话永远裁不掉
                        + images.len() * 4_000
                }
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

/// 一段思考块：Anthropic 开交错思考时一条流里有多个块，每块有**自己的**
/// 签名——混成一段再配最后一个签名，回放时签名校验必失败。
#[derive(Default)]
struct ThinkingPart {
    text: String,
    signature: Option<String>,
}

#[derive(Default)]
struct Accumulator {
    text: String,
    /// 思考块按内容块序号分开攒（顺序 = 首次出现的顺序）
    thinking: Vec<(usize, ThinkingPart)>,
    /// Anthropic 的 redacted_thinking 块：没有 delta，内容在 content_block_start
    /// 一次给全（加密 data）。与 thinking 一样按内容块序号攒，落库时按序号合并。
    redacted: Vec<(usize, String)>,
    tools: Vec<ToolCall>,
    finish: Option<String>,
    /// 服务商在这条流里报的真实用量（可能为空）
    usage: provider::ProviderUsage,
}

impl Accumulator {
    fn thinking_part(&mut self, index: usize) -> &mut ThinkingPart {
        if !self.thinking.iter().any(|(i, _)| *i == index) {
            self.thinking.push((index, ThinkingPart::default()));
        }
        &mut self.thinking.iter_mut().find(|(i, _)| *i == index).expect("刚插入过").1
    }
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
                ContentBlock::ToolResult { content, images, .. } => {
                    tool_tokens += est(content);
                    tool_tokens += images
                        .iter()
                        .map(|i| crate::agent::attachment::estimate_image_tokens(i.width, i.height))
                        .sum::<u32>();
                }
                ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => {
                    msg_tokens += est(text)
                }
                // 加密思考块按明文同量级估（它的 token 数与解出来的思考等量）
                ContentBlock::RedactedThinking { data } => msg_tokens += est(data),
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
    // chat_id 由前端传进来、直接拼进文件名：带分隔符或 `..` 就能在工作区外
    // 造文件。正常 id 都是 uuid；非法的一律退到 chats 目录里的固定兜底名
    //（写得进但撞不出工作区，且多个非法 id 只会混在这一个文件里）。
    let safe = crate::paths::is_safe_slug(chat_id);
    match topic {
        Some(t) if safe => t.chat_path(chat_id),
        Some(t) => t.dir.join(".hub").join("chats").join("illegal-chat-id.jsonl"),
        None if safe => core
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("chats")
            .join(format!("{chat_id}.jsonl")),
        None => core
            .workspace()
            .root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("chats")
            .join("illegal-chat-id.jsonl"),
    }
}

fn load_transcript(core: &AppCore, topic: Option<&Topic>, chat_id: &str) -> AppResult<Vec<ChatMessage>> {
    let path = transcript_path(core, topic, chat_id);
    let mut msgs = store::read_jsonl::<ChatMessage>(&path)?;
    // 只取最近一段，防止历史无限增长。截断点必须落在 user 消息上——
    // 按条数硬切会把历史切成「以 tool/assistant 开头」或拆散
    // tool_use/tool_result 的配对，服务商对这两种形状都直接 400。
    const MAX_RESTORE: usize = 400;
    if msgs.len() > MAX_RESTORE {
        let cut = msgs.len() - MAX_RESTORE;
        let first_user = msgs
            .iter()
            .skip(cut)
            .position(|m| m.role == crate::agent::message::Role::User)
            .map(|p| cut + p);
        match first_user {
            Some(p) => {
                msgs.drain(0..p);
            }
            // 截断范围内连一条 user 都没有（不可能的正常对话）：保底全保留
            None => {}
        }
    }
    // 崩溃/强杀留下的配对孤儿在这里清掉：append 不是原子的，assistant(tool_use)
    // 落库与 tool_result 落库之间隔着审批等待与工具执行（可达数十秒），进程死在
    // 中间就是一条「有 tool_use 没回应」的残尾——之后这条对话**每一次**请求都被
    // 服务商 400，且没有任何自愈路径（历史不变、重发同样 400）。读历史与发请求
    // 走同一条路，所以界面看到的也是修复后的形状；盘上文件不动（坏行旁边本就有
    // .bad 留底，历史保持「只追加」的简单性）。
    drop_orphan_tool_pairs(&mut msgs);
    Ok(msgs)
}

/// 清掉历史里「tool_use / tool_result 配对断了」的孤儿。两条规则：
///
/// - **有 tool_use 没回应**：把没被任何 tool_result 回应的 tool_use 块从它的
///   assistant 消息里摘掉，摘空了就丢掉整条 assistant——宁可少一个工具调用，
///   也不能整条对话被 400；
/// - **有 tool_result 没有对应的 tool_use**：整行丢掉。
///
/// 只清内存副本，不写盘。
pub fn drop_orphan_tool_pairs(messages: &mut Vec<ChatMessage>) {
    use crate::agent::message::Role;
    // 第一遍：收集历史里出现过的全部 tool_use id 与已被回应的 tool_use id
    let mut all_uses: std::collections::HashSet<String> = Default::default();
    let mut answered: std::collections::HashSet<String> = Default::default();
    for m in messages.iter() {
        for b in &m.blocks {
            if let ContentBlock::ToolUse { id, .. } = b {
                all_uses.insert(id.clone());
            }
        }
        if m.role == Role::Tool {
            for b in &m.blocks {
                if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                    answered.insert(tool_use_id.clone());
                }
            }
        }
    }
    // 第二遍：摘掉没被回应的 tool_use（assistant 空了就丢），丢掉孤儿 tool_result
    let mut kept: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for mut m in messages.drain(..) {
        if m.role == Role::Assistant {
            m.blocks.retain(|b| match b {
                ContentBlock::ToolUse { id, .. } => answered.contains(id),
                _ => true,
            });
            if m.blocks.is_empty() {
                continue;
            }
        } else if m.role == Role::Tool {
            let orphan = m.blocks.iter().all(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => !all_uses.contains(tool_use_id),
                _ => false,
            });
            if orphan {
                continue;
            }
        }
        kept.push(m);
    }
    *messages = kept;
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

    /// 进程死在「assistant(tool_use) 落库」与「tool_result 落库」之间会留下配对
    /// 孤儿，之后这条对话每次请求都被服务商 400。清孤儿的三条规则分别钉住：
    /// 没回应的 tool_use 摘块（摘空丢整条）、孤儿 tool_result 丢行、正常配对不动。
    #[test]
    fn orphan_tool_pairs_are_dropped() {
        let use_a = || ContentBlock::ToolUse {
            id: "A".into(),
            name: "viewer_read".into(),
            input: serde_json::json!({}),
        };
        let use_b = || ContentBlock::ToolUse {
            id: "B".into(),
            name: "run_command".into(),
            input: serde_json::json!({}),
        };
        // 形状一：A 有回应、B 没有（进程死在两个工具之间）——
        // B 从 assistant 里摘掉，A 与它的结果原样保留
        let mut msgs = vec![
            ChatMessage::user("帮我查"),
            ChatMessage::assistant(vec![use_a(), use_b()]),
            ChatMessage::tool_result("A", "结果", false),
        ];
        drop_orphan_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[1].blocks[0], ContentBlock::ToolUse { ref id, .. } if id == "A"));
        assert_eq!(msgs[1].blocks.len(), 1, "没被回应的 B 要被摘掉");

        // 形状二：整条 assistant 只有没被回应的 tool_use（进程死在工具调用后）——
        // 摘空了连整条一起丢，历史收在上一轮的 tool_result 上（合法形状）
        let mut msgs = vec![
            ChatMessage::user("帮我查"),
            ChatMessage::assistant(vec![use_a()]),
            ChatMessage::tool_result("A", "结果", false),
            ChatMessage::assistant(vec![use_b()]),
        ];
        drop_orphan_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 3, "尾端孤儿 assistant 要整条丢掉");
        assert!(matches!(&msgs[2].blocks[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "A"));

        // 形状三：孤儿 tool_result（对应的 tool_use 根本不在历史里）——整行丢
        let mut msgs = vec![
            ChatMessage::user("帮我查"),
            ChatMessage::tool_result("幽灵", "结果", false),
        ];
        drop_orphan_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 1);

        // 正常配对一动不动
        let mut msgs = vec![
            ChatMessage::user("帮我查"),
            ChatMessage::assistant(vec![use_a()]),
            ChatMessage::tool_result("A", "结果", false),
        ];
        drop_orphan_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn tool_args_parse_gracefully() {
        assert_eq!(parse_tool_args(""), Value::Object(Default::default()));
        assert_eq!(parse_tool_args("{bad").as_object().unwrap().len(), 0);
        assert_eq!(parse_tool_args("{\"a\":1}")["a"], 1);
    }

    /// 深度研究轮的循环上限：只放宽到「不小于 40」，用户手动调大过的上限照常生效；
    /// 普通轮原样返回——这个函数是「让研究跑得完」的唯一开关，别在主循环里再写特例。
    #[test]
    fn deep_research_relaxes_iteration_cap() {
        assert_eq!(effective_max_iterations(8, false), 8, "普通轮不动");
        assert_eq!(effective_max_iterations(8, true), DEEP_RESEARCH_MIN_ITERATIONS);
        assert_eq!(effective_max_iterations(120, true), 120, "用户调大过的上限优先");
        assert_eq!(effective_max_iterations(0, true), DEEP_RESEARCH_MIN_ITERATIONS);
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
