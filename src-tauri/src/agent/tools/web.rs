//! 联网工具：抓网页正文 + 联网搜索。
//!
//! 与内置浏览器分工：`viewer_*` 管「用户看到什么」，`web_*` 管「模型读到什么」。

use crate::agent::registry::{
    arg_bool, arg_str, arg_str_req, arg_u32, bool_prop, num_prop, object_schema, str_prop, Tool,
    ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::net;
use crate::viewer::OpenRequest;
use async_trait::async_trait;
use serde_json::{json, Value};

fn ensure_web_allowed(ctx: &ToolCtx) -> AppResult<()> {
    if ctx.core.config_read().agent.allow_web {
        Ok(())
    } else {
        Err(AppError::Denied(
            "联网功能已在设置里关闭。可以在「设置 → Agent」里重新打开。".into(),
        ))
    }
}

pub struct WebFetch;

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "抓取一个网页并提取正文（自动去掉导航、广告、脚本），返回干净的 Markdown 供你阅读引用。\
         想查最新资料、看论文摘要、读文档时用它。open_viewer=true 时同时在内置浏览器打开，让用户一起看。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "url": str_prop("网页地址，可以省略 https://"),
                "max_chars": num_prop("正文最多返回多少字符，默认 30000"),
                "open_viewer": bool_prop("是否同时在内置浏览器打开，默认 false"),
            }),
            &["url"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("抓取网页 {}", arg_str(input, "url").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        ensure_web_allowed(ctx)?;
        let raw = arg_str_req(&input, "url")?;
        let max = arg_u32(&input, "max_chars").unwrap_or(30_000).clamp(1_000, 200_000) as usize;

        let page = net::fetch_readable(&ctx.core.http, &raw, max).await?;

        if arg_bool(&input, "open_viewer").unwrap_or(false) {
            let tab = ctx
                .core
                .viewer
                .open(OpenRequest {
                    url: Some(page.final_url.clone()),
                    path: None,
                    topic_slug: None,
                    title: Some(page.title.clone()),
                    kind: None,
                    page: None,
                    new_tab: false,
                })
                .await?;
            ctx.core
                .viewer
                .report_snapshot(
                    &tab.id,
                    Some(format!("# {}\n\n来源：{}\n\n{}", page.title, page.final_url, page.markdown)),
                    None,
                    None,
                    None,
                )
                .await;
            ctx.core.emit_viewer_sync().await;
        }

        let mut out = format!(
            "标题：{}\n来源：{}\n（{} 字节，正文 {} 字{})\n\n{}",
            page.title,
            page.final_url,
            page.bytes,
            page.markdown.chars().count(),
            if page.truncated { "，已截断" } else { "" },
            page.markdown
        );
        if !page.text.is_empty() && page.text != page.markdown {
            out.push_str("\n\n（如需纯文本版本可再说明）");
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct WebSearch;

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "联网搜索，返回标题/链接/摘要列表。需要权威资料时，先搜再挑一两条用 web_fetch 深读。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("搜索词，写具体一点效果更好"),
                "limit": num_prop("返回条数，默认 8"),
                "engine": { "type": "string", "enum": ["duckduckgo", "bing"], "description": "默认跟随设置" },
                "open_first": bool_prop("是否在内置浏览器打开第一条，默认 false"),
            }),
            &["query"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("联网搜索「{}」", arg_str(input, "query").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        ensure_web_allowed(ctx)?;
        let query = arg_str_req(&input, "query")?;
        let limit = arg_u32(&input, "limit").unwrap_or(8).clamp(1, 30) as usize;
        let engine = arg_str(&input, "engine")
            .unwrap_or_else(|| ctx.core.config_read().viewer.search_engine.clone());

        let hits = net::search(&ctx.core.http, &engine, &query, limit).await?;
        if hits.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "没有搜到「{query}」的结果（引擎 {engine} 可能被限流，换个引擎或稍后再试）。"
            )));
        }

        if arg_bool(&input, "open_first").unwrap_or(false) {
            if let Some(first) = hits.first() {
                let tab = ctx
                    .core
                    .viewer
                    .open(OpenRequest {
                        url: Some(first.url.clone()),
                        path: None,
                        topic_slug: None,
                        title: Some(first.title.clone()),
                        kind: None,
                        page: None,
                        new_tab: false,
                    })
                    .await?;
                let _ = tab;
                ctx.core.emit_viewer_sync().await;
            }
        }

        let mut out = format!("「{query}」的搜索结果（{} 条，引擎 {engine}）：\n\n", hits.len());
        for (i, h) in hits.iter().enumerate() {
            out.push_str(&format!("{}. {}\n   {}\n   {}\n\n", i + 1, h.title, h.url, h.snippet));
        }
        out.push_str("挑其中最相关的用 web_fetch 读正文——不要凭标题和摘要下结论。");
        Ok(ToolOutput::ok(out))
    }
}
