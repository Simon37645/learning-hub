//! 内置浏览器（查看器）：标签页状态机 + 供 agent 读取的内容快照。
//!
//! 架构要点：**渲染在前端，状态在 Rust**。
//! - 前端负责真正画出来（Markdown 渲染、pdf.js 画布、iframe 网页）
//! - Rust 保存每个标签页的「元数据 + 文本快照」，于是 agent 可以像人一样
//!   「看」当前页面、「翻页」、「跳转」，而前端只是执行者。
//!
//! 前端 → Rust：`viewer_report_snapshot`（文档加载完把文本交上来）、
//! `viewer_report_state`（用户翻页/滚动）。
//! Rust → 前端：`hub://viewer` 事件（打开、关闭、跳转、请求快照）。

pub mod web_extract;

use crate::error::{AppError, AppResult};
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock as AsyncRwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerKind {
    /// Markdown 文档（渲染版）
    Markdown,
    /// PDF（pdf.js 渲染）
    Pdf,
    /// 网页（iframe / 内嵌阅读器）
    Web,
    /// 纯文本 / 代码
    Text,
    /// 图片
    Image,
    /// 空白页（用户点「+」新建标签）
    Blank,
}

impl ViewerKind {
    /// 从路径或 URL 猜类型。
    pub fn detect(source: &str) -> ViewerKind {
        let lower = source.to_ascii_lowercase();
        let ext = lower
            .split(['?', '#'])
            .next()
            .unwrap_or(&lower)
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_string();
        match ext.as_str() {
            "md" | "markdown" | "mdx" => ViewerKind::Markdown,
            "pdf" => ViewerKind::Pdf,
            "html" | "htm" => ViewerKind::Web,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif" => ViewerKind::Image,
            "txt" | "json" | "jsonl" | "csv" | "log" | "yml" | "yaml" | "toml" | "rs" | "ts"
            | "tsx" | "js" | "jsx" | "py" | "c" | "h" | "cpp" | "go" | "java" | "sh" | "tex"
            | "bib" => ViewerKind::Text,
            _ => {
                if lower.starts_with("http://") || lower.starts_with("https://") {
                    ViewerKind::Web
                } else {
                    ViewerKind::Text
                }
            }
        }
    }

    pub fn needs_bytes(self) -> bool {
        matches!(self, ViewerKind::Pdf | ViewerKind::Image)
    }

    pub fn is_textual(self) -> bool {
        matches!(self, ViewerKind::Markdown | ViewerKind::Text)
    }
}

/// PDF 的单页文本。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

/// Rust 侧的完整标签页（含大字段，不直接发给前端）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerTab {
    pub id: String,
    pub kind: ViewerKind,
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    /// 所属主题目录名（本地文件才有）
    #[serde(default)]
    pub topic_slug: Option<String>,
    /// 相对主题目录的路径（本地文件才有）
    #[serde(default)]
    pub path: Option<String>,
    pub page: u32,
    pub total_pages: u32,
    pub scroll: f32,
    pub zoom: f32,
    #[serde(default)]
    pub loading: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// agent 可读的正文快照
    #[serde(default)]
    pub content: String,
    /// PDF 分页文本
    #[serde(default)]
    pub pages: Vec<PageText>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 发给前端的轻量视图。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabView {
    pub id: String,
    pub kind: ViewerKind,
    pub title: String,
    pub url: Option<String>,
    pub topic_slug: Option<String>,
    pub path: Option<String>,
    pub page: u32,
    pub total_pages: u32,
    pub scroll: f32,
    pub zoom: f32,
    pub loading: bool,
    pub error: Option<String>,
    /// 快照字符数（界面上显示「agent 已读到 N 字」）
    pub snapshot_chars: usize,
    pub updated_at: DateTime<Utc>,
}

