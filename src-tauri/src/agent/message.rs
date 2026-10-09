//! 对话消息模型。
//!
//! 刻意对齐「内容块」结构（Anthropic / OpenAI 现在都收敛到这个形状）：
//! 一条消息 = 若干块，块可以是文本、思考、工具调用、工具结果。
//! 这样转发给不同服务商时只是重排，不需要另建模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// 用户随消息附上的一张图片。
    ///
    /// 只记**工作区相对路径**（`.hub/attachments/<id>.<ext>`），字节在磁盘上——
    /// 见 `agent/attachment.rs` 里「为什么不内联 base64」的说明。
    /// 发请求时才读盘，按各家的形状编码（OpenAI 的 `image_url` / Anthropic 的 `image` 块）。
    Image {
        path: String,
        media_type: String,
        /// 原文件名（只用于显示；用户起的名字可能带斜杠与中文标点，不参与路径）
        #[serde(default)]
        name: String,
        /// 字节数（界面上显示大小）
        #[serde(default)]
        bytes: u64,
        /// 像素尺寸。前端贴图时量得到就带上（估 token 用）；
        /// 量不到也不影响发送——服务商自己会解码。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
    },
    /// 模型的思考过程（DeepSeek-R1 的 reasoning_content / Claude 的 thinking）
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        /// 结果里附带的图片（目前只有「PDF 页截图」会用）。
        /// 默认空：老对话里没有这个字段，读出来就是空的。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ToolImage>,
    },
}

/// 工具结果里附带的图片（`pdf_screenshot` 这类「模型要亲眼看一眼」的东西）。
///
/// 和用户贴图共用一套约定：**字节在磁盘上，消息里只存工作区相对路径**
/// （见 `attachment.rs` 里「为什么不内联 base64」）。
/// 为什么随工具结果落盘：模型读图才知道那一页画的是什么，而下一轮的请求与历史回看
/// 都是从对话记录重建的——不落盘的话它只在当轮「看过一眼」，回看时那张图就没了。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolImage {
    pub path: String,
    pub media_type: String,
    /// 显示用名字，例如「讲义.pdf 第 12 页」
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl ToolImage {
    /// 借用形式的字段，喂给协议编码与图片解析（和 `ContentBlock::Image` 同一形状）。
    pub fn as_image(&self) -> ImageRef<'_> {
        ImageRef {
            path: &self.path,
            media_type: &self.media_type,
            name: &self.name,
            width: self.width,
            height: self.height,
        }
    }
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse { .. })
    }
    /// 图片块 → 借用形式的字段（协议编码与前端渲染都要问这个）。
    pub fn as_image(&self) -> Option<ImageRef<'_>> {
        match self {
            ContentBlock::Image { path, media_type, name, width, height, .. } => Some(ImageRef {
                path,
                media_type,
                name,
                width: *width,
                height: *height,
            }),
            _ => None,
        }
    }
}

/// 图片块里的字段（借用形式，避免每次都要拆 `ContentBlock`）。
#[derive(Debug, Clone, Copy)]
pub struct ImageRef<'a> {
    pub path: &'a str,
    pub media_type: &'a str,
    pub name: &'a str,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMeta {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// 这一轮里模型请求了多少次、用了多少 token。
    /// 服务商报了真实用量就用真实值，没报就退回本地估算（重算时按字符估）。
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub output_tokens: Option<u32>,
    /// 输入里由**缓存**提供的部分（服务商没报就是 None，界面上那一轮就不计入命中率）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// 这一轮写进缓存的部分（Anthropic 才有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u32>,
    /// 这条消息是否被用户中止
    #[serde(default)]
    pub interrupted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: Role,
    pub blocks: Vec<ContentBlock>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub meta: MessageMeta,
}

impl ChatMessage {
    pub fn new(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            blocks,
            created_at: Utc::now(),
            meta: MessageMeta::default(),
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self::new(Role::Assistant, blocks)
    }

    /// 工具结果统一挂在 `tool` 角色上。
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self::tool_result_with(tool_use_id, content, is_error, Vec::new())
    }

    /// 同上，但结果里还附了图片（PDF 页截图）。图片单独走一个字段，
    /// 不混进 `content`：`content` 是给模型读的文字，图片要按协议编码成图片块。
    pub fn tool_result_with(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
        images: Vec<ToolImage>,
    ) -> Self {
        Self::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
                images,
            }],
        )
    }

    /// 这条消息（工具结果）附带的图片。
    pub fn tool_images(&self) -> Vec<&ToolImage> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { images, .. } => Some(images.iter()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 把所有文本块拼起来（忽略工具与思考）。
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn thinking(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }

    /// 这条消息带的所有图片（顺序即块顺序）。
    pub fn images(&self) -> Vec<ImageRef<'_>> {
        self.blocks.iter().filter_map(|b| b.as_image()).collect()
    }

    pub fn has_images(&self) -> bool {
        self.blocks.iter().any(|b| b.as_image().is_some())
    }

    /// 用户消息：若干张图 + 一段文字。
    ///
    /// 图片排在文字**前面**：两家服务商都推荐「图在前、问题在后」，模型对图的指代更稳
    /// （「这张图里的第三行」不会指错）；界面上缩略图也在输入框文字上方。
    pub fn user_with_images(text: &str, images: Vec<ContentBlock>) -> Self {
        let mut blocks = images;
        if !text.trim().is_empty() {
            blocks.push(ContentBlock::text(text));
        }
        Self::new(Role::User, blocks)
    }

    /// 会话标题用的那点文字：没有文本块时（只贴了张图就问）给一句能看懂的占位。
    pub fn display_text(&self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            return text;
        }
        let n = self.images().len();
        if n > 0 {
            return if n == 1 {
                "（图片）".to_string()
            } else {
                format!("（{n} 张图片）")
            };
        }
        String::new()
    }

    pub fn is_empty_assistant(&self) -> bool {
        self.role == Role::Assistant
            && self.text().trim().is_empty()
            && self.blocks.iter().all(|b| !b.is_tool_use())
    }
}

