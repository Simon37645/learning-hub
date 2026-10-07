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
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse { .. })
    }
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
        Self::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
            }],
        )
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
                    ContentBlock::ToolUse { name, input, .. } => {
                        estimate_tokens(name) + estimate_tokens(&input.to_string())
                    }
                    ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
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
}
