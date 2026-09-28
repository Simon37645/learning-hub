//! 出网能力：抓网页 + 联网搜索。
//!
//! agent 的「看网页」有两条路：
//! 1. 内置浏览器打开真实网页（前端 iframe，给人看）
//! 2. 这里抓下来转成 Markdown 正文（给模型读）
//! 两条路互补：人看的可能被 JS 渲染，模型读的需要干净。

use crate::error::{AppError, AppResult};
use crate::viewer::web_extract::{self, Extracted};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 多数站点会按 UA 屏蔽爬虫，这里用一个常见的桌面浏览器标识。
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

pub fn client(timeout_secs: u64) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(timeout_secs.max(5)))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(AppError::from)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedPage {
    pub url: String,
    pub final_url: String,
    pub title: String,
    pub markdown: String,
    pub text: String,
    pub content_type: String,
    pub bytes: usize,
    pub truncated: bool,
}

/// 抓取并提取正文。`max_chars` 限制返回的正文长度（防止把提示词撑爆）。
pub async fn fetch_readable(
    http: &reqwest::Client,
    url: &str,
    max_chars: usize,
) -> AppResult<FetchedPage> {
    let url = normalize_url(url)?;
    let resp = http.get(&url).send().await?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let raw = resp.bytes().await?;
    let bytes = raw.len();

    if !status.is_success() {
        return Err(AppError::other(format!("抓取失败：HTTP {status} — {final_url}")));
    }

    let charset = charset_from_content_type(&content_type);
    let html = decode_html(&raw, charset);

    // 纯文本 / Markdown 直接返回，不做 DOM 提取
    let looks_html = content_type.contains("html") || html.trim_start().starts_with('<');
    let extracted: Extracted = if looks_html {
        web_extract::extract(&html, &final_url)
    } else {
        let text = web_extract::tidy(&html);
        Extracted {
            title: final_url.clone(),
            markdown: text.clone(),
            text,
            links: Vec::new(),
        }
    };

    let (markdown, truncated) = if extracted.markdown.chars().count() > max_chars {
        (extracted.markdown.chars().take(max_chars).collect::<String>(), true)
    } else {
        (extracted.markdown.clone(), false)
    };

    Ok(FetchedPage {
        url: url.clone(),
        final_url,
        title: extracted.title,
        markdown,
        text: extracted.text,
        content_type,
        bytes,
        truncated,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}

/// 联网搜索。默认 DuckDuckGo 的轻量 HTML 版（不需要 key），失败时回退 Bing。
pub async fn search(
    http: &reqwest::Client,
    engine: &str,
    query: &str,
    limit: usize,
) -> AppResult<Vec<WebSearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Err(AppError::invalid("搜索词为空"));
    }
    let encoded = percent_encoding::utf8_percent_encode(q, percent_encoding::NON_ALPHANUMERIC).to_string();

    match engine {
        "bing" => fetch_bing(http, &encoded, limit).await,
        _ => match fetch_duckduckgo(http, &encoded, limit).await {
            Ok(hits) if !hits.is_empty() => Ok(hits),
            Ok(_) => fetch_bing(http, &encoded, limit).await,
            Err(e) => match fetch_bing(http, &encoded, limit).await {
                Ok(hits) if !hits.is_empty() => Ok(hits),
                _ => Err(e),
            },
        },
    }
}