/// 粗略的 token 估算：中文按 1 字 ≈ 1 token，英文按 4 字符 ≈ 1 token。
/// 只用于界面展示用量，不用于计费。
pub fn estimate_tokens(text: &str) -> u32 {
    let mut cjk = 0u32;
    let mut other = 0u32;
    for ch in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) || ('\u{3000}'..='\u{303f}').contains(&ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

pub fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    messages
        .iter()
        .map(|m| {
            m.blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => estimate_tokens(text),
                    ContentBlock::Thinking { text } => estimate_tokens(text),
                    ContentBlock::Image { width, height, .. } => {
                        crate::agent::attachment::estimate_image_tokens(*width, *height)
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        estimate_tokens(name) + estimate_tokens(&input.to_string())
                    }
                    ContentBlock::ToolResult { content, images, .. } => {
                        estimate_tokens(content)
                            + images
                                .iter()
                                .map(|i| crate::agent::attachment::estimate_image_tokens(i.width, i.height))
                                .sum::<u32>()
                    }
                })
                .sum::<u32>()
                + 4
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_json() {
        let m = ChatMessage::new(
            Role::Assistant,
            vec![
                ContentBlock::text("好的"),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "fs_read".into(),
                    input: serde_json::json!({"path": "notes/a.md"}),
                },
            ],
        );
        let s = serde_json::to_string(&m).unwrap();
        let back: ChatMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.text(), "好的");
        assert_eq!(back.tool_uses().len(), 1);
    }

    #[test]
    fn token_estimate_reasonable() {
        assert!(estimate_tokens("你好世界") >= 4);
        assert!(estimate_tokens("hello world") < 10);
    }

    /// 图片块要能原样落盘再读回来（字段名一旦写错，老对话就再也渲染不出图）。
    #[test]
    fn image_block_roundtrip_json() {
        let m = ChatMessage::user_with_images(
            "这张图里第 3 行是什么？",
            vec![ContentBlock::Image {
                path: ".hub/attachments/ab12.png".into(),
                media_type: "image/png".into(),
                name: "截图 2026-10-08.png".into(),
                bytes: 1234,
                width: Some(1200),
                height: Some(800),
            }],
        );
        let s = serde_json::to_string(&m).unwrap();
        // 带 tag 的枚举：变体名是 snake_case，变体内的字段保持原样（snake_case）
        assert!(s.contains("\"type\":\"image\""), "实际：{s}");
        assert!(s.contains("\"media_type\":\"image/png\""), "实际：{s}");
        let back: ChatMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.images().len(), 1);
        assert_eq!(back.images()[0].path, ".hub/attachments/ab12.png");
        assert_eq!(back.images()[0].width, Some(1200));
        // 图在前、文字在后
        assert_eq!(back.blocks[0].as_image().is_some(), true);
        assert_eq!(back.text(), "这张图里第 3 行是什么？");
        // 只带图不带字时，标题要有一句能看懂的占位（否则侧栏那行是空的）
        let bare = ChatMessage::new(Role::User, m.blocks.iter().take(1).cloned().collect());
        assert_eq!(bare.display_text(), "（图片）");
        assert_eq!(m.display_text(), "这张图里第 3 行是什么？");
    }

    /// 老对话没有 width/height（或整个字段都没有）也要能读——JSONL 是长期资产。
    #[test]
    fn image_block_tolerates_missing_optional_fields() {
        let raw = r#"{"id":"m1","role":"user","blocks":[{"type":"image","path":".hub/attachments/a.png","media_type":"image/png"}],"createdAt":"2026-01-01T00:00:00Z","meta":{}}"#;
        let m: ChatMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(m.images().len(), 1);
        assert_eq!(m.images()[0].width, None);
        assert_eq!(m.images()[0].name, "");
    }

    /// 工具结果附带的图片要能落盘再读回来；没有图片的老工具结果不能被新字段读挂。
    #[test]
    fn tool_result_images_roundtrip() {
        let m = ChatMessage::tool_result_with(
            "call_1",
            "第 12 页的图已截给你",
            false,
            vec![ToolImage {
                path: ".hub/attachments/p12.png".into(),
                media_type: "image/png".into(),
                name: "讲义.pdf 第 12 页".into(),
                bytes: 4096,
                width: Some(1568),
                height: Some(2218),
            }],
        );
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"tool_use_id\":\"call_1\""), "实际：{s}");
        assert!(s.contains("\"media_type\":\"image/png\""), "实际：{s}");
        let back: ChatMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tool_images().len(), 1);
        assert_eq!(back.tool_images()[0].width, Some(1568));

        // 没有图片的工具结果不该写出空数组（老对话的形状保持不变）
        let plain = ChatMessage::tool_result("call_2", "读完了", false);
        let ps = serde_json::to_string(&plain).unwrap();
        assert!(!ps.contains("images"), "实际：{ps}");
        let old: ChatMessage = serde_json::from_str(
            r#"{"id":"m2","role":"tool","blocks":[{"type":"tool_result","tool_use_id":"c","content":"x","is_error":false}],"createdAt":"2026-01-01T00:00:00Z","meta":{}}"#,
        )
        .unwrap();
        assert!(old.tool_images().is_empty());
    }
}