impl From<&ViewerTab> for TabView {
    fn from(t: &ViewerTab) -> Self {
        Self {
            id: t.id.clone(),
            kind: t.kind,
            title: t.title.clone(),
            url: t.url.clone(),
            topic_slug: t.topic_slug.clone(),
            path: t.path.clone(),
            page: t.page,
            total_pages: t.total_pages,
            scroll: t.scroll,
            zoom: t.zoom,
            loading: t.loading,
            error: t.error.clone(),
            snapshot_chars: t.content.chars().count(),
            updated_at: t.updated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerSnapshot {
    pub tabs: Vec<TabView>,
    pub active_id: Option<String>,
    pub visible: bool,
}

/// 打开一个标签页的请求。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    #[serde(default)]
    pub url: Option<String>,
    /// 相对主题目录的路径
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub topic_slug: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<ViewerKind>,
    /// 打开后跳到第几页
    #[serde(default)]
    pub page: Option<u32>,
    /// 是否新开标签（默认复用同源标签）
    #[serde(default)]
    pub new_tab: bool,
}

struct Inner {
    tabs: Vec<ViewerTab>,
    active: Option<String>,
    visible: bool,
}

/// 查看器服务。可在命令与 agent 工具之间共享。
#[derive(Default)]
pub struct ViewerService {
    inner: AsyncRwLock<Inner>,
    /// 正在等待前端上报快照的请求
    pending: parking_lot::Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self { tabs: Vec::new(), active: None, visible: false }
    }
}

impl ViewerService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> ViewerSnapshot {
        let g = self.inner.read().await;
        ViewerSnapshot {
            tabs: g.tabs.iter().map(TabView::from).collect(),
            active_id: g.active.clone(),
            visible: g.visible,
        }
    }

    pub async fn list(&self) -> Vec<TabView> {
        self.inner.read().await.tabs.iter().map(TabView::from).collect()
    }

    pub async fn find(&self, id: &str) -> Option<ViewerTab> {
        self.inner.read().await.tabs.iter().find(|t| t.id == id).cloned()
    }

    pub async fn active_tab(&self) -> Option<ViewerTab> {
        let g = self.inner.read().await;
        let id = g.active.clone()?;
        g.tabs.iter().find(|t| t.id == id).cloned()
    }

    /// 解析标签页指向的本地文件绝对路径。
    pub async fn file_of(&self, id: &str, resolve_root: impl Fn(&str) -> AppResult<std::path::PathBuf>) -> AppResult<std::path::PathBuf> {
        let tab = self.find(id).await.ok_or_else(|| AppError::NotFound(format!("标签页不存在：{id}")))?;
        let slug = tab
            .topic_slug
            .ok_or_else(|| AppError::invalid("这个标签页不是本地文件"))?;
        let rel = tab.path.ok_or_else(|| AppError::invalid("标签页缺少文件路径"))?;
        let root = resolve_root(&slug)?;
        crate::paths::resolve_in_root(&root, &rel)
    }