async fn fetch_duckduckgo(
    http: &reqwest::Client,
    encoded: &str,
    limit: usize,
) -> AppResult<Vec<WebSearchHit>> {
    let url = format!("https://html.duckduckgo.com/html/?q={encoded}");
    let body = http.get(&url).send().await?.text().await?;
    let doc = scraper::Html::parse_document(&body);

    let result_sel = sel(".result, .web-result");
    let link_sel = sel(".result__a, a.result__a");
    let snip_sel = sel(".result__snippet");

    let mut hits = Vec::new();
    for block in doc.select(&result_sel) {
        let Some(a) = block.select(&link_sel).next() else { continue };
        let title = clean(&a.text().collect::<String>());
        let href = a.value().attr("href").unwrap_or("");
        let Some(target) = decode_ddg_link(href) else { continue };
        let snippet = block
            .select(&snip_sel)
            .next()
            .map(|s| clean(&s.text().collect::<String>()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        hits.push(WebSearchHit { title, url: target, snippet, engine: "duckduckgo".into() });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

async fn fetch_bing(
    http: &reqwest::Client,
    encoded: &str,
    limit: usize,
) -> AppResult<Vec<WebSearchHit>> {
    let url = format!("https://www.bing.com/search?q={encoded}&setlang=zh-CN");
    let body = http.get(&url).send().await?.text().await?;
    let doc = scraper::Html::parse_document(&body);

    let item_sel = sel("li.b_algo");
    let link_sel = sel("h2 a");
    let snip_sel = sel("p");

    let mut hits = Vec::new();
    for block in doc.select(&item_sel) {
        let Some(a) = block.select(&link_sel).next() else { continue };
        let title = clean(&a.text().collect::<String>());
        let href = a.value().attr("href").unwrap_or("").to_string();
        if !href.starts_with("http") {
            continue;
        }
        let snippet = block
            .select(&snip_sel)
            .next()
            .map(|s| clean(&s.text().collect::<String>()))
            .unwrap_or_default();
        hits.push(WebSearchHit { title, url: href, snippet, engine: "bing".into() });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

fn sel(s: &str) -> scraper::Selector {
    scraper::Selector::parse(s).expect("内置选择器必须可解析")
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// DuckDuckGo 的结果链接是 `//duckduckgo.com/l/?uddg=<urlencoded>`，需要还原。
fn decode_ddg_link(href: &str) -> Option<String> {
    if href.is_empty() {
        return None;
    }
    let full = if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_string()
    };
    let parsed = url::Url::parse(&full).ok()?;
    for (k, v) in parsed.query_pairs() {
        if k == "uddg" {
            return Some(v.to_string());
        }
    }
    if parsed.host_str() == Some("duckduckgo.com") {
        return None; // 站内链接，不是搜索结果
    }
    Some(full)
}

pub fn normalize_url(input: &str) -> AppResult<String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(AppError::invalid("网址为空"));
    }
    let with_scheme = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if s.contains("://") {
        return Err(AppError::invalid(format!("不支持的协议：{s}")));
    } else {
        format!("https://{s}")
    };
    let parsed = url::Url::parse(&with_scheme).map_err(|e| AppError::invalid(format!("网址无法解析：{e}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string()),
        other => Err(AppError::invalid(format!("不支持的协议：{other}"))),
    }
}

fn charset_from_content_type(ct: &str) -> Option<&str> {
    ct.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("charset=")
            .or_else(|| part.strip_prefix("charset ="))
    })
}

/// 字节 → 文本。优先用 HTTP 头里的 charset，其次嗅探 `<meta charset>`，
/// 最后按 UTF-8 → GBK 的顺序试（中文站点大量还是 GBK）。
fn decode_html(bytes: &[u8], header_charset: Option<&str>) -> String {
    if let Some(label) = header_charset {
        if let Some(enc) = encoding_rs::Encoding::for_label(label.trim().as_bytes()) {
            let (text, _, _) = enc.decode(bytes);
            return text.into_owned();
        }
    }
    if let Some(sniffed) = sniff_meta_charset(bytes) {
        if let Some(enc) = encoding_rs::Encoding::for_label(sniffed.as_bytes()) {
            let (text, _, _) = enc.decode(bytes);
            return text.into_owned();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::GBK.decode(bytes);
            text.into_owned()
        }
    }
}

/// 从前 4KB 里嗅探 `<meta charset=...>`。
fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(4096)];
    let ascii: String = head.iter().map(|b| if b.is_ascii() { *b as char } else { ' ' }).collect();
    let lower = ascii.to_ascii_lowercase();
    let idx = lower.find("charset")?;
    let rest = &ascii[idx + 7..];
    let rest = rest.trim_start_matches(['=', ' ', '"', '\'']);
    let value: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com/");
        assert!(normalize_url("https://a.com/x").is_ok());
        assert!(normalize_url("file:///c:/x").is_err());
    }

    #[test]
    fn ddg_link_decoding() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=x";
        assert_eq!(decode_ddg_link(href).unwrap(), "https://example.com/a");
    }

    #[test]
    fn meta_charset_sniff() {
        let html = br#"<html><head><meta charset="gb2312"><title>x</title></head></html>"#;
        assert_eq!(sniff_meta_charset(html).as_deref(), Some("gb2312"));
    }
}
