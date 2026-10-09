//! Anthropic 协议（`POST {base}/messages`）。
//!
//! 与 OpenAI 的三个关键差异：
//! 1. system 是顶层参数，不是一条消息
//! 2. 工具调用/结果是内容块（`tool_use` / `tool_result`），不是独立字段
//! 3. user / assistant 必须交替出现，同角色连续消息需要合并

use super::{error_from_response, pump_sse, ChatRequest, LlmProvider, ProviderUsage, StreamEvent};
use crate::agent::message::{ChatMessage, ContentBlock, Role};
use crate::agent::registry::ToolSpec;
use crate::config::{ProviderKind, ProviderProfile};
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc::UnboundedSender;

/// 与 Anthropic API 版本绑定的常量；升级时改这里。
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    profile: ProviderProfile,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(profile: &ProviderProfile, http: reqwest::Client) -> Self {
        Self { profile: profile.clone(), http }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn label(&self) -> String {
        format!("{} · {}", self.profile.name, self.profile.model)
    }

    async fn stream(&self, req: ChatRequest, tx: UnboundedSender<StreamEvent>) -> AppResult<()> {
        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        body.insert("max_tokens".into(), json!(req.max_tokens));
        // Anthropic 开了 extended thinking 就不接受自定义 temperature（必须为默认值 1）
        let thinking = reasoning_for_anthropic(&req.reasoning);
        if thinking.is_none() {
            body.insert("temperature".into(), json!(req.temperature));
        }
        if let Some(budget) = thinking {
            // 官方要求 budget_tokens < max_tokens，这里夹一下避免请求被拒
            let budget = budget.min(req.max_tokens.saturating_sub(1).max(1024));
            body.insert(
                "thinking".into(),
                json!({ "type": "enabled", "budget_tokens": budget }),
            );
            body.insert("max_tokens".into(), json!(req.max_tokens.max(budget + 1)));
        }
        body.insert("stream".into(), json!(true));
        if !req.system.trim().is_empty() {
            body.insert("system".into(), json!(req.system));
        }
        body.insert(
            "messages".into(),
            json!(to_anthropic_messages(
                &req.messages,
                &req.attachments,
                req.supports_vision
            )),
        );
        if !req.tools.is_empty() {
            body.insert("tools".into(), json!(to_anthropic_tools(&req.tools)));
        }

        let mut rb = self
            .http
            .post(self.profile.chat_url())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .timeout(req.timeout)
            .json(&Value::Object(body));
        let key = self.profile.api_key.trim();
        if !key.is_empty() {
            rb = rb.header("x-api-key", key);
        }
        for (k, v) in &self.profile.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }

        let resp = rb.send().await?;
        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let cancel = req.cancel.clone();
        // 记录 content_block_index → 是否工具块，用于把增量归到正确的工具序号上
        let mut block_is_tool: Vec<bool> = Vec::new();
        let mut tool_seq: Vec<usize> = Vec::new();

        pump_sse(resp, &cancel, move |event, v| {
            let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or(event);
            match kind {
                "error" => {
                    let msg = v
                        .pointer("/error/message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Anthropic 返回错误");
                    return Err(AppError::Provider(msg.to_string()));
                }
                "content_block_start" => {
                    let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    if block_is_tool.len() <= index {
                        block_is_tool.resize(index + 1, false);
                        tool_seq.resize(index + 1, usize::MAX);
                    }
                    let block = v.get("content_block");
                    let is_tool = block
                        .and_then(|b| b.get("type"))
                        .and_then(|t| t.as_str())
                        .is_some_and(|t| t == "tool_use" || t == "server_tool_use");
                    block_is_tool[index] = is_tool;
                    if is_tool {
                        let seq = tool_seq.iter().filter(|s| **s != usize::MAX).count();
                        tool_seq[index] = seq;
                        let id = block.and_then(|b| b.get("id")).and_then(|i| i.as_str()).map(String::from);
                        let name = block.and_then(|b| b.get("name")).and_then(|n| n.as_str()).map(String::from);
                        if tx
                            .send(StreamEvent::ToolCall { index: seq, id, name, args: String::new() })
                            .is_err()
                        {
                            return Ok(false);
                        }
                    }
                }
                "content_block_delta" => {
                    let block_index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let delta = v.get("delta");
                    let dtype = delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()).unwrap_or("");
                    match dtype {
                        "text_delta" => {
                            if let Some(text) = delta.and_then(|d| d.get("text")).and_then(|t| t.as_str()) {
                                if !text.is_empty() && tx.send(StreamEvent::Text(text.to_string())).is_err() {
                                    return Ok(false);
                                }
                            }
                        }
                        "thinking_delta" => {
                            if let Some(text) = delta.and_then(|d| d.get("thinking")).and_then(|t| t.as_str()) {
                                if !text.is_empty() && tx.send(StreamEvent::Thinking(text.to_string())).is_err() {
                                    return Ok(false);
                                }
                            }
                        }
                        "input_json_delta" => {
                            let seq = tool_seq.get(block_index).copied().unwrap_or(usize::MAX);
                            if seq != usize::MAX {
                                let partial = delta
                                    .and_then(|d| d.get("partial_json"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("");
                                if !partial.is_empty()
                                    && tx
                                        .send(StreamEvent::ToolCall {
                                            index: seq,
                                            id: None,
                                            name: None,
                                            args: partial.to_string(),
                                        })
                                        .is_err()
                                {
                                    return Ok(false);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // 输入用量（含缓存命中/写入）在 message_start 里
                "message_start" => {
                    if let Some(u) = v.pointer("/message/usage").and_then(usage_from_anthropic) {
                        let _ = tx.send(StreamEvent::Usage(u));
                    }
                }
                "message_delta" => {
                    if let Some(reason) = v.pointer("/delta/stop_reason").and_then(|r| r.as_str()) {
                        let mapped = match reason {
                            "tool_use" => "tool_calls",
                            "end_turn" | "stop_sequence" => "stop",
                            "max_tokens" => "length",
                            other => other,
                        };
                        let _ = tx.send(StreamEvent::Finish(mapped.to_string()));
                    }
                    // 累计输出量在 message_delta 的 usage 里
                    if let Some(u) = v.get("usage").and_then(usage_from_anthropic) {
                        let _ = tx.send(StreamEvent::Usage(u));
                    }
                }
                "message_stop" => return Ok(false),
                _ => {}
            }
            Ok(true)
        })
        .await
    }
}

/// 从 Anthropic 的 `usage` 对象里取用量。
///
/// 口径和 OpenAI 不同：`input_tokens` 只算**没命中缓存**的那部分，
/// 所以这里的输入总量 = input + cache_read + cache_creation，
/// 命中量就是 `cache_read_input_tokens`（写进去的那次算未命中，符合直觉）。
fn usage_from_anthropic(u: &Value) -> Option<ProviderUsage> {
    let num = |k: &str| u.get(k).and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0);
    let fresh = num("input_tokens");
    let read = num("cache_read_input_tokens");
    let write = num("cache_creation_input_tokens");
    let output = num("output_tokens");
    let total_in = fresh + read + write;
    if total_in == 0 && output == 0 {
        return None;
    }
    Some(ProviderUsage {
        input_tokens: total_in,
        cached_tokens: read,
        cache_write_tokens: write,
        output_tokens: output,
    })
}

/// 该不该开 extended thinking，开了的话预算是多少。
///
/// 风格为 QwenThinking / None 时不发（那是别的服务商的写法）。
fn reasoning_for_anthropic(cfg: &crate::config::ReasoningConfig) -> Option<u32> {
    use crate::config::ReasoningStyle;
    if !cfg.effort.is_on() {
        return None;
    }
    match cfg.style {
        ReasoningStyle::QwenThinking | ReasoningStyle::None => None,
        _ => Some(cfg.effort.thinking_budget()),
    }
}

/// 消息 → Anthropic 格式，并合并连续同角色消息。
///
/// `attachments` / `supports_vision` 只影响带图的用户消息：Anthropic 的图是内容块
/// （`{"type":"image","source":{"type":"base64",…}}`），和文字同一个数组里按顺序排。
pub fn to_anthropic_messages(
    messages: &[ChatMessage],
    attachments: &crate::agent::attachment::Attachments,
    supports_vision: bool,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();

    let mut push = |role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                    arr.extend(blocks);
                    return;
                }
            }
        }
        out.push(json!({ "role": role, "content": blocks }));
    };

    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                if m.has_images() {
                    let mut blocks: Vec<Value> = Vec::new();
                    for b in &m.blocks {
                        match b {
                            ContentBlock::Text { text } => {
                                if !text.is_empty() {
                                    blocks.push(json!({ "type": "text", "text": text }));
                                }
                            }
                            ContentBlock::Image { .. } => {
                                let img = b.as_image().expect("分支已确认是图片块");
                                match super::resolve_image(attachments, supports_vision, img) {
                                    super::ResolvedImage::Ready(loaded) => blocks.push(json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": loaded.media_type,
                                            // Anthropic 要裸 base64，不要 data URL 前缀
                                            "data": crate::agent::attachment::b64_encode(&loaded.data),
                                        }
                                    })),
                                    super::ResolvedImage::Placeholder(text) => {
                                        blocks.push(json!({ "type": "text", "text": text }))
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    push("user", blocks);
                } else {
                    let text = m.text();
                    push("user", vec![json!({ "type": "text", "text": text })]);
                }
            }
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                let text = m.text();
                if !text.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for b in &m.blocks {
                    if let ContentBlock::ToolUse { id, name, input } = b {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                }
                push("assistant", blocks);
            }
            Role::Tool => {
                // 工具结果以 user 身份回传
                let blocks: Vec<Value> = m
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, content, is_error, images } => {
                            // 纯文字结果保持字符串形状（最兼容）；带图的才升级成内容块数组
                            // ——Anthropic 允许 tool_result 的 content 里直接放 image 块
                            let body = if images.is_empty() {
                                json!(content)
                            } else {
                                let mut parts = vec![json!({ "type": "text", "text": content })];
                                for img in images {
                                    match super::resolve_image(attachments, supports_vision, img.as_image()) {
                                        super::ResolvedImage::Ready(loaded) => parts.push(json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": loaded.media_type,
                                                "data": crate::agent::attachment::b64_encode(&loaded.data),
                                            }
                                        })),
                                        super::ResolvedImage::Placeholder(text) => {
                                            parts.push(json!({ "type": "text", "text": text }))
                                        }
                                    }
                                }
                                json!(parts)
                            };
                            Some(json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": body,
                                "is_error": is_error,
                            }))
                        }
                        _ => None,
                    })
                    .collect();
                push("user", blocks);
            }
        }
    }
    out
}

