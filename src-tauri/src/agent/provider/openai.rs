//! OpenAI 兼容协议（`POST {base}/chat/completions`）。
//!
//! 覆盖绝大多数国产与自建服务：DeepSeek、Moonshot、通义、智谱、硅基流动、
//! Ollama / vLLM / LM Studio 的 OpenAI 兼容端点、以及各种中转网关。

use super::{
    error_from_response, pump_sse, resolve_image, to_data_url, ChatRequest, LlmProvider, ProviderUsage,
    ResolvedImage, StreamEvent,
};
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
        body.insert(
            "messages".into(),
            json!(to_openai_messages(
                &req.system,
                &req.messages,
                &req.attachments,
                req.supports_vision
            )),
        );
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

        // 让官方端点也在流里带上 usage（官方默认不带，界面上的缓存命中率就没数据了）。
        // 但不是所有网关都认这个字段：一旦报「不认识的参数」就退回不带它的请求——用量拿不到可以忍，
        // 对话失败不能。
        let resp = match self.post_with_usage(&body, &req).await {
            Err(e) if e.to_string().contains("stream_options") => {
                eprintln!("[provider] 该端点不认 stream_options，已退回不带用量的请求");
                self.post(&body, &req).await?
            }
            other => other?,
        };

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

            // 用量块要**先于** choices 判断：带 usage 的那一块 choices 通常是空数组。
            if let Some(usage) = v.get("usage").filter(|u| !u.is_null()).and_then(usage_from_openai) {
                let _ = tx.send(StreamEvent::Usage(usage));
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

impl OpenAiProvider {
    /// 发一次请求；非 2xx 会转成可读的错误。
    async fn post(
        &self,
        body: &Map<String, Value>,
        req: &ChatRequest,
    ) -> AppResult<reqwest::Response> {
        let mut rb = self
            .http
            .post(self.profile.chat_url())
            .timeout(req.timeout)
            .json(&Value::Object(body.clone()));
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
        Ok(resp)
    }

    /// 同上，但请求体里多带 `stream_options.include_usage`（官方端点靠它才会在流里报用量）。
    async fn post_with_usage(
        &self,
        body: &Map<String, Value>,
        req: &ChatRequest,
    ) -> AppResult<reqwest::Response> {
        let mut with_usage = body.clone();
        with_usage.insert("stream_options".into(), json!({ "include_usage": true }));
        self.post(&with_usage, req).await
    }
}

/// 从 OpenAI 兼容的 `usage` 对象里取用量。
///
/// 只有能拿到数字才算数：`prompt_tokens` 是**输入总量**（命中缓存的部分也包含在内），
/// 命中量各家写法不同——OpenAI 用 `prompt_tokens_details.cached_tokens`，
/// DeepSeek 用 `prompt_cache_hit_tokens`，个别网关用 `cache_read_input_tokens`。
fn usage_from_openai(u: &Value) -> Option<ProviderUsage> {
    let num = |v: &Value| v.as_u64().map(|n| n as u32);
    let input = num(u.get("prompt_tokens")?)?;
    let output = u
        .get("completion_tokens")
        .and_then(num)
        .unwrap_or_default();
    let cached = u
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(num)
        .or_else(|| u.get("prompt_cache_hit_tokens").and_then(num))
        .or_else(|| u.get("cache_read_input_tokens").and_then(num))
        .unwrap_or_default()
        // 有的网关会把命中量报成总输入的一部分以上，兜一下别让命中率超过 100%
        .min(input);
    let cache_write = u
        .get("cache_creation_input_tokens")
        .and_then(num)
        .unwrap_or_default();
    Some(ProviderUsage {
        input_tokens: input,
        cached_tokens: cached,
        cache_write_tokens: cache_write,
        output_tokens: output,
    })
}

/// 消息 → OpenAI 格式。思考块不回传（DeepSeek 等会因此报错）。
///
/// `attachments` / `supports_vision` 只影响**带图**的用户消息：那种消息的 `content`
/// 要写成 parts 数组（`[{"type":"text"},{"type":"image_url"}]`），
/// 纯文字消息仍然给字符串——数组形式不是所有网关都吃得住，能不用就不用。
pub fn to_openai_messages(
    system: &str,
    messages: &[ChatMessage],
    attachments: &crate::agent::attachment::Attachments,
    supports_vision: bool,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }

    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                let content = if m.has_images() {
                    user_content_parts(m, attachments, supports_vision)
                } else {
                    json!(m.text())
                };
                out.push(json!({ "role": "user", "content": content }));
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

/// 带图用户消息的 `content`：按块顺序排，图在前（模型对图的指代更稳）。
///
/// 读不到图、或档案没开图片输入时，`resolve_image` 会给出一行文字占位——
/// 请求照样发得出去，用户也能从回复/报错里看懂发生了什么。
fn user_content_parts(
    m: &ChatMessage,
    attachments: &crate::agent::attachment::Attachments,
    supports_vision: bool,
) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    for block in &m.blocks {
        match block {
            crate::agent::message::ContentBlock::Text { text } => {
                if !text.is_empty() {
                    parts.push(json!({ "type": "text", "text": text }));
                }
            }
            crate::agent::message::ContentBlock::Image { .. } => {
                let img = block.as_image().expect("分支已经确认是图片块");
                match resolve_image(attachments, supports_vision, img) {
                    ResolvedImage::Ready(loaded) => parts.push(json!({
                        "type": "image_url",
                        "image_url": { "url": to_data_url(&loaded) }
                    })),
                    ResolvedImage::Placeholder(text) => {
                        parts.push(json!({ "type": "text", "text": text }))
                    }
                }
            }
            // 思考块与工具块不出现在用户消息里
            _ => {}
        }
    }
    if parts.is_empty() {
        // 空 content 数组会被部分服务商拒掉，退成空串（它们接受空串）
        return json!("");
    }
    Value::Array(parts)
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
        let out = to_openai_messages(
            "sys",
            &msgs,
            &crate::agent::attachment::Attachments::disabled(),
            false,
        );
        assert_eq!(out.len(), 4);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[2]["tool_calls"][0]["function"]["name"], "fs_read");
        assert!(out[2]["tool_calls"][0]["function"]["arguments"].is_string());
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[3]["tool_call_id"], "call_1");
        // 纯文字消息仍然给字符串 content：数组形式不是所有网关都吃得住
        assert_eq!(out[1]["content"], "读一下笔记");
    }

    /// 带图消息：content 变 parts 数组，图排在文字前面（模型指代更稳）。
    #[test]
    fn encodes_images_as_content_parts() {
        let tmp = std::env::temp_dir().join(format!("lh-oai-{}", uuid::Uuid::new_v4()));
        let att = crate::agent::attachment::Attachments::new(&tmp);
        let png: Vec<u8> = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(b"data");
            v
        };
        let stored = att.save("a.png", &png).unwrap();
        let msg = ChatMessage::user_with_images(
            "这张图里第 3 行是什么？",
            vec![ContentBlock::Image {
                path: stored.rel.clone(),
                media_type: "image/png".into(),
                name: "a.png".into(),
                bytes: png.len() as u64,
                width: Some(800),
                height: Some(600),
            }],
        );

        let out = to_openai_messages("", &[msg], &att, true);
        let content = out[0]["content"].as_array().expect("带图时应当是 parts 数组");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image_url");
        assert_eq!(content[1]["type"], "text");
        assert!(content[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        // 档案没开图片输入：图消失、换成一行文字，请求本身仍然合法
        let off = to_openai_messages(
            "",
            &[ChatMessage::user_with_images(
                "看这张图",
                vec![ContentBlock::Image {
                    path: stored.rel.clone(),
                    media_type: "image/png".into(),
                    name: "a.png".into(),
                    bytes: 4,
                    width: None,
                    height: None,
                }],
            )],
            &att,
            false,
        );
        let content = off[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("未开启图片输入"));
        assert_eq!(content[1]["text"], "看这张图");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn parses_usage_from_openai_and_deepseek_shapes() {
        // OpenAI 官方
        let openai = json!({
            "prompt_tokens": 1000,
            "completion_tokens": 50,
            "prompt_tokens_details": { "cached_tokens": 960 }
        });
        let u = usage_from_openai(&openai).expect("应当解析出用量");
        assert_eq!((u.input_tokens, u.cached_tokens, u.output_tokens), (1000, 960, 50));

        // DeepSeek：命中/未命中分开报，没有 prompt_tokens_details
        let deepseek = json!({
            "prompt_tokens": 2000,
            "completion_tokens": 80,
            "prompt_cache_hit_tokens": 1900,
            "prompt_cache_miss_tokens": 100
        });
        let u = usage_from_openai(&deepseek).expect("应当解析出用量");
        assert_eq!((u.input_tokens, u.cached_tokens, u.output_tokens), (2000, 1900, 80));

        // 没有 usage / 没有数字：不算数（界面就沿用本地估算）
        assert!(usage_from_openai(&json!({})).is_none());
        assert!(usage_from_openai(&json!({"prompt_tokens": null})).is_none());

        // 命中量比总量还大时兜住，别让命中率超过 100%
        let weird = json!({ "prompt_tokens": 100, "prompt_cache_hit_tokens": 5000 });
        assert_eq!(usage_from_openai(&weird).unwrap().cached_tokens, 100);
    }
}
