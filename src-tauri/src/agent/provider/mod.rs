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
    /// 思考强度设置（由模型档案决定）
    pub reasoning: crate::config::ReasoningConfig,
    /// 用户点「停止」时置位，provider 在流循环里检查
    pub cancel: Arc<AtomicBool>,
    /// 图片附件的落盘位置：消息里的 `image` 块只存路径，字节在这里读。
    pub attachments: crate::agent::attachment::Attachments,
    /// 当前档案的模型认不认图片。不认就把图片降级成一行文字——
    /// 让请求失败是最坏的处理方式（用户只会看到一句看不懂的 400）。
    pub supports_vision: bool,
}

/// 服务商报回来的**真实**用量（不是本地估算）。
///
/// 各家的字段口径不一样，这里统一成：
/// - `input_tokens` = 这次请求的**输入总量**（命中缓存的那部分也算在内）
/// - `cached_tokens` = 其中由缓存提供的部分（OpenAI 的 `prompt_tokens_details.cached_tokens`、
///   DeepSeek 的 `prompt_cache_hit_tokens`、Anthropic 的 `cache_read_input_tokens`）
/// - `cache_write_tokens` = 这次写进缓存的部分（Anthropic 的 `cache_creation_input_tokens`）
///
/// `cached_tokens / input_tokens` 就是界面上那个「缓存命中率」。
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderUsage {
    pub input_tokens: u32,
    pub cached_tokens: u32,
    pub cache_write_tokens: u32,
    pub output_tokens: u32,
}

impl ProviderUsage {
    /// 合并同一轮里的多次上报（Anthropic 把输入放在 message_start、输出放在 message_delta；
    /// 这些字段都是累计值，所以取较大者）。
    pub fn merge(&mut self, other: ProviderUsage) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.cached_tokens = self.cached_tokens.max(other.cached_tokens);
        self.cache_write_tokens = self.cache_write_tokens.max(other.cache_write_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

/// 流被网关「规范地」掐断（没等到 [DONE] / finish_reason / message_stop 就耗尽）
/// 时的错误文案。`is_transient` 靠这句话识别它为值得自动重连的错误。
pub(crate) const STREAM_CUT: &str = "模型流被提前截断";

/// 服务商返回的流式增量（尚未累积成完整消息）。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    /// 思考增量，`index` 是内容块序号：Anthropic 开交错思考时会来**多个**
    /// 思考块，各块有各块的签名，必须分开攒（OpenAI 恒为 0）。
    Thinking { index: usize, text: String },
    /// 思考块的签名（Anthropic 的 signature_delta）。回传思考块时必须原样带上，
    /// 缺了服务商会 400。
    ThinkingSignature { index: usize, sig: String },
    /// Anthropic 的 redacted_thinking 块：服务端把这段思考隐去了，没有 delta，
    /// 加密的 data 在 content_block_start 里一次给全。必须原样落库并回传
    /// （见 `ContentBlock::RedactedThinking`），否则思考模式下这条对话会被 400 卡死。
    RedactedThinking { index: usize, data: String },
    /// 工具调用的增量片段，按 `index` 归并
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        args: String,
    },
    /// 结束原因：stop / tool_calls / length …
    Finish(String),
    /// 真实用量。可能来一次（OpenAI 兼容端点通常在最后一块，且那一块 `choices` 是空的），
    /// 也可能来两次（Anthropic：输入在 message_start、输出在 message_delta）。
    Usage(ProviderUsage),
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
/// 返回值表示流是不是**明确地**读完了：true = 收到了完成哨兵（handle 返回
/// false），false = 字节流自然耗尽（可能只是被网关规范地掐断了，模型没说完）。
pub(crate) async fn pump_sse(
    resp: reqwest::Response,
    cancel: &Arc<AtomicBool>,
    mut handle: impl FnMut(&str, Value) -> AppResult<bool>,
) -> AppResult<bool> {
    use futures_util::StreamExt;

    let mut stream = resp.bytes_stream();
    // 必须在**字节层**缓冲：SSE 以行组织，行边界 \n 是单个 ASCII 字节，绝不可
    // 能落进 UTF-8 多字节序列内部，所以「先按字节切行、整行再转字符串」永远安全。
    // 反过来先 `to_string_lossy` 再切行，会把正好跨网络分块的中文拆成 U+FFFD
    // 且不可恢复——长参数的工具调用 JSON 就此报废，整行被当成解析失败丢掉。
    let mut buf: Vec<u8> = Vec::new();
    let mut current_event = String::new();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        buf.extend_from_slice(&chunk?);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let mut line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            line_bytes.pop(); // 换行本身
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop(); // CRLF
            }
            let line = String::from_utf8_lossy(&line_bytes);
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
                return Ok(true);
            }
            match serde_json::from_str::<Value>(payload) {
                Ok(v) => {
                    if !handle(&current_event, v)? {
                        return Ok(true);
                    }
                }
                Err(e) => {
                    // 半截 JSON / 心跳包都会走到这里，记一笔但不中断整轮对话
                    eprintln!("[provider] 跳过无法解析的 SSE 数据：{e} | {}", truncate(payload, 200));
                }
            }
        }
    }
    Ok(false)
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

