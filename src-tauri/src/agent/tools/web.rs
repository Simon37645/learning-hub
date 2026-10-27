//! 联网工具：DeepSeek Search / Fetch（alpha）。
//!
//! 与内置浏览器分工：`viewer_*` 管「用户看到什么」，`web_*` 管「模型读到什么」。
//! 底层是 DeepSeek 的 Search / Fetch 接口（与 DeepSeek 模型 API 同一把 Key、
//! 同一 Bearer 鉴权，服务端抓取能拿到本机 reqwest 拿不到的反爬页面）。
//! **按成功请求计费**——所以 web_search 默认不要全文（summary 已够模型判断），
//! 读全文是对具体 url 再调一次 web_fetch；HTTP 走 `http_fresh()`（系统代理在
//! 发请求那一刻生效）并带单次超时。
//!
//! 未配 Key / 被开关关掉 / 请求失败：返回中文、可操作的错误（引导用户去
//! 「设置 → 联网搜索」），**绝不让整轮对话失败**——工具错误是 AppError，
//! 会被包成工具结果回到模型手里，由它转告用户。

use crate::agent::registry::{
    arg_bool, arg_str, arg_str_req, arg_u32, bool_prop, num_prop, object_schema, str_prop, Tool,
    ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::viewer::OpenRequest;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

const SEARCH_URL: &str = "https://api.deepseek.com/v1/tools/search";
const FETCH_URL: &str = "https://api.deepseek.com/v1/tools/fetch";
/// DeepSeek 对 query / ori_question 的上限（官方文档：1-128 字符）
const QUERY_MAX_CHARS: usize = 128;
/// 抓取 URL 的上限（官方文档：不超过 8192 字符）
const FETCH_URL_MAX_CHARS: usize = 8192;
/// 单次请求超时：短请求、按次计费，30 秒足够（更长的卡死交给网络层空闲超时）
const DEEPSEEK_TIMEOUT: Duration = Duration::from_secs(30);
/// 搜索结果条数上下限（官方文档：count 1-50，默认 10；我们默认 8 省着用）
const COUNT_MIN: u32 = 1;
const COUNT_MAX: u32 = 50;
/// rerank_score 达到多少算「高质量」结果（官方文档：不低于 1.5）
const GOOD_SCORE: f64 = 1.5;

fn ensure_web_allowed(ctx: &ToolCtx) -> AppResult<()> {
    if ctx.core.config_read().agent.allow_web {
        Ok(())
    } else {
        Err(AppError::Denied(
            "联网功能已在设置里关闭。可以在「设置 → Agent」里重新打开。".into(),
        ))
    }
}

/// 联网工具的门：既有的联网总闸（allow_web）+ DeepSeek 配置检查。
/// 未配 Key / 被关掉都返回**可操作的中文指引**，让模型转告用户去设置页处理，
/// 而不是把整轮对话打断。
fn ensure_deepseek_ready(ctx: &ToolCtx) -> AppResult<()> {
    ensure_web_allowed(ctx)?;
    let ds = ctx.core.config_read().deepseek;
    if !ds.has_key() {
        return Err(AppError::Denied(
            "联网搜索还没配置 DeepSeek API Key（设置 → 联网搜索；与 DeepSeek 模型 API 是同一把 Key）。\
             请引导用户去设置页配置后再试；在那之前不要编造检索结果。"
                .into(),
        ));
    }
    if !ds.enabled {
        return Err(AppError::Denied(
            "联网搜索的总开关目前是关着的（设置 → 联网搜索）。请引导用户打开后再试。".into(),
        ));
    }
    Ok(())
}

/// 清洗搜索词：压掉多余空白、截到官方上限（1-128 字符）。
/// DeepSeek 支持 site: / -site: / 引号短语，原样保留。
pub(crate) fn sanitize_query(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(QUERY_MAX_CHARS).collect()
}

/// 组装搜索请求体。query 必填（清洗后为空报错）、count 夹到 1-50、
/// `need_content` 恒为 false——按次计费，summary（600-800 字）已够模型判断
/// 要不要用 web_fetch 深读。
pub(crate) fn build_search_body(
    query: &str,
    count: u32,
    ori_question: Option<&str>,
) -> AppResult<Value> {
    let q = sanitize_query(query);
    if q.is_empty() {
        return Err(AppError::invalid(
            "搜索词不能为空。先想清楚要查什么（可带 site: / -site: / 引号短语），再调用一次。",
        ));
    }
    let mut body = json!({
        "query": q,
        "count": count.clamp(COUNT_MIN, COUNT_MAX),
        "need_content": false,
    });
    if let Some(oq) = ori_question.map(sanitize_query).filter(|s| !s.is_empty()) {
        body["ori_question"] = Value::String(oq);
    }
    Ok(body)
}

/// 校验并组装抓取请求体：补默认协议、只放行 http/https、拒绝带用户名密码的
/// URL（官方要求）、限长 8192。
pub(crate) fn build_fetch_body(raw: &str) -> AppResult<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::invalid("网址不能为空。"));
    }
    // 带 "://" 的按原样解析（这样 ftp:// 之类会被下面的 scheme 检查拒掉，
    // 而不是被前置 https:// 后蒙混过关）；没带的按 https:// 处理
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    if with_scheme.len() > FETCH_URL_MAX_CHARS {
        return Err(AppError::invalid(format!(
            "网址超过 {} 字符上限，DeepSeek 抓取接口不收。",
            FETCH_URL_MAX_CHARS
        )));
    }
    let parsed = Url::parse(&with_scheme)
        .map_err(|_| AppError::invalid(format!("这不是一个能打开的网址：{trimmed}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::invalid(format!(
            "只支持 http/https（收到的是「{}」）。",
            parsed.scheme()
        )));
    }
    if !parsed.username().is_empty() {
        return Err(AppError::invalid(
            "网址里不能带用户名密码（DeepSeek 接口要求）。把凭据从 URL 里去掉再试。",
        ));
    }
    Ok(json!({ "url": parsed.as_str() }))
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 把搜索响应整理成给模型读的清单：按相关度排序，rerank_score 不低于 1.5 的标
/// 「高质量」，summary 缺失时退回 snippet。results 整个缺失说明响应形状变了，
/// 如实说，别装作搜到了。
pub(crate) fn summarize_search_results(v: &Value, query: &str) -> String {
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return "DeepSeek 搜索接口的响应里没有 results 字段（接口形状可能变了）。\
                可以稍后重试，或把这一情况告诉用户。"
            .into();
    };
    if results.is_empty() {
        return format!(
            "没搜到与「{query}」相关的结果。可以换个更具体的搜索词、去掉 site: 限定、\
             或用同义词再试一次；确实查不到就直说，不要编造。"
        );
    }
    let mut out = format!("「{query}」的搜索结果（{} 条，按相关度排序）：\n\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let title = str_field(r, "title");
        let url = str_field(r, "url");
        let site = str_field(r, "site_name");
        let date = str_field(r, "date").chars().take(10).collect::<String>();
        let score = r.get("rerank_score").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let quality = if score >= GOOD_SCORE { "（高质量）" } else { "" };
        let mut summary = str_field(r, "summary");
        if summary.is_empty() {
            summary = str_field(r, "snippet");
        }

        out.push_str(&format!("{}. {}{}\n   {}\n", i + 1, title, quality, url));
        let mut meta: Vec<String> = Vec::new();
        if !site.is_empty() {
            meta.push(site);
        }
        if !date.is_empty() {
            meta.push(date);
        }
        if !meta.is_empty() {
            out.push_str(&format!("   {}\n", meta.join(" ｜ ")));
        }
        if !summary.is_empty() {
            out.push_str(&format!("   {}\n", summary));
        }
        out.push('\n');
    }
    out.push_str(
        "需要细节就挑最相关的 1-2 条用 web_fetch 读全文（搜索/抓取都按次计费，别批量抓）；\
         回答里标注来源（标题 + 链接）。",
    );
    out
}

