//! 内置浏览器工具：agent 的「眼睛和手」。
//!
//! agent 可以打开文件/网页、读当前页面、翻页、检索并高亮命中。
//! 内容快照优先由 Rust 侧直接提取（本地文件用文本/PDF 解析，网页用正文提取），
//! 提取不到时再请前端把渲染结果交上来。

use crate::agent::event::ViewerEvent;
use crate::agent::registry::{
    arg_bool, arg_str, arg_str_req, arg_u32, bool_prop, num_prop, object_schema, str_prop, Tool,
    ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::viewer::{OpenRequest, ViewerKind, ViewerTab};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// 等前端上报快照的最长时间。
const SNAPSHOT_WAIT: Duration = Duration::from_secs(8);

/// 把「主题名/materials/xx.pdf」拆成 (所属主题, 主题内路径)。
///
/// 继承来的资料在提示词里就是这种带主题前缀的写法（引用时要照抄），
/// 所以这里替模型兜住：不传 topic 也能按前缀认出来，省得它同时写错两个参数。
fn topic_and_rel(
    ctx: &ToolCtx,
    raw: &str,
    topic_arg: Option<&str>,
) -> AppResult<(crate::domain::topic::Topic, String)> {
    let want = topic_arg.map(str::trim).filter(|s| !s.is_empty());
    if want.is_none() {
        if let Some((head, rest)) = raw.split_once(['/', '\\']) {
            let ws = ctx.core.workspace();
            if ws.topic_dir(head).is_dir() && !rest.trim().is_empty() {
                if let Ok(topic) = ws.resolve(head) {
                    return Ok((topic, rest.to_string()));
                }
            }
        }
    }
    Ok((ctx.topic_or(want)?, raw.to_string()))
}

/// 确保标签页有可读文本，返回是否成功拿到。
async fn ensure_content(ctx: &ToolCtx, tab: &ViewerTab) -> AppResult<ViewerTab> {
    if !tab.content.trim().is_empty() {
        return Ok(tab.clone());
    }

    // 1) 本地文件：Rust 侧直接解析，最快也最可靠
    if let (Some(slug), Some(rel)) = (tab.topic_slug.as_deref(), tab.path.as_deref()) {
        let topic = ctx.core.workspace().resolve(slug)?;
        let path = crate::paths::resolve_in_root(&topic.dir, rel)?;
        if let Ok(text) = super::fs::read_document(&path).await {
            ctx.core
                .viewer
                .report_snapshot(&tab.id, Some(text), None, None, None)
                .await;
            if let Some(t) = ctx.core.viewer.find(&tab.id).await {
                return Ok(t);
            }
        }
    }

    // 2) 网页：抓下来转正文
    if let Some(url) = tab.url.clone() {
        if ctx.core.config_read().agent.allow_web {
            let http = ctx.core.http.clone();
            if let Ok(page) = crate::net::fetch_readable(&http, &url, 200_000).await {
                let body = format!("# {}\n\n来源：{}\n\n{}", page.title, page.final_url, page.markdown);
                ctx.core
                    .viewer
                    .report_snapshot(&tab.id, Some(body), None, None, None)
                    .await;
                if let Some(t) = ctx.core.viewer.find(&tab.id).await {
                    return Ok(t);
                }
            }
        }
    }

    // 3) 请前端把渲染后的文本交上来
    let rx = ctx.core.viewer.register_pending(&tab.id);
    ctx.core.emit_viewer(ViewerEvent::SnapshotRequest { tab_id: tab.id.clone() });
    match tokio::time::timeout(SNAPSHOT_WAIT, rx).await {
        Ok(Ok(())) => {}
        _ => ctx.core.viewer.cancel_pending(&tab.id),
    }
    ctx.core
        .viewer
        .find(&tab.id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("标签页已关闭：{}", tab.id)))
}

/// 解析「用哪个标签」：显式 id > 当前激活的标签。
async fn resolve_tab(ctx: &ToolCtx, tab_id: Option<String>) -> AppResult<ViewerTab> {
    match tab_id {
        Some(id) if !id.trim().is_empty() => ctx
            .core
            .viewer
            .find(id.trim())
            .await
            .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{id}"))),
        _ => ctx.core.viewer.active_tab().await.ok_or_else(|| {
            AppError::invalid("内置浏览器里没有打开的标签页。可以先用 viewer_open 打开一个文件或网页。")
        }),
    }
}

// ---------------------------------------------------------------- viewer_open

pub struct ViewerOpen;

