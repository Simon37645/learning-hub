//! 对话相关命令：发消息、中止、审批、读历史。

use crate::agent::message::ChatMessage;
use crate::agent::TurnRequest;
use crate::error::AppResult;
use crate::state::AppState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub turn_id: String,
    pub chat_id: String,
    /// 落库后的用户消息（带正式 id，前端据此渲染）
    pub message: ChatMessage,
}

#[tauri::command]
pub async fn agent_send(state: State<'_, AppState>, req: TurnRequest) -> AppResult<SendResult> {
    let core = state.0.clone();
    let chat_id = req.chat_id.clone();
    let (turn_id, message) = core.agent.start_turn(core.clone(), req)?;
    Ok(SendResult { turn_id, chat_id, message })
}

#[tauri::command]
pub async fn agent_cancel(state: State<'_, AppState>, turn_id: String) -> AppResult<bool> {
    Ok(state.0.agent.cancel(&turn_id))
}

/// 回应工具审批弹窗。`always=true` 表示本次运行内一直允许这个工具。
#[tauri::command]
pub async fn agent_approve(
    state: State<'_, AppState>,
    request_id: String,
    allow: bool,
    always: Option<bool>,
) -> AppResult<bool> {
    Ok(state
        .0
        .agent
        .respond_approval(&request_id, allow, always.unwrap_or(false)))
}

#[tauri::command]
pub async fn agent_transcript(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
    chat_id: String,
) -> AppResult<Vec<ChatMessage>> {
    crate::agent::read_transcript(&state.0, topic_slug.as_deref(), &chat_id)
}

#[tauri::command]
pub async fn agent_chats(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<Vec<String>> {
    crate::agent::list_transcripts(&state.0, topic_slug.as_deref())
}

/// 生成一个新对话 id（不落盘，等第一条消息写入时才产生文件）。
#[tauri::command]
pub async fn agent_new_chat() -> AppResult<String> {
    Ok(uuid::Uuid::new_v4().to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningTurn {
    pub turn_id: String,
    pub running_ms: i64,
}

#[tauri::command]
pub async fn agent_running(state: State<'_, AppState>) -> AppResult<Vec<RunningTurn>> {
    Ok(state
        .0
        .agent
        .running_turns()
        .into_iter()
        .map(|t| RunningTurn { turn_id: t.turn_id, running_ms: t.started_at_ms })
        .collect())
}
