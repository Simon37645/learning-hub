//! agent → 前端的事件协议。统一从 `hub://agent` 频道发出，前端按 `kind` 分派。

use crate::agent::message::ChatMessage;
use crate::config::PermissionMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// 读：看文件、搜网页、查卡片
    Read,
    /// 写：新建/修改笔记、卡片、任务
    Write,
    /// 破坏：删除、覆盖、批量改写
    Destructive,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Read => "读取",
            Risk::Write => "写入",
            Risk::Destructive => "删除/覆盖",
        }
    }

    /// 给定权限模式，这个风险等级是否需要用户点头。
    pub fn needs_approval(self, mode: PermissionMode) -> bool {
        match mode {
            PermissionMode::Full => false,
            PermissionMode::AutoEdit => self == Risk::Destructive,
            PermissionMode::Ask => self != Risk::Read,
        }
    }
}

/// 一次等待审批的工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCall {
    pub request_id: String,
    pub call_id: String,
    pub name: String,
    /// 人类可读的摘要，例如「写入 notes/特征值.md」
    pub summary: String,
    pub risk: Risk,
    pub risk_label: String,
    pub input: serde_json::Value,
}

/// 工具执行结果预览（用于聊天气泡里的工具卡片）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcomeView {
    pub call_id: String,
    pub name: String,
    pub summary: String,
    pub risk: Risk,
    pub ok: bool,
    /// 结果前若干字符
    pub preview: String,
    pub duration_ms: u64,
    /// 这条工具调用是否被拒绝
    pub denied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStarted {
        turn_id: String,
        chat_id: String,
        topic_slug: Option<String>,
        message_id: String,
        mode: PermissionMode,
    },
    /// 流式增量。`thinking` 为真时是模型的思考内容。
    Delta {
        turn_id: String,
        message_id: String,
        text: String,
        thinking: bool,
    },
    /// 一条完整消息落库（助手回复或工具结果）
    Message {
        turn_id: String,
        message: ChatMessage,
    },
    Iteration {
        turn_id: String,
        index: u32,
        max: u32,
    },
    ToolApproval {
        turn_id: String,
        call: PendingCall,
    },
    /// 越权申请：agent 想碰工作区之外的文件
    SandboxRequest {
        turn_id: String,
        request_id: String,
        /// agent 想访问的具体路径
        path: String,
        /// 拟授权的上级目录（批准一次，整个目录有效）
        root: String,
        /// read / write / delete
        mode: String,
        /// 说明它想干什么
        reason: String,
        /// 是否已在本次会话里被批准过（前端可据此直接放行）
        pre_approved: bool,
    },
    ToolStarted {
        turn_id: String,
        call_id: String,
        name: String,
        summary: String,
        risk: Risk,
    },
    ToolFinished {
        turn_id: String,
        outcome: ToolOutcomeView,
    },
    Usage {
        turn_id: String,
        input_tokens: u32,
        output_tokens: u32,
    },
    Finished {
        turn_id: String,
        reason: String,
        duration_ms: u64,
        interrupts: u32,
    },
    Failed {
        turn_id: String,
        message: String,
    },
}

impl AgentEvent {
    pub fn turn_id(&self) -> &str {
        match self {
            AgentEvent::TurnStarted { turn_id, .. }
            | AgentEvent::Delta { turn_id, .. }
            | AgentEvent::Message { turn_id, .. }
            | AgentEvent::Iteration { turn_id, .. }
            | AgentEvent::ToolApproval { turn_id, .. }
            | AgentEvent::SandboxRequest { turn_id, .. }
            | AgentEvent::ToolStarted { turn_id, .. }
            | AgentEvent::ToolFinished { turn_id, .. }
            | AgentEvent::Usage { turn_id, .. }
            | AgentEvent::Finished { turn_id, .. }
            | AgentEvent::Failed { turn_id, .. } => turn_id,
        }
    }
}

pub const EVENT_AGENT: &str = "hub://agent";
pub const EVENT_VIEWER: &str = "hub://viewer";
pub const EVENT_TOPICS: &str = "hub://topics";
pub const EVENT_TOAST: &str = "hub://toast";

/// 查看器事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewerEvent {
    /// 全量状态（打开/关闭/激活后前端直接覆盖本地状态）
    Sync {
        snapshot: crate::viewer::ViewerSnapshot,
    },
    /// 请求前端把当前文档文本交上来
    SnapshotRequest {
        tab_id: String,
    },
    /// 让前端跳到指定页/锚点/百分比
    Goto {
        tab_id: String,
        page: Option<u32>,
        scroll: Option<f32>,
        anchor: Option<String>,
        highlight: Option<String>,
    },
    /// 让前端重新加载某个标签
    Reload {
        tab_id: String,
    },
    /// 前端状态回写后通知（标签标题变化等）
    Updated {
        tab: crate::viewer::TabView,
    },
}

pub const VIEWER_EVENT: &str = "hub://viewer";

/// 主题集合发生变化（新建、改名、统计更新），前端据此刷新侧栏。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopicsEvent {
    Created { slug: String, name: String },
    Updated { slug: String },
    Deleted { slug: String },
    /// 让前端整体重扫
    Refresh,
}
