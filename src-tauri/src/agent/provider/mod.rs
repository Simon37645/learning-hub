//! 模型接入层：把「对话」翻译成各家 API 的请求，再把流式响应翻译成统一的 [`StreamEvent`]。
//!
//! 新增服务商只需要实现 [`LlmProvider`]，agent 主循环完全不用动。

pub mod anthropic;
pub mod openai;

use crate::agent::message::ChatMessage;
use crate::agent::registry::ToolSpec;
use crate::config::{ProviderKind, ProviderProfile};
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    /// 系统提示词（Anthropic 放在顶层参数，OpenAI 放在 messages[0]）
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// 单次请求超时
    pub timeout: std::time::Duration,
    /// 用户点「停止」时置位，provider 在流循环里检查
    pub cancel: Arc<AtomicBool>,
}

/// 服务商返回的流式增量（尚未累积成完整消息）。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    Thinking(String),
    /// 工具调用的增量片段，按 `index` 归并
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        args: String,
    },
    /// 结束原因：stop / tool_calls / length …
    Finish(String),
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn label(&self) -> String;
    /// 流式请求。返回 `Err(AppError::Cancelled)` 表示被用户中止。
    async fn stream(&self, req: ChatRequest, tx: UnboundedSender<StreamEvent>) -> AppResult<()>;
}

/// 按档案构造 provider。
pub fn build(profile: &ProviderProfile, http: reqwest::Client) -> Arc<dyn LlmProvider> {
    match profile.kind {
        ProviderKind::OpenAi => Arc::new(openai::OpenAiProvider::new(profile, http)),
        ProviderKind::Anthropic => Arc::new(anthropic::AnthropicProvider::new(profile, http)),
    }
}

/// 统一的 SSE 读取器：把字节流切成 (event, json) 交给回调。
///
/// `handle` 返回 `Ok(false)` 表示「够了，停止读取」（例如收到了 [DONE]）。
pub(crate) async fn pump_sse(
    resp: reqwest::Response,
    cancel: &Arc<AtomicBool>,
    mut handle: impl FnMut(&str, Value) -> AppResult<bool>,
) -> AppResult<()> {
    use futures_util::StreamExt;

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut current_event = String::new();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // SSE 以行组织；只用 \n 切，\r 单独去掉
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);

            let line = line.trim_end();
            if line.is_empty() {
                // 空行 = 一个事件结束
                current_event.clear();
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                current_event = rest.trim().to_string();
                continue;
            }
            let Some(payload) = line.strip_prefix("data:") else { continue };
            let payload = payload.trim();
            if payload == "[DONE]" {
                return Ok(());
            }
            match serde_json::from_str::<Value>(payload) {
                Ok(v) => {
                    if !handle(&current_event, v)? {
                        return Ok(());
                    }
                }
                Err(e) => {
                    // 半截 JSON / 心跳包都会走到这里，记一笔但不中断整轮对话
                    eprintln!("[provider] 跳过无法解析的 SSE 数据：{e} | {}", truncate(payload, 200));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// 把非 2xx 的响应体整理成一句能看懂的错误。
pub(crate) async fn error_from_response(resp: reqwest::Response) -> AppError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(&body).ok();
    let msg = parsed
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| truncate(&body, 400));
    let hint = match status.as_u16() {
        401 | 403 => "（检查 API Key 是否正确）",
        404 => "（检查 base_url 是否包含了 /v1、模型名是否存在）",
        429 => "（触发限流，稍后再试或换模型）",
        _ => "",
    };
    AppError::Provider(format!("HTTP {status}{hint}：{msg}"))
}

/// 估算「输入 token」，用于界面上的用量显示。
pub use crate::agent::message::estimate_messages_tokens;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_boundary() {
        assert_eq!(truncate("中文很长的一段话", 3), "中文很…");
    }
}
