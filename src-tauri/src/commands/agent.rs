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

/// 侧栏「对话」列表用的一项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatOverviewItem {
    pub id: String,
    /// 第一条用户消息的前 40 字，当列表标题（还没聊过就是空串）
    pub title: String,
    pub messages: usize,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// 某个主题下的历史对话清单（侧栏用）。
///
/// 对话文件里只有 id 和消息，光看 id 没法挑——所以这里读一遍 jsonl，
/// 把「第一句话」当标题，按最近使用排序。
#[tauri::command]
pub async fn chat_overview(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<Vec<ChatOverviewItem>> {
    let core = state.0.clone();
    let dir = crate::agent::chats_dir_for(&core, topic_slug.as_deref());
    let ids = crate::agent::list_transcripts(&core, topic_slug.as_deref())?;
    let mut out = Vec::new();
    for id in ids.into_iter().take(60) {
        let path = dir.join(format!("{id}.jsonl"));
        let messages = crate::store::read_jsonl::<ChatMessage>(&path).unwrap_or_default();
        let title = messages
            .iter()
            .find(|m| m.role == crate::agent::message::Role::User)
            .map(|m| crate::agent::provider::truncate(m.text().trim(), 40))
            .unwrap_or_default();
        let updated_at = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(chrono::DateTime::<chrono::Utc>::from)
            .unwrap_or_else(|_| crate::store::now());
        out.push(ChatOverviewItem {
            id,
            title,
            messages: messages.len(),
            updated_at,
        });
    }
    Ok(out)
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
