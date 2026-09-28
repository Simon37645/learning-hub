//! 笔记：`notes/` 下的 Markdown 文件。
//!
//! 刻意不引入复杂格式：正文就是 Markdown，元数据可选地写在文件头部的
//! front matter 里（`---` 包起来），缺了就用文件名当标题。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    /// 相对主题目录的路径，例如 `notes/特征值.md`
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub updated_at: DateTime<Utc>,
    pub size: u64,
    /// 正文前若干字，列表里当摘要用
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub content: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// 正文起始字节偏移
    pub body_offset: usize,
}

/// 解析可选的 YAML-ish front matter。只支持 `key: value` 和 `key: a, b`，
/// 够用且不会因为引入 yaml 依赖而让行为变得难以预测。
pub fn parse_front_matter(text: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    let trimmed = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !trimmed.starts_with("---") {
        return fm;
    }
    let rest = &trimmed[3..];
    let Some(end) = rest.find("\n---") else {
        return fm;
    };
    let head = &rest[..end];
    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match k.trim().to_ascii_lowercase().as_str() {
            "title" | "标题" => fm.title = Some(v),
            "tags" | "标签" => {
                fm.tags = v
                    .split([',', '，', ' '])
                    .map(|s| s.trim().trim_start_matches('#').to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            _ => {}
        }
    }
    // +4 = "\n---"
    let after = end + 4;
    let mut offset = 3 + after;
    while trimmed[offset..].starts_with('\n') || trimmed[offset..].starts_with("\r\n") {
        offset += if trimmed[offset..].starts_with("\r\n") { 2 } else { 1 };
    }
    fm.body_offset = offset.min(trimmed.len());
    fm
}

/// 从 Markdown 正文里取标题：front matter > 第一个 H1 > 文件名。
pub fn extract_title(content: &str, fallback: &str) -> String {
    let fm = parse_front_matter(content);
    if let Some(t) = fm.title.filter(|t| !t.is_empty()) {
        return t;
    }
    let body = &content[fm.body_offset.min(content.len())..];
    for line in body.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("# ") {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    fallback.to_string()
}

/// 取一段纯文本摘要（去掉 Markdown 记号）。
pub fn excerpt(content: &str, limit: usize) -> String {
    let fm = parse_front_matter(content);
    let body = &content[fm.body_offset.min(content.len())..];
    let mut out = String::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("```") {
            continue;
        }
        let t = t.trim_start_matches('#').trim();
        let t = t.trim_start_matches(['-', '*', '>', ' ']).trim();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(t);
        if out.chars().count() >= limit {
            break;
        }
    }
    let mut s: String = out.chars().take(limit).collect();
    if out.chars().count() > limit {
        s.push('…');
    }
    s
}

impl NoteSummary {
    pub fn from_file(rel_path: String, content: &str, size: u64, updated_at: DateTime<Utc>) -> Self {
        let file_stem = std::path::Path::new(&rel_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| rel_path.clone());
        let fm = parse_front_matter(content);
        Self {
            path: rel_path,
            title: extract_title(content, &file_stem),
            tags: fm.tags,
            updated_at,
            size,
            excerpt: excerpt(content, 120),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_title() {
        let text = "---\ntitle: 特征值\ntags: 线代, 矩阵\n---\n\n# 忽略这个\n\n正文";
        let fm = parse_front_matter(text);
        assert_eq!(fm.title.as_deref(), Some("特征值"));
        assert_eq!(fm.tags, vec!["线代", "矩阵"]);
        assert_eq!(extract_title(text, "fallback"), "特征值");
        assert!(excerpt(text, 50).contains("正文"));

        assert_eq!(extract_title("# 直接标题\n正文", "fb"), "直接标题");
        assert_eq!(extract_title("没有标题", "fb"), "fb");
    }
}