/// 从抓取响应里取 (标题, Markdown 正文)。空正文（反爬页 / 非文章页）如实报错，
/// 让模型换 URL 或让用户自己打开，别把空字符串当结果。
pub(crate) fn parse_fetch_response(v: &Value) -> AppResult<(String, String)> {
    let title = str_field(v, "title");
    let content = str_field(v, "content");
    if content.is_empty() {
        return Err(AppError::other(
            "这个页面没有返回正文（可能是反爬页或非文章页）。换一个 URL，\
             或建议用户在内置浏览器里直接打开。",
        ));
    }
    Ok((title, content))
}

/// DeepSeek 工具接口的统一 POST：Bearer 鉴权、单次超时、系统代理现取
/// （`http_fresh`，同模型请求的做法——reqwest 只在建客户端时读一次系统代理）、
/// 非 2xx 抽 /error/message 并按状态码给可操作的提示。
async fn deepseek_post(ctx: &ToolCtx, url: &str, body: Value) -> AppResult<Value> {
    let key = ctx.core.config_read().deepseek.api_key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::Denied(
            "DeepSeek API Key 还没配置（设置 → 联网搜索）。请引导用户去设置页配置后再试。".into(),
        ));
    }
    let http = ctx.core.http_fresh()?;
    let resp = http
        .post(url)
        .bearer_auth(&key)
        .timeout(DEEPSEEK_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::other(format!("DeepSeek 接口连不上：{e}（检查网络或系统代理设置）")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| crate::agent::provider::truncate(&text, 300));
        let hint = match status.as_u16() {
            401 | 403 => "（Key 不对或已失效，去「设置 → 联网搜索」检查）",
            402 => "（账户余额不足，需要去 DeepSeek 平台充值）",
            429 => "（触发限流，等一会儿再试，不要连续重试）",
            _ => "",
        };
        return Err(AppError::other(format!(
            "DeepSeek 接口返回 HTTP {status}：{msg}{hint}"
        )));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| AppError::other(format!("DeepSeek 接口的响应不是合法 JSON：{e}")))
}

