//! 知识库：把主题里的讲义、资料、笔记变成「可以检索、可以引用」的东西。
//!
//! 为什么需要它：用户的讲义（PDF/PPT/Markdown）才是老师划的重点，
//! agent 如果只读到自己生成的笔记，就会讲偏。这里把主题内的文件统一抽成文本块，
//! 带来源标注（文件名 + PDF 页码），让 agent 能引用到「第 12 页第三段」这一级。
//!
//! 索引缓存在 `<主题>/.hub/kb.json`，按文件的大小 + 修改时间判断是否需要重建，
//! 所以刷新很快，只有真的改了文件才会重新抽文本。

use crate::error::AppResult;
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 单个文件抽取后保留的最大字符数（PPT 转出来的 PDF 可能非常长）
const MAX_CHARS_PER_FILE: usize = 400_000;
/// 一个文本块的字符数
const CHUNK_CHARS: usize = 900;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbChunk {
    /// 主题内相对路径
    pub path: String,
    /// PDF 的页码（其它文件为 0）
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbFile {
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    /// 这份资料里出现的小标题（Markdown 取标题行，PDF 取短行）——提示词里给模型看的摘要
    #[serde(default)]
    pub headings: Vec<String>,
    pub chars: usize,
    /// 抽取失败时记下原因，界面上能提示用户「这份是扫描件」
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KbIndex {
    pub built_at: Option<DateTime<Utc>>,
    pub files: Vec<KbFile>,
    pub chunks: Vec<KbChunk>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbHit {
    pub path: String,
    pub page: u32,
    pub context: String,
    /// 引用用的短标签，例如 `ch1.pdf 第 12 页`
    pub label: String,
}

impl KbIndex {
    pub fn total_chars(&self) -> usize {
        self.files.iter().map(|f| f.chars).sum()
    }
}

fn index_path(topic_dir: &Path) -> PathBuf {
    topic_dir.join(crate::domain::topic::DIR_INTERNAL).join("kb.json")
}

pub fn load_index(topic_dir: &Path) -> KbIndex {
    store::read_json_opt::<KbIndex>(&index_path(topic_dir))
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn is_indexable(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "md" | "markdown" | "txt" | "pdf" | "csv" | "tsv" | "json" | "html" | "htm" | "tex" | "rst"
            | "org" | "srt" | "log"
    )
}

/// 把长文本切成块。中文按字符切，尽量在段落/句号处断开。
fn chunk_text(path: &str, page: u32, text: &str, out: &mut Vec<KbChunk>) -> usize {
    let total = text.chars().count();
    let mut start = 0usize;
    let chars: Vec<char> = text.chars().collect();
    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        // 往回找一个自然的断点
        let mut cut = end;
        if end < chars.len() {
            for back in 0..160.min(end - start) {
                let idx = end - back;
                if matches!(chars[idx - 1], '\n' | '。' | '！' | '？' | '.' | '；' | ';') {
                    cut = idx;
                    break;
                }
            }
        }
        let piece: String = chars[start..cut].iter().collect();
        let trimmed = piece.trim();
        if trimmed.chars().count() > 20 {
            out.push(KbChunk {
                path: path.to_string(),
                page,
                text: trimmed.to_string(),
            });
        }
        start = cut.max(start + 1);
    }
    total
}

/// 从 Markdown / 文本里挑「像小标题」的行，作为提示词里的摘要。
fn pick_headings(text: &str, md: bool) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let candidate = if md {
            t.starts_with('#')
        } else {
            // PDF 抽出来的标题通常很短、没有句号结尾
            t.chars().count() <= 40 && !t.ends_with('。') && !t.ends_with('.') && !t.ends_with('，')
        };
        if candidate && t.chars().count() <= 60 {
            let cleaned = t.trim_start_matches('#').trim().to_string();
            if cleaned.chars().count() >= 2 && !out.contains(&cleaned) {
                out.push(cleaned);
            }
        }
        if out.len() >= 24 {
            break;
        }
    }
    out
}

/// 抽取单个文件的文本。PDF 按页返回，其它按整篇一页（page=0）。
async fn extract(path: &Path) -> AppResult<Vec<(u32, String)>> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if ext == "pdf" {
        let p = path.to_path_buf();
        let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text(&p))
            .await
            .map_err(|e| crate::error::AppError::other(format!("PDF 解析失败：{e}")))?
            .map_err(|e| crate::error::AppError::other(format!("PDF 文本提取失败：{e}")))?;
        // pdf-extract 不保留分页，这里按换页符（\u{c}）粗分；没有就用整篇
        if text.contains('\u{c}') {
            return Ok(text
                .split('\u{c}')
                .enumerate()
                .map(|(i, t)| (i as u32 + 1, t.to_string()))
                .collect());
        }
        return Ok(vec![(0, text)]);
    }

    let text = store::read_text_capped(path, MAX_CHARS_PER_FILE as u64)?;
    Ok(vec![(0, text)])
}