#[async_trait]
impl Tool for ViewerOpen {
    fn name(&self) -> &'static str {
        "viewer_open"
    }

    fn description(&self) -> &'static str {
        "在内置浏览器里打开一个文件或网页，让用户看到你正在讲的东西。\
         本地文件传相对主题目录的 path（PDF、Markdown、图片、代码都行）；网页传 url。\
         讲解 PDF 的某一页时用 page 直接跳过去。\
         要打开**别的主题**（比如父主题里那份整门课的讲义）的文件时，把 path 写成\
         「主题名/materials/xx.pdf」并传 topic，或直接传该主题内相对路径 + topic。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("相对主题目录的文件路径，例如 materials/ch1.pdf"),
                "url": str_prop("网页地址，例如 https://example.com/article"),
                "topic": str_prop("文件所属主题（默认当前主题）；打开父主题的资料时传它"),
                "title": str_prop("标签页标题（可选）"),
                "page": num_prop("打开后跳到第几页（PDF 用）"),
                "new_tab": bool_prop("是否强制新开标签，默认复用同名标签"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        if let Some(u) = arg_str(input, "url") {
            return format!("在内置浏览器打开网页 {u}");
        }
        let path = arg_str(input, "path").unwrap_or_default();
        match arg_u32(input, "page") {
            Some(p) => format!("打开 {path} 第 {p} 页"),
            None => format!("在内置浏览器打开 {path}"),
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let mut path = arg_str(&input, "path");
        let url = arg_str(&input, "url");
        if path.is_none() && url.is_none() {
            return Err(AppError::invalid("需要 path 或 url 之一"));
        }

        // 本地文件先确认存在，避免前端开个空白页
        let topic_slug = if let Some(rel) = path.clone() {
            let (topic, rel) = topic_and_rel(ctx, &rel, arg_str(&input, "topic").as_deref())?;
            // 「…pdf 第三章」这种带章节的引用：去掉尾巴再判断存在性
            let rel = crate::paths::strip_locator_if_missing(&topic.dir, &rel);
            let abs = crate::paths::resolve_in_root(&topic.dir, &rel)?;
            if !abs.exists() {
                return Err(AppError::NotFound(format!("文件不存在：{rel}")));
            }
            path = Some(rel);
            Some(topic.slug())
        } else {
            None
        };

        let req = OpenRequest {
            url,
            path: path.clone(),
            topic_slug,
            title: arg_str(&input, "title"),
            kind: None,
            page: arg_u32(&input, "page"),
            new_tab: arg_bool(&input, "new_tab").unwrap_or(false),
        };
        let tab = ctx.core.viewer.open(req).await?;
        ctx.core.emit_viewer_sync().await;

        let kind_label = match tab.kind {
            ViewerKind::Pdf => "PDF",
            ViewerKind::Markdown => "Markdown",
            ViewerKind::Web => "网页",
            ViewerKind::Image => "图片",
            ViewerKind::Text => "文本",
            ViewerKind::Blank => "空白页",
        };
        let mut out = format!(
            "已在内置浏览器打开（{}）：{}\n标签 id：{}",
            kind_label,
            tab.path.clone().or(tab.url.clone()).unwrap_or_default(),
            tab.id
        );
        if let Some(p) = req_page(&tab) {
            out.push_str(&format!("\n当前页码：{p}"));
        }
        out.push_str("\n需要内容时用 viewer_read 读取，需要定位时用 viewer_goto / viewer_search。");
        Ok(ToolOutput::ok(out))
    }
}

fn req_page(tab: &ViewerTab) -> Option<u32> {
    if tab.kind == ViewerKind::Pdf {
        Some(tab.page)
    } else {
        None
    }
}

// ---------------------------------------------------------------- viewer_list

pub struct ViewerList;