    /// 打开（或复用）标签页。
    pub async fn open(&self, req: OpenRequest) -> AppResult<ViewerTab> {
        if req.url.is_none() && req.path.is_none() {
            return Err(AppError::invalid("需要 url 或 path"));
        }
        let kind = req.kind.unwrap_or_else(|| {
            ViewerKind::detect(req.url.as_deref().or(req.path.as_deref()).unwrap_or(""))
        });

        let mut g = self.inner.write().await;

        // 同源复用：同一个文件/网址不重复开标签
        if !req.new_tab {
            let existing = g.tabs.iter().position(|t| {
                (req.url.is_some() && t.url == req.url)
                    || (req.path.is_some() && t.path == req.path && t.topic_slug == req.topic_slug)
            });
            if let Some(idx) = existing {
                let id = g.tabs[idx].id.clone();
                if let Some(page) = req.page {
                    g.tabs[idx].page = page;
                }
                g.tabs[idx].updated_at = Utc::now();
                g.active = Some(id);
                g.visible = true;
                return Ok(g.tabs[idx].clone());
            }
        }

        let title = req
            .title
            .clone()
            .unwrap_or_else(|| default_title(&req, kind));
        let now = Utc::now();
        let tab = ViewerTab {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            title,
            url: req.url,
            topic_slug: req.topic_slug,
            path: req.path,
            page: req.page.unwrap_or(1),
            total_pages: 0,
            scroll: 0.0,
            zoom: 1.0,
            loading: true,
            error: None,
            content: String::new(),
            pages: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        g.tabs.push(tab.clone());
        g.active = Some(tab.id.clone());
        g.visible = true;
        Ok(tab)
    }

    pub async fn activate(&self, id: &str) -> AppResult<ViewerTab> {
        let mut g = self.inner.write().await;
        if !g.tabs.iter().any(|t| t.id == id) {
            return Err(AppError::NotFound(format!("标签页不存在：{id}")));
        }
        g.active = Some(id.to_string());
        g.visible = true;
        Ok(g.tabs.iter().find(|t| t.id == id).unwrap().clone())
    }

    pub async fn close(&self, id: &str) -> AppResult<()> {
        let mut g = self.inner.write().await;
        g.tabs.retain(|t| t.id != id);
        if g.active.as_deref() == Some(id) {
            g.active = g.tabs.last().map(|t| t.id.clone());
        }
        if g.tabs.is_empty() {
            g.visible = false;
        }
        Ok(())
    }

    pub async fn set_visible(&self, visible: bool) {
        self.inner.write().await.visible = visible;
    }

    pub async fn report_state(&self, id: &str, page: Option<u32>, scroll: Option<f32>, total_pages: Option<u32>) {
        let mut g = self.inner.write().await;
        if let Some(t) = g.tabs.iter_mut().find(|t| t.id == id) {
            if let Some(p) = page {
                t.page = p.max(1);
            }
            if let Some(s) = scroll {
                t.scroll = s.clamp(0.0, 1.0);
            }
            if let Some(tp) = total_pages {
                t.total_pages = tp;
            }
            t.loading = false;
            t.updated_at = Utc::now();
        }
    }

    /// 前端把文档文本交上来（Markdown/网页正文/PDF 分页文本）。
    pub async fn report_snapshot(
        &self,
        id: &str,
        content: Option<String>,
        pages: Option<Vec<PageText>>,
        total_pages: Option<u32>,
        error: Option<String>,
    ) {
        let has_pages = pages.is_some();
        {
            let mut g = self.inner.write().await;
            if let Some(t) = g.tabs.iter_mut().find(|t| t.id == id) {
                if let Some(c) = content {
                    t.content = c;
                }
                if let Some(p) = pages {
                    t.total_pages = total_pages.unwrap_or(p.len() as u32);
                    t.pages = p;
                    // 全文快照 = 各页拼接，供关键词检索用
                    t.content = t
                        .pages
                        .iter()
                        .map(|p| format!("【第 {} 页】\n{}", p.page, p.text))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                }
                if !has_pages {
                    if let Some(tp) = total_pages {
                        t.total_pages = tp;
                    }
                }
                t.error = error;
                t.loading = false;
                t.updated_at = Utc::now();
            }
        }
        // 唤醒等待中的 agent
        if let Some(tx) = self.pending.lock().remove(id) {
            let _ = tx.send(());
        }
    }

    pub async fn set_loading(&self, id: &str, loading: bool, error: Option<String>) {
        let mut g = self.inner.write().await;
        if let Some(t) = g.tabs.iter_mut().find(|t| t.id == id) {
            t.loading = loading;
            t.error = error;
            t.updated_at = Utc::now();
        }
    }

    /// 在前端注册一个「我要拿快照」的回调位，返回接收端。
    pub fn register_pending(&self, id: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.to_string(), tx);
        rx
    }

    pub fn cancel_pending(&self, id: &str) {
        self.pending.lock().remove(id);
    }