/// 构建（或增量刷新）主题的知识库索引。
pub async fn build(topic_dir: &Path, force: bool) -> AppResult<KbIndex> {
    let mut index = if force { KbIndex::default() } else { load_index(topic_dir) };

    // 收集要索引的文件：materials/（讲义资料）、kb/（用户自己丢进来的）、notes/（笔记）
    let mut targets: Vec<PathBuf> = Vec::new();
    for sub in [
        crate::domain::topic::DIR_MATERIALS,
        crate::domain::topic::DIR_KB,
        crate::domain::topic::DIR_NOTES,
    ] {
        for f in store::walk_files(&topic_dir.join(sub), 5) {
            if is_indexable(&f) {
                targets.push(f);
            }
        }
    }
    // 主题根目录下的 README 也算
    let readme = topic_dir.join("README.md");
    if readme.is_file() {
        targets.push(readme);
    }

    let mut files: Vec<KbFile> = Vec::new();
    let mut chunks: Vec<KbChunk> = Vec::new();

    for path in targets {
        let rel = crate::paths::rel_in_root(topic_dir, &path);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        let modified = store::modified_at(&path);

        // 命中缓存就复用，避免每次都抽 PDF
        if !force {
            if let Some(cached) = index.files.iter().find(|f| f.path == rel && f.size == size && f.modified == modified) {
                files.push(cached.clone());
                chunks.extend(index.chunks.iter().filter(|c| c.path == rel).cloned());
                continue;
            }
        }

        let kind = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        match extract(&path).await {
            Ok(pages) => {
                let mut chars = 0usize;
                let mut headings: Vec<String> = Vec::new();
                for (page, text) in &pages {
                    chars += chunk_text(&rel, *page, text, &mut chunks);
                    if headings.len() < 12 {
                        let picked = pick_headings(text, kind == "md" || kind == "markdown");
                        for h in picked {
                            if !headings.contains(&h) && headings.len() < 24 {
                                headings.push(h);
                            }
                        }
                    }
                }
                files.push(KbFile {
                    path: rel,
                    kind,
                    size,
                    modified,
                    headings,
                    chars,
                    error: None,
                });
            }
            Err(e) => {
                files.push(KbFile {
                    path: rel,
                    kind,
                    size,
                    modified,
                    headings: Vec::new(),
                    chars: 0,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    index.files = files;
    index.chunks = chunks;
    index.built_at = Some(Utc::now());

    crate::paths::ensure_dir(&index_path(topic_dir).parent().unwrap().to_path_buf())?;
    store::write_json(&index_path(topic_dir), &index)?;
    Ok(index)
}

/// 在索引里检索。返回带来源标注的片段。
pub fn search(index: &KbIndex, query: &str, limit: usize) -> Vec<KbHit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let terms: Vec<String> = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();

    let mut scored: Vec<(f32, &KbChunk)> = Vec::new();
    for chunk in &index.chunks {
        let lower = chunk.text.to_lowercase();
        let mut score = 0f32;
        for term in &terms {
            let hits = lower.matches(term.as_str()).count();
            if hits > 0 {
                // 完整短语命中权重更高
                score += hits as f32 * (1.0 + term.chars().count() as f32 / 4.0);
            }
        }
        if score > 0.0 {
            scored.push((score, chunk));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .take(limit)
        .map(|(_, c)| {
            let label = if c.page > 0 {
                format!("{} 第 {} 页", c.path, c.page)
            } else {
                c.path.clone()
            };
            // 截取命中附近的一段
            let text = c.text.replace('\n', " ");
            let lower = text.to_lowercase();
            let snippet = match lower.find(&terms[0]) {
                Some(pos) => {
                    let start = pos.saturating_sub(80);
                    let end = (pos + 260).min(text.len());
                    let mut s = start;
                    while s > 0 && !text.is_char_boundary(s) {
                        s -= 1;
                    }
                    let mut e = end;
                    while e < text.len() && !text.is_char_boundary(e) {
                        e += 1;
                    }
                    format!("…{}…", text[s..e].trim())
                }
                None => crate::agent::provider::truncate(&text, 260),
            };
            KbHit {
                path: c.path.clone(),
                page: c.page,
                context: snippet,
                label,
            }
        })
        .collect()
}

/// 生成给系统提示词用的知识库摘要。
pub fn digest(index: &KbIndex, max_files: usize, max_headings: usize) -> String {
    if index.files.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut usable = 0;
    for f in index.files.iter().take(max_files) {
        if f.chars == 0 && f.error.is_some() {
            out.push_str(&format!(
                "- {}（{}，读不出文字：可能是扫描件或图片型 PDF）\n",
                f.path, f.kind
            ));
            continue;
        }
        usable += 1;
        out.push_str(&format!("- {}（{}，约 {} 字）", f.path, f.kind, f.chars));
        if !f.headings.is_empty() {
            let hs: Vec<&str> = f.headings.iter().take(max_headings).map(|s| s.as_str()).collect();
            out.push_str(&format!("：提到 {}", hs.join(" / ")));
        }
        out.push('\n');
    }
    if usable == 0 {
        out.push_str("（索引里没有可读文本，可能需要 OCR 或换一份资料）\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_and_searches() {
        let text = "特征值是矩阵的一个基本概念。\n它描述了变换中方向不变的向量。\n\n第二段讲怎么求特征值：解特征多项式。";
        let mut chunks = Vec::new();
        chunk_text("notes/a.md", 0, text, &mut chunks);
        assert!(!chunks.is_empty());

        let index = KbIndex {
            built_at: None,
            files: vec![KbFile {
                path: "notes/a.md".into(),
                kind: "md".into(),
                size: text.len() as u64,
                modified: Utc::now(),
                headings: vec![],
                chars: text.chars().count(),
                error: None,
            }],
            chunks,
        };
        let hits = search(&index, "特征值", 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].context.contains("特征值"));

        let hits2 = search(&index, "特征多项式", 5);
        assert_eq!(hits2.len(), 1);
        assert!(search(&index, "不存在的词", 5).is_empty());
    }

    #[test]
    fn picks_markdown_headings() {
        let md = "# 标题一\n正文正文正文。\n## 标题二\n更多正文";
        let h = pick_headings(md, true);
        assert_eq!(h, vec!["标题一", "标题二"]);
    }
}