pub struct WebFetch;

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    // 工坊里也要查规范、查文档，联网工具两种模式都给
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "抓取一个网页的全文（DeepSeek 服务端抓取，返回 Markdown 正文，本机抓不到的反爬页面通常也拿得到）。\
         配合 web_search 用：从搜索结果里挑 1-2 条最相关的读全文，别只凭摘要下结论。\
         按次计费，别批量抓。open_viewer=true 时同时在内置浏览器打开，让用户一起看。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "url": str_prop("要读全文的网页地址（web_search 结果里的 url），可以省略 https://"),
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
        ensure_deepseek_ready(ctx)?;
        let raw = arg_str_req(&input, "url")?;
        let max = arg_u32(&input, "max_chars").unwrap_or(30_000).clamp(1_000, 200_000) as usize;

        let body = build_fetch_body(&raw)?;
        let url = body["url"].as_str().unwrap_or_default().to_string();
        let resp = deepseek_post(ctx, FETCH_URL, body).await?;
        let (title, content) = parse_fetch_response(&resp)?;

        if arg_bool(&input, "open_viewer").unwrap_or(false) {
            let tab = ctx
                .core
                .viewer
                .open(OpenRequest {
                    url: Some(url.clone()),
                    path: None,
                    topic_slug: None,
                    title: Some(title.clone()),
                    kind: None,
                    page: None,
                    new_tab: false,
                })
                .await?;
            let _ = tab;
            ctx.core
                .viewer
                .report_snapshot(
                    &tab.id,
                    Some(format!("# {}\n\n来源：{}\n\n{}", title, url, content)),
                    None,
                    None,
                    None,
                )
                .await;
            ctx.core.emit_viewer_sync().await;
        }

        let total = content.chars().count();
        let truncated = total > max;
        let shown: String = content.chars().take(max).collect();
        let mut out = format!(
            "标题：{}\n来源：{}\n（正文 {} 字{}）\n\n{}",
            title,
            url,
            total,
            if truncated { "，已截断" } else { "" },
            shown
        );
        if truncated {
            out.push_str("\n\n（正文被截断了。需要更多内容可以再调一次并调大 max_chars。）");
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

    // 工坊里也要查规范、查文档，联网工具两种模式都给
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "联网搜索（DeepSeek，按次计费）。什么时候用：遇到拿不准的事实、来源不清楚的引用、\
         时效性问题（最新进展、价格、版本、谁在何时提出）先搜再答。返回标题/链接/站点/日期\
         与给模型读的摘要（rerank 分高的标了「高质量」）。省着用：一次搜索够就别反复搜；\
         要读全文对具体 url 调 web_fetch。回答里标注来源。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("搜索词，写具体一点效果更好（支持 site: / -site: / 引号短语）"),
                "count": num_prop("结果条数 1-50，默认跟随设置（默认 8）"),
                "ori_question": str_prop("可选：触发搜索的用户原始问题（不超过 128 字符），帮搜索引擎理解意图"),
                "open_first": bool_prop("是否在内置浏览器打开第一条结果，默认 false"),
            }),
            &["query"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("联网搜索「{}」", arg_str(input, "query").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        ensure_deepseek_ready(ctx)?;
        let query = arg_str_req(&input, "query")?;
        let count = arg_u32(&input, "count")
            .unwrap_or_else(|| ctx.core.config_read().deepseek.count)
            .clamp(COUNT_MIN, COUNT_MAX);
        let ori = arg_str(&input, "ori_question");

        let body = build_search_body(&query, count, ori.as_deref())?;
        let resp = deepseek_post(ctx, SEARCH_URL, body).await?;
        let out = summarize_search_results(&resp, &sanitize_query(&query));

        if arg_bool(&input, "open_first").unwrap_or(false) {
            let url = resp
                .pointer("/results/0/url")
                .and_then(|u| u.as_str())
                .map(String::from);
            let title = resp
                .pointer("/results/0/title")
                .and_then(|u| u.as_str())
                .map(String::from);
            if let Some(url) = url {
                let tab = ctx
                    .core
                    .viewer
                    .open(OpenRequest {
                        url: Some(url),
                        path: None,
                        topic_slug: None,
                        title,
                        kind: None,
                        page: None,
                        new_tab: false,
                    })
                    .await?;
                let _ = tab;
                ctx.core.emit_viewer_sync().await;
            }
        }

        Ok(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_query() {
        // 压掉多余空白、去首尾
        assert_eq!(sanitize_query("  2026   高考  分数线  "), "2026 高考 分数线");
        // 截到官方上限 128 字符
        let long = "字".repeat(200);
        assert_eq!(sanitize_query(&long).chars().count(), QUERY_MAX_CHARS);
        // site: / 引号短语原样保留
        assert_eq!(
            sanitize_query("site:who.int \"疾病命名\""),
            "site:who.int \"疾病命名\""
        );
    }

    #[test]
    fn builds_search_body() {
        let long_question = "用户问了个很长的问题。".repeat(20);
        let body = build_search_body(" 治疗指南 ", 99, Some(&long_question)).unwrap();
        assert_eq!(body["query"], "治疗指南");
        assert_eq!(body["count"], 50, "count 夹到上限 50");
        assert_eq!(body["need_content"], false, "按次计费：默认不要全文");
        let ori = body["ori_question"].as_str().unwrap();
        assert!(ori.chars().count() <= 128, "ori_question 也要截到 128");

        // count 下限与 ori_question 省略
        let body = build_search_body("x", 0, None).unwrap();
        assert_eq!(body["count"], 1);
        assert!(body.get("ori_question").is_none(), "空 ori_question 不写进请求体");

        // 清洗后为空是参数错误
        assert!(build_search_body("   ", 8, None).is_err());
    }

    #[test]
    fn summarizes_search_results() {
        let resp = json!({
            "results": [
                {
                    "title": "高血压防治指南",
                    "url": "https://example.com/a",
                    "site_name": "示例站",
                    "date": "2026-10-01T08:00:00Z",
                    "rerank_score": 2.2,
                    "summary": "这是给模型读的摘要。"
                },
                {
                    "title": "另一篇",
                    "url": "https://example.com/b",
                    "rerank_score": 1.0,
                    "snippet": "只有 snippet 的结果。"
                }
            ]
        });
        let out = summarize_search_results(&resp, "高血压指南");
        assert!(out.contains("2 条"), "{out}");
        assert!(out.contains("（高质量）"), "rerank 高的结果要标出来：{out}");
        assert!(out.contains("https://example.com/a"));
        assert!(out.contains("2026-10-01"), "日期取到天：{out}");
        assert!(out.contains("这是给模型读的摘要。"));
        // summary 缺失退回 snippet
        assert!(out.contains("只有 snippet 的结果。"));
        assert!(out.contains("web_fetch"), "结尾要指引深读方式：{out}");

        // 空结果给可操作的下一步
        let empty = summarize_search_results(&json!({ "results": [] }), "怪词");
        assert!(empty.contains("换个更具体的搜索词"), "{empty}");
        // results 整个缺失：如实说，不装作搜到了
        let broken = summarize_search_results(&json!({}), "怪词");
        assert!(broken.contains("results"), "{broken}");
    }

    #[test]
    fn builds_fetch_body() {
        // 补默认协议
        let body = build_fetch_body("example.com/a").unwrap();
        assert_eq!(body["url"], "https://example.com/a");
        // http / https 原样
        assert_eq!(build_fetch_body("http://a.com").unwrap()["url"], "http://a.com/");
        assert_eq!(build_fetch_body("https://a.com").unwrap()["url"], "https://a.com/");
        // 非 http(s) 拒绝
        assert!(build_fetch_body("ftp://a.com").is_err());
        // 带用户名密码拒绝（官方要求）
        assert!(build_fetch_body("https://user:pass@a.com").is_err());
        // 超长拒绝
        let long = format!("https://a.com/{}", "x".repeat(9000));
        assert!(build_fetch_body(&long).is_err());
        // 乱串拒绝（空 host、带空格的「域名」——中文域名本身是合法 IDN，拦不了也不必拦，
        // DeepSeek 抓不到时工具会如实报错）
        assert!(build_fetch_body("http://").is_err());
        assert!(build_fetch_body("不是 网址").is_err());
    }

    #[test]
    fn parses_fetch_response() {
        let (title, content) =
            parse_fetch_response(&json!({ "title": "  一篇文章  ", "content": "  # 正文  " })).unwrap();
        assert_eq!(title, "一篇文章");
        assert_eq!(content, "# 正文");
        // 空正文（反爬页）：报错而不是把空串当结果
        assert!(parse_fetch_response(&json!({ "title": "x", "content": "  " })).is_err());
        assert!(parse_fetch_response(&json!({})).is_err());
    }
}