fn to_anthropic_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_usage_sums_cache_into_input() {
        // message_start 的形状：input_tokens 只是「没命中」的那部分
        let start = json!({
            "input_tokens": 120,
            "cache_read_input_tokens": 8800,
            "cache_creation_input_tokens": 80,
            "output_tokens": 1
        });
        let u = usage_from_anthropic(&start).expect("应当解析出用量");
        assert_eq!(u.input_tokens, 9000, "总量 = 未命中 + 命中 + 写入");
        assert_eq!(u.cached_tokens, 8800, "命中量只有 cache_read");
        assert_eq!(u.cache_write_tokens, 80);

        // message_delta 的形状：只有累计输出
        let delta = json!({ "output_tokens": 512 });
        let u = usage_from_anthropic(&delta).expect("应当解析出用量");
        assert_eq!(u.output_tokens, 512);
        assert_eq!(u.input_tokens, 0);

        // 全空 → 不算数
        assert!(usage_from_anthropic(&json!({})).is_none());
    }

    #[test]
    fn merges_consecutive_roles() {
        let msgs = vec![
            ChatMessage::user("问题"),
            ChatMessage::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "fs_read".into(),
                    input: json!({"path": "a.md"}),
                }],
            ),
            ChatMessage::tool_result("tu_1", "结果", false),
            ChatMessage::tool_result("tu_2", "结果2", false),
        ];
        let out = to_anthropic_messages(
            &msgs,
            &crate::agent::attachment::Attachments::disabled(),
            false,
        );
        // user / assistant / user（后两条工具结果合并）
        assert_eq!(out.len(), 3);
        assert_eq!(out[1]["content"][0]["type"], "tool_use");
        assert_eq!(out[2]["content"].as_array().unwrap().len(), 2);
        // 纯文字的工具结果保持字符串形状
        assert!(out[2]["content"][0]["content"].is_string());
    }

    /// 工具结果带图（PDF 页截图）：tool_result 的 content 升级成内容块数组，
    /// 图用裸 base64；文字块仍在最前面。
    #[test]
    fn tool_result_images_become_content_blocks() {
        let tmp = std::env::temp_dir().join(format!("lh-ant-shot-{}", uuid::Uuid::new_v4()));
        let att = crate::agent::attachment::Attachments::new(&tmp);
        let png: Vec<u8> = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(b"page-3");
            v
        };
        let stored = att.save("p3.png", &png).unwrap();
        let msgs = vec![
            ChatMessage::user("第三页那个图"),
            ChatMessage::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tu_9".into(),
                    name: "pdf_screenshot".into(),
                    input: json!({"page": 3}),
                }],
            ),
            ChatMessage::tool_result_with(
                "tu_9",
                "第 3 页已截给你",
                false,
                vec![crate::agent::message::ToolImage {
                    path: stored.rel.clone(),
                    media_type: "image/png".into(),
                    name: "讲义.pdf 第 3 页".into(),
                    bytes: png.len() as u64,
                    width: Some(1568),
                    height: Some(2218),
                }],
            ),
        ];
        let out = to_anthropic_messages(&msgs, &att, true);
        let result = &out[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        let blocks = result["content"].as_array().expect("带图的工具结果要用内容块数组");
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert!(!blocks[1]["source"]["data"].as_str().unwrap().starts_with("data:"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 带图消息：图是内容块，source 里是**裸 base64**（不能带 data URL 前缀）。
    #[test]
    fn encodes_images_as_content_blocks() {
        let tmp = std::env::temp_dir().join(format!("lh-ant-{}", uuid::Uuid::new_v4()));
        let att = crate::agent::attachment::Attachments::new(&tmp);
        let png: Vec<u8> = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(b"data");
            v
        };
        let stored = att.save("a.png", &png).unwrap();
        let msg = ChatMessage::user_with_images(
            "看这张图",
            vec![ContentBlock::Image {
                path: stored.rel.clone(),
                media_type: "image/png".into(),
                name: "a.png".into(),
                bytes: png.len() as u64,
                width: Some(100),
                height: Some(100),
            }],
        );
        let out = to_anthropic_messages(&[msg], &att, true);
        let blocks = out[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert!(!blocks[0]["source"]["data"].as_str().unwrap().contains("data:"));
        assert_eq!(blocks[1]["type"], "text");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