    /// 在快照里做关键词检索，返回命中位置与上下文。
    pub async fn search(&self, id: &str, query: &str, limit: usize) -> AppResult<Vec<SearchHit>> {
        let tab = self
            .find(id)
            .await
            .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{id}")))?;
        Ok(search_in_text(&tab.content, query, limit))
    }

    /// 取一段文本快照（PDF 可按页裁剪）。
    pub async fn read(
        &self,
        id: &str,
        page_from: Option<u32>,
        page_to: Option<u32>,
        max_chars: usize,
    ) -> AppResult<(ViewerTab, String)> {
        let tab = self
            .find(id)
            .await
            .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{id}")))?;
        let text = if !tab.pages.is_empty() {
            let from = page_from.unwrap_or(1).max(1);
            let to = page_to.unwrap_or(tab.total_pages.max(from)).max(from);
            tab.pages
                .iter()
                .filter(|p| p.page >= from && p.page <= to)
                .map(|p| format!("【第 {} 页】\n{}", p.page, p.text))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            tab.content.clone()
        };
        let text = truncate_chars(&text, max_chars);
        Ok((tab, text))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: u32,
    pub context: String,
    /// 命中在全文里的字符序号，供前端高亮定位
    pub index: usize,
}

/// 跨页/跨段检索。返回带上下文的命中列表。
pub fn search_in_text(content: &str, query: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let q = query.trim();
    if q.is_empty() {
        return hits;
    }
    let lower = content.to_lowercase();
    let ql = q.to_lowercase();
    let mut from = 0usize;
    // 记录每个「【第 N 页】」标记的位置，用来判断命中落在哪一页
    let page_marks: Vec<(usize, u32)> = {
        let mut v = Vec::new();
        let mut idx = 0;
        while let Some(p) = content[idx..].find("【第 ") {
            let abs = idx + p;
            let rest = &content[abs + 6..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>() {
                v.push((abs, n));
            }
            idx = abs + 6;
            if idx >= content.len() {
                break;
            }
        }
        v
    };

    while let Some(pos) = lower[from..].find(&ql) {
        let abs = from + pos;
        let mut start = abs.saturating_sub(80);
        while start > 0 && !content.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (abs + q.len() + 120).min(content.len());
        while end < content.len() && !content.is_char_boundary(end) {
            end += 1;
        }
        let page = page_marks
            .iter()
            .rev()
            .find(|(p, _)| *p <= abs)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        hits.push(SearchHit {
            page,
            context: content[start..end].replace('\n', " ").trim().to_string(),
            index: abs,
        });
        from = abs + q.len().max(1);
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n\n…（内容过长已截断，可用分页或检索定位）");
    out
}

fn default_title(req: &OpenRequest, kind: ViewerKind) -> String {
    if let Some(u) = &req.url {
        if let Ok(parsed) = url::Url::parse(u) {
            let host = parsed.host_str().unwrap_or("网页");
            let path = parsed.path().trim_end_matches('/');
            let last = path.rsplit('/').next().filter(|s| !s.is_empty());
            return match last {
                Some(l) => format!("{host}/{l}"),
                None => host.to_string(),
            };
        }
        return u.clone();
    }
    if let Some(p) = &req.path {
        let name = Path::new(p).file_name().map(|s| s.to_string_lossy().to_string());
        return name.unwrap_or_else(|| format!("{:?}", kind));
    }
    "新标签页".to_string()
}

/// 供工具层构造「文件 URL 描述」用。
pub fn file_source_label(topic_slug: Option<&str>, rel: &str) -> String {
    match topic_slug {
        Some(s) => format!("{s}/{rel}"),
        None => rel.to_string(),
    }
}

/// 内部状态只用于调试输出。
impl ViewerService {
    pub async fn debug_line(&self) -> String {
        let g = self.inner.read().await;
        format!("tabs={} active={:?} visible={}", g.tabs.len(), g.active, g.visible)
    }
}

pub type SharedViewer = Arc<ViewerService>;

/// 便于命令层记录「最近一次操作」。
pub fn now_stamp() -> DateTime<Utc> {
    store::now()
}