#[async_trait]
impl Tool for ViewerList {
    fn name(&self) -> &'static str {
        "viewer_list"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "列出内置浏览器当前打开的所有标签页（含 id、页码、滚动位置、已读字数）。\
         要读**用户当前没在看**的那个标签（比如他把讲义开在另一个标签上），先调这个拿 id，\
         再用 viewer_read 的 tab_id 去读——不用让用户切过去。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "查看内置浏览器标签".into()
    }

    async fn run(&self, ctx: &ToolCtx, _input: Value) -> AppResult<ToolOutput> {
        let snap = ctx.core.viewer.snapshot().await;
        if snap.tabs.is_empty() {
            return Ok(ToolOutput::ok("内置浏览器当前没有打开任何标签页。"));
        }
        let mut out = format!("共 {} 个标签页：\n\n", snap.tabs.len());
        for t in &snap.tabs {
            let active = if Some(&t.id) == snap.active_id.as_ref() { "▶ " } else { "  " };
            let src = t.path.clone().or(t.url.clone()).unwrap_or_else(|| "（空白）".into());
            out.push_str(&format!(
                "{active}{}｜{}｜{src}\n    id={}｜页码 {}/{}｜滚动 {:.0}%｜已读 {} 字\n",
                t.title,
                kind_label(t.kind),
                t.id,
                t.page,
                if t.total_pages == 0 { 1 } else { t.total_pages },
                t.scroll * 100.0,
                t.snapshot_chars
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

fn kind_label(k: ViewerKind) -> &'static str {
    match k {
        ViewerKind::Pdf => "PDF",
        ViewerKind::Markdown => "MD",
        ViewerKind::Web => "WEB",
        ViewerKind::Image => "IMG",
        ViewerKind::Text => "TXT",
        ViewerKind::Blank => "空",
    }
}

// ---------------------------------------------------------------- viewer_read

pub struct ViewerRead;

#[async_trait]
impl Tool for ViewerRead {
    fn name(&self) -> &'static str {
        "viewer_read"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "读取内置浏览器里某个标签页的文字内容交给模型理解。\
         默认读当前激活的那个；传 tab_id 可以读**任意标签页**——包括用户当前没在看的后台标签，\
         本地文件（PDF、Markdown、txt）与网页都由后端直接提取，不需要用户切过去或重新打开。\
         用 viewer_list 拿各标签的 id。\
         PDF 可以用 page_from / page_to 只读某几页——讲到哪一页就读哪一页。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
                "page_from": num_prop("PDF 起始页（含）"),
                "page_to": num_prop("PDF 结束页（含）"),
                "max_chars": num_prop("最多返回多少字符，默认 30000"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        match (arg_u32(input, "page_from"), arg_u32(input, "page_to")) {
            (Some(a), Some(b)) => format!("读内置浏览器第 {a}-{b} 页"),
            (Some(a), None) => format!("读内置浏览器第 {a} 页起"),
            _ => "读内置浏览器当前页面".into(),
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;
        let tab = ensure_content(ctx, &tab).await?;
        let max = arg_u32(&input, "max_chars").unwrap_or(30_000) as usize;
        let (tab, text) = ctx
            .core
            .viewer
            .read(&tab.id, arg_u32(&input, "page_from"), arg_u32(&input, "page_to"), max)
            .await?;

        if text.trim().is_empty() {
            return Ok(ToolOutput::ok(format!(
                "「{}」暂时读不到文字（可能是扫描版 PDF 或需要登录的网页）。\
                 可以让用户直接看内置浏览器，或改用 web_fetch。",
                tab.title
            )));
        }

        let range = if tab.total_pages > 1 {
            format!("（共 {} 页，当前在第 {} 页）", tab.total_pages, tab.page)
        } else {
            String::new()
        };
        Ok(ToolOutput::ok(format!(
            "来源：{}{}\n\n{text}",
            tab.title, range
        )))
    }
}

// ---------------------------------------------------------------- viewer_goto

pub struct ViewerGoto;

#[async_trait]
impl Tool for ViewerGoto {
    fn name(&self) -> &'static str {
        "viewer_goto"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "让内置浏览器跳转位置：PDF 用 page，长文档用 anchor（标题文字）或 scroll（0~1 的比例）。\
         讲到哪就翻到哪，用户不用自己找。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
                "page": num_prop("PDF 页码"),
                "anchor": str_prop("要滚动到的标题文字（Markdown/网页）"),
                "scroll": { "type": "number", "description": "滚动比例 0~1" },
                "highlight": str_prop("要同时高亮的文字"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        if let Some(p) = arg_u32(input, "page") {
            return format!("翻到第 {p} 页");
        }
        if let Some(a) = arg_str(input, "anchor") {
            return format!("定位到「{a}」");
        }
        if let Some(s) = input.get("scroll").and_then(|v| v.as_f64()) {
            return format!("滚动到 {:.0}%", s * 100.0);
        }
        "刷新浏览器位置".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;
        let page = arg_u32(&input, "page");
        let scroll = input.get("scroll").and_then(|v| v.as_f64()).map(|v| v as f32);
        let anchor = arg_str(&input, "anchor");
        let highlight = arg_str(&input, "highlight");

        if page.is_none() && scroll.is_none() && anchor.is_none() {
            return Err(AppError::invalid("至少要给出 page / anchor / scroll 之一"));
        }
        if let Some(p) = page {
            ctx.core
                .viewer
                .report_state(&tab.id, Some(p), None, None)
                .await;
        }
        if let Some(s) = scroll {
            ctx.core
                .viewer
                .report_state(&tab.id, None, Some(s), None)
                .await;
        }
        ctx.core.emit_viewer(ViewerEvent::Goto {
            tab_id: tab.id.clone(),
            page,
            scroll,
            anchor: anchor.clone(),
            highlight: highlight.clone(),
        });

        Ok(ToolOutput::ok(format!(
            "已让内置浏览器定位：{}{}{}",
            page.map(|p| format!("第 {p} 页")).unwrap_or_default(),
            anchor.map(|a| format!("标题「{a}」")).unwrap_or_default(),
            scroll.map(|s| format!("滚动到 {:.0}%", s * 100.0)).unwrap_or_default(),
        )))
    }
}

// ---------------------------------------------------------------- viewer_search

pub struct ViewerSearch;

#[async_trait]
impl Tool for ViewerSearch {
    fn name(&self) -> &'static str {
        "viewer_search"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "在内置浏览器当前文档里检索关键词，返回命中所在页码与上下文。\
         读长 PDF 或长网页前先用它定位，比通读省时间。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("检索词"),
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
                "limit": num_prop("最多返回多少条，默认 15"),
                "goto_first": bool_prop("是否让浏览器跳到第一处命中，默认 true"),
            }),
            &["query"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("在浏览器文档里检索「{}」", arg_str(input, "query").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let query = arg_str_req(&input, "query")?;
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;
        let tab = ensure_content(ctx, &tab).await?;
        let limit = arg_u32(&input, "limit").unwrap_or(15).min(80) as usize;

        let hits = ctx.core.viewer.search(&tab.id, &query, limit).await?;
        if hits.is_empty() {
            return Ok(ToolOutput::ok(format!("「{}」里没有找到「{query}」。", tab.title)));
        }

        if arg_bool(&input, "goto_first").unwrap_or(true) {
            let first = &hits[0];
            let page = if first.page > 0 { Some(first.page) } else { None };
            ctx.core
                .viewer
                .report_state(&tab.id, page, None, None)
                .await;
            ctx.core.emit_viewer(ViewerEvent::Goto {
                tab_id: tab.id.clone(),
                page,
                scroll: None,
                anchor: None,
                highlight: Some(query.clone()),
            });
        }

        let mut out = format!("在「{}」里找到 {} 处「{query}」：\n\n", tab.title, hits.len());
        for (i, h) in hits.iter().enumerate() {
            let loc = if h.page > 0 { format!("第 {} 页", h.page) } else { format!("第 {} 字", h.index) };
            out.push_str(&format!("{}. {loc}｜{}…\n", i + 1, h.context.chars().take(160).collect::<String>()));
        }
        out.push_str("\n（已让内置浏览器跳到第一处命中）");
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- viewer_activate / close

pub struct ViewerActivate;

#[async_trait]
impl Tool for ViewerActivate {
    fn name(&self) -> &'static str {
        "viewer_activate"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "把某个标签页切到前台（用户当前看到的就是它）。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({ "tab_id": str_prop("标签页 id") }), &["tab_id"])
    }

    fn summarize(&self, input: &Value) -> String {
        format!("切换到标签 {}", arg_str(input, "tab_id").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let id = arg_str_req(&input, "tab_id")?;
        let tab = ctx.core.viewer.activate(&id).await?;
        ctx.core.emit_viewer_sync().await;
        Ok(ToolOutput::ok(format!("已切换到「{}」", tab.title)))
    }
}

pub struct ViewerClose;

#[async_trait]
impl Tool for ViewerClose {
    fn name(&self) -> &'static str {
        "viewer_close"
    }

    // 这些按 tab_id 干活，不依赖主题：工坊里也能读用户正开着的文档
    // （viewer_open 例外——它要靠主题相对路径找到文件，所以只在学习模式出现）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "关闭内置浏览器里的一个标签页。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({ "tab_id": str_prop("标签页 id") }), &["tab_id"])
    }

    fn summarize(&self, input: &Value) -> String {
        format!("关闭标签 {}", arg_str(input, "tab_id").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let id = arg_str_req(&input, "tab_id")?;
        ctx.core.viewer.close(&id).await?;
        ctx.core.emit_viewer_sync().await;
        Ok(ToolOutput::ok("标签页已关闭。"))
    }
}
