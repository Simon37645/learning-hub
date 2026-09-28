//! OpenAI 兼容协议（`POST {base}/chat/completions`）。
//!
//! 覆盖绝大多数国产与自建服务：DeepSeek、Moonshot、通义、智谱、硅基流动、
//! Ollama / vLLM / LM Studio 的 OpenAI 兼容端点、以及各种中转网关。

use super::{error_from_response, pump_sse, ChatRequest, LlmProvider, StreamEvent};
use crate::agent::message::{ChatMessage, Role};
use crate::agent::registry::ToolSpec;
use crate::config::{ProviderKind, ProviderProfile};
use crate::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc::UnboundedSender;

pub struct OpenAiProvider {
    profile: ProviderProfile,
    http: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(profile: &ProviderProfile, http: reqwest::Client) -> Self {
        Self { profile: profile.clone(), http }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }

    fn label(&self) -> String {
        format!("{} · {}", self.profile.name, self.profile.model)
    }

    async fn stream(&self, req: ChatRequest, tx: UnboundedSender<StreamEvent>) -> AppResult<()> {
        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        body.insert("messages".into(), json!(to_openai_messages(&req.system, &req.messages)));
        body.insert("stream".into(), json!(true));
        body.insert("temperature".into(), json!(req.temperature));
        body.insert("max_tokens".into(), json!(req.max_tokens));

        // 思考强度：只有用户明确打开、且风格允许时才写进请求体
        apply_reasoning_openai(&mut body, &req.reasoning, self.profile.kind);

        let use_tools = self.profile.supports_tools && !req.tools.is_empty();
        if use_tools {
            body.insert("tools".into(), json!(to_openai_tools(&req.tools)));
            body.insert("tool_choice".into(), json!("auto"));
        }

        let mut rb = self
            .http
            .post(self.profile.chat_url())
            .timeout(req.timeout)
            .json(&Value::Object(body));
        let key = self.profile.api_key.trim();
        if !key.is_empty() {
            rb = rb.bearer_auth(key);
        }
        for (k, v) in &self.profile.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }

        let resp = rb.send().await?;
        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let cancel = req.cancel.clone();
        pump_sse(resp, &cancel, move |_event, v| {
            if let Some(err) = v.get("error") {
                if !err.is_null() {
                    let msg = err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("模型返回了错误");
                    return Err(crate::error::AppError::Provider(msg.to_string()));
                }
            }

            let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
                return Ok(true);
            };

            if let Some(delta) = choice.get("delta") {
                if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() && tx.send(StreamEvent::Text(text.to_string())).is_err() {
                        return Ok(false);
                    }
                }
                // DeepSeek-R1 / 部分推理模型把思考放在 reasoning_content
                let reasoning = delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .and_then(|c| c.as_str());
                if let Some(text) = reasoning {
                    if !text.is_empty() && tx.send(StreamEvent::Thinking(text.to_string())).is_err() {
                        return Ok(false);
                    }
                }
                if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let id = call.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
                        let name = call
                            .pointer("/function/name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string());
                        let args = call
                            .pointer("/function/arguments")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        if tx
                            .send(StreamEvent::ToolCall { index, id, name, args })
                            .is_err()
                        {
                            return Ok(false);
                        }
                    }
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                let _ = tx.send(StreamEvent::Finish(reason.to_string()));
            }
            Ok(true)
        })
        .await
    }
}

/// 消息 → OpenAI 格式。思考块不回传（DeepSeek 等会因此报错）。
pub fn to_openai_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }

    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                out.push(json!({ "role": "user", "content": m.text() }));
            }
            Role::Assistant => {
                let text = m.text();
                let tool_calls: Vec<Value> = m
                    .tool_uses()
                    .iter()
                    .map(|(id, name, input)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                // 参数必须是字符串形式的 JSON
                                "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                            }
                        })
                    })
                    .collect();

                let mut obj = Map::new();
                obj.insert("role".into(), json!("assistant"));
                // 有工具调用时 content 允许为空串（部分服务商不接受 null）
                obj.insert("content".into(), json!(text));
                if !tool_calls.is_empty() {
                    obj.insert("tool_calls".into(), json!(tool_calls));
                }
                out.push(Value::Object(obj));
            }
            Role::Tool => {
                for block in &m.blocks {
                    if let crate::agent::message::ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": content,
                        }));
                    }
                }
            }
        }
    }
    out
}

/// 把思考强度写进 OpenAI 兼容的请求体。
///
/// 这个字段不是所有服务商都认，所以：
/// - `effort = off` 时完全不写（默认，保证最大兼容）
/// - 风格为 QwenThinking 时改发 `enable_thinking`（通义千问的写法）
/// - 风格为 None 时不发
fn apply_reasoning_openai(
    body: &mut Map<String, Value>,
    cfg: &crate::config::ReasoningConfig,
    _kind: crate::config::ProviderKind,
) {
    use crate::config::{ReasoningEffort, ReasoningStyle};
    if !cfg.effort.is_on() || cfg.style == ReasoningStyle::None {
        return;
    }
    match cfg.style {
        ReasoningStyle::QwenThinking => {
            body.insert("enable_thinking".into(), json!(true));
            // 部分网关同时认这个上限字段
            body.insert(
                "thinking_budget".into(),
                json!(cfg.effort.thinking_budget()),
            );
        }
        ReasoningStyle::AnthropicThinking => {
            body.insert(
                "thinking".into(),
                json!({ "type": "enabled", "budget_tokens": cfg.effort.thinking_budget() }),
            );
        }
        // Auto / OpenaiEffort：发标准字段
        _ => {
            if cfg.effort != ReasoningEffort::Off {
                body.insert("reasoning_effort".into(), json!(cfg.effort.openai_value()));
            }
        }
    }
}

fn to_openai_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::ContentBlock;

    #[test]
    fn converts_tool_roundtrip() {
        let msgs = vec![
            ChatMessage::user("读一下笔记"),
            ChatMessage::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "fs_read".into(),
                    input: json!({"path": "notes/a.md"}),
                }],
            ),
            ChatMessage::tool_result("call_1", "内容", false),
        ];
        let out = to_openai_messages("sys", &msgs);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[2]["tool_calls"][0]["function"]["name"], "fs_read");
        assert!(out[2]["tool_calls"][0]["function"]["arguments"].is_string());
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[3]["tool_call_id"], "call_1");
    }
}