// ============================================================ 图片

/// 一张图片的结果：要么能发，要么**降级成一行文字**。
///
/// 为什么降级而不是报错：下面三种情况都很常见，而且都不该让整轮对话失败——
/// 用户会看到一句看不懂的 HTTP 400，却不知道是自己关着图片输入还是图被删了。
/// - 文件被删了（对话还能继续，顺便告诉他图没了）
/// - 模型档案没开图片输入（那是配置问题）
/// - 文件超过上限
pub(crate) enum ResolvedImage {
    Ready(crate::agent::attachment::LoadedImage),
    Placeholder(String),
}

pub(crate) fn resolve_image(
    attachments: &crate::agent::attachment::Attachments,
    supports_vision: bool,
    img: crate::agent::message::ImageRef<'_>,
) -> ResolvedImage {
    let label = if img.name.trim().is_empty() { "图片" } else { img.name.trim() };
    if !supports_vision {
        return ResolvedImage::Placeholder(format!(
            "［图片：{label}］（当前模型档案未开启图片输入，这张图没有发出去——\
             可以在「设置 → 模型档案」里打开「支持图片输入」）"
        ));
    }
    if !attachments.is_enabled() {
        return ResolvedImage::Placeholder(format!("［图片：{label}］（没有打开工作区，读不到这张图）"));
    }
    match attachments.load(img.path) {
        Ok(data) => ResolvedImage::Ready(data),
        Err(e) => {
            eprintln!("[provider] 图片读取失败，已降级成文字：{e}");
            ResolvedImage::Placeholder(format!("［图片：{label}］（{e}）"))
        }
    }
}

/// OpenAI 兼容端点的 `image_url.url`：内联 data URL（本地文件没有公网地址）。
pub(crate) fn to_data_url(img: &crate::agent::attachment::LoadedImage) -> String {
    format!(
        "data:{};base64,{}",
        img.media_type,
        crate::agent::attachment::b64_encode(&img.data)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_boundary() {
        assert_eq!(truncate("中文很长的一段话", 3), "中文很…");
    }

    /// 图片读不到 / 模型没开图片输入 → 都要降级成文字，绝不让整轮请求失败。
    #[test]
    fn image_failures_degrade_to_text() {
        use crate::agent::message::ContentBlock;
        let block = ContentBlock::Image {
            path: ".hub/attachments/missing.png".into(),
            media_type: "image/png".into(),
            name: "截图.png".into(),
            bytes: 10,
            width: Some(100),
            height: Some(100),
        };
        let img = block.as_image().unwrap();

        // 档案标了「不支持图片」：直接降级，连盘都不读
        let off = resolve_image(&crate::agent::attachment::Attachments::disabled(), false, img);
        match off {
            ResolvedImage::Placeholder(t) => {
                assert!(t.contains("截图.png"));
                assert!(t.contains("未开启图片输入"));
            }
            _ => panic!("不支持图片时必须降级"),
        }

        // 开着图片输入但文件没了：也要降级
        let tmp = std::env::temp_dir().join(format!("lh-prov-{}", uuid::Uuid::new_v4()));
        let att = crate::agent::attachment::Attachments::new(&tmp);
        match resolve_image(&att, true, img) {
            ResolvedImage::Placeholder(t) => assert!(t.contains("截图.png")),
            _ => panic!("文件不存在时必须降级"),
        }
    }

    /// 正常路径：读盘 → data URL 的形状要能直接被服务商认出来。
    #[test]
    fn image_becomes_inline_data_url() {
        let tmp = std::env::temp_dir().join(format!("lh-prov-{}", uuid::Uuid::new_v4()));
        let att = crate::agent::attachment::Attachments::new(&tmp);
        let png: Vec<u8> = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(b"data");
            v
        };
        let stored = att.save("a.png", &png).unwrap();
        let block = crate::agent::message::ContentBlock::Image {
            path: stored.rel.clone(),
            media_type: stored.media_type.clone(),
            name: "a.png".into(),
            bytes: png.len() as u64,
            width: Some(2),
            height: Some(2),
        };
        match resolve_image(&att, true, block.as_image().unwrap()) {
            ResolvedImage::Ready(loaded) => {
                assert_eq!(loaded.data, png);
                let url = to_data_url(&loaded);
                assert!(url.starts_with("data:image/png;base64,"), "实际：{url}");
            }
            ResolvedImage::Placeholder(t) => panic!("应当能读到：{t}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
