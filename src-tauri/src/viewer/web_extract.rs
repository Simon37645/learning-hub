//! 网页正文提取（Reader 模式）：HTML → Markdown。
//!
//! 为什么不用现成库：agent 需要的是「干净、带结构、能直接进上下文」的文本。
//! 这里用一个打分启发式挑出正文容器，再把 DOM 转成 Markdown，
//! 保留标题层级/列表/代码块/链接，够模型理解，也够人读。
//!
//! 实现上统一在 `NodeRef` 上递归，需要元素能力时再 `ElementRef::wrap`，
//! 这样文本节点与元素节点可以用同一套遍历代码。

use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct Extracted {
    pub title: String,
    pub markdown: String,
    pub text: String,
    /// 正文里出现的外链，agent 可以顺着继续读
    pub links: Vec<(String, String)>,
}

/// 需要整块丢掉的噪音节点。
const NOISE: &str = "script,style,noscript,svg,iframe,canvas,template,link,meta,\
                     nav,header,footer,aside,form,button,input,select,textarea,\
                     [aria-hidden=true],[role=navigation],[role=banner],[role=contentinfo],\
                     [class*=cookie],[class*=sidebar],[class*=advert],[class*=ad-],[class*=share],\
                     [class*=comment],[class*=related],[class*=menu],[class*=breadcrumb],\
                     [id*=comment],[id*=sidebar],[id*=advert]";

const CANDIDATES: &str = "article,main,[role=main],#content,.content,#main,.main,\
                          .post,.article,.article-body,.entry-content,.markdown-body,\
                          .post-content,.doc-content,#article,body";

const MAX_DEPTH: usize = 64;

/// 主入口。`base_url` 用于把相对链接补全。
pub fn extract(html_source: &str, base_url: &str) -> Extracted {
    let mut doc = Html::parse_document(html_source);
    let base = Url::parse(base_url).ok();

    let title = doc
        .select(&sel("title"))
        .next()
        .map(|e| norm_ws(&e.text().collect::<String>()))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            doc.select(&sel("h1"))
                .next()
                .map(|e| norm_ws(&e.text().collect::<String>()))
                .filter(|t| !t.is_empty())
        })
        .unwrap_or_else(|| "未命名页面".to_string());

    strip_noise(&mut doc);

    let mut md = String::new();
    match pick_container(&doc) {
        Some(id) => {
            if let Some(node) = doc.tree.get(id) {
                render_block(node, &mut md, 0, base.as_ref());
            }
        }
        None => {
            if let Some(body) = doc.select(&sel("body")).next() {
                render_block(*body, &mut md, 0, base.as_ref());
            }
        }
    }

    let md = tidy(&md);
    Extracted {
        title,
        text: markdown_to_text(&md),
        links: collect_links(&md),
        markdown: md,
    }
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("内置选择器必须可解析")
}

fn strip_noise(doc: &mut Html) {
    let selector = sel(NOISE);
    let ids: Vec<_> = doc.select(&selector).map(|e| e.id()).collect();
    for id in ids {
        if let Some(mut node) = doc.tree.get_mut(id) {
            node.detach();
        }
    }
}

/// 给候选容器打分，挑最像正文的那个，返回节点 id。
fn pick_container(doc: &Html) -> Option<ego_tree::NodeId> {
    let selector = sel(CANDIDATES);
    let mut best: Option<(f64, ego_tree::NodeId)> = None;
    for el in doc.select(&selector) {
        let text: String = el.text().collect();
        let text_len = text.trim().chars().count() as f64;
        if text_len < 80.0 {
            continue;
        }
        // 链接密度高的多半是导航/列表页
        let link_len: usize = el
            .select(&sel("a"))
            .map(|a| a.text().collect::<String>().chars().count())
            .sum();
        let link_ratio = (link_len as f64 / text_len).min(1.0);
        // 标点密度高的更像成段文字（中文用全角标点兜底）
        let punctuation = text
            .chars()
            .filter(|c| matches!(c, '。' | '，' | '！' | '？' | '.' | ',' | ';' | '；'))
            .count() as f64;
        let punct_bonus = (punctuation / text_len * 40.0).min(30.0);

        let name_bonus = match el.value().name() {
            "article" => 60.0,
            "main" => 40.0,
            "body" => 0.0,
            _ => 20.0,
        };

        let score = text_len * (1.0 - link_ratio) + punct_bonus + name_bonus;
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, el.id()));
        }
    }
    best.map(|(_, id)| id)
}

/// 块级渲染。
fn render_block(node: ego_tree::NodeRef<'_, Node>, out: &mut String, depth: usize, base: Option<&Url>) {
    if depth > MAX_DEPTH {
        return;
    }

    // 纯文本节点：直接保留（有些正文就是裸文本）
    if let Node::Text(t) = node.value() {
        let s = t.text.trim();
        if !s.is_empty() {
            out.push_str(s);
            out.push('\n');
        }
        return;
    }

    let Some(el) = ElementRef::wrap(node) else { return };

    match el.value().name() {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = el.value().name()[1..].parse::<usize>().unwrap_or(2).clamp(1, 6);
            let text = render_inline(node, base);
            if !text.trim().is_empty() {
                out.push('\n');
                out.push_str(&"#".repeat(level));
                out.push(' ');
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
        "p" | "figcaption" | "dd" | "dt" | "caption" => {
            let text = render_inline(node, base);
            if !text.trim().is_empty() {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
        "br" => out.push('\n'),
        "hr" => out.push_str("\n---\n\n"),
        "pre" => {
            let code = el.text().collect::<String>();
            let code = code.trim_matches('\n');
            let lang = el
                .select(&sel("code"))
                .next()
                .and_then(|c| c.value().attr("class"))
                .and_then(|c| {
                    c.split_whitespace()
                        .find_map(|t| t.strip_prefix("language-").map(String::from))
                })
                .unwrap_or_default();
            out.push_str("\n```");
            out.push_str(&lang);
            out.push('\n');
            out.push_str(code);
            out.push_str("\n```\n\n");
        }
        "blockquote" => {
            let mut inner = String::new();
            for child in node.children() {
                render_block(child, &mut inner, depth + 1, base);
            }
            for line in inner.trim().lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
        "ul" | "ol" => {
            let ordered = el.value().name() == "ol";
            let mut idx = 1;
            for child in node.children() {
                let is_li = ElementRef::wrap(child)
                    .map(|li| li.value().name() == "li")
                    .unwrap_or(false);
                if !is_li {
                    continue;
                }
                let mut item = String::new();
                for c in child.children() {
                    render_block(c, &mut item, depth + 1, base);
                }
                let bullet = if ordered { format!("{idx}. ") } else { "- ".to_string() };
                idx += 1;
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let mut lines = trimmed.lines();
                if let Some(first) = lines.next() {
                    out.push_str(&bullet);
                    out.push_str(first);
                    out.push('\n');
                    for l in lines {
                        if l.trim().is_empty() {
                            continue;
                        }
                        out.push_str("  ");
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
            out.push('\n');
        }
        "table" => {
            render_table(el, out, base);
            out.push('\n');
        }
        "img" => {
            if let Some(src) = el.value().attr("src") {
                let alt = el.value().attr("alt").unwrap_or("");
                out.push_str(&format!("\n![{}]({})\n\n", alt.trim(), absolutize(src, base)));
            }
        }
        // 行内容器直接当内联处理
        "span" | "a" | "strong" | "b" | "em" | "i" | "code" | "small" | "sub" | "sup" | "mark"
        | "label" | "time" => {
            let text = render_inline(node, base);
            if !text.trim().is_empty() {
                out.push_str(&text);
            }
        }
        _ => {
            for child in node.children() {
                render_block(child, out, depth + 1, base);
            }
            if matches!(
                el.value().name(),
                "div" | "section" | "article" | "main" | "body" | "td"
            ) {
                out.push('\n');
            }
        }
    }
}

/// 行内渲染：返回单行 Markdown。
fn render_inline(node: ego_tree::NodeRef<'_, Node>, base: Option<&Url>) -> String {
    let mut out = String::new();
    walk_inline(node, &mut out, base, 0);
    norm_ws(&out)
}

fn walk_inline(
    node: ego_tree::NodeRef<'_, Node>,
    out: &mut String,
    base: Option<&Url>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for child in node.children() {
        // 先把「这是什么」算出来，避免后面的递归与借用打架
        let kind: &'static str = match child.value() {
            Node::Text(_) => "text",
            Node::Element(e) => match e.name() {
                "br" => "br",
                "strong" | "b" => "strong",
                "em" | "i" => "em",
                "code" | "kbd" | "samp" => "code",
                "a" => "a",
                "img" => "img",
                "script" | "style" | "svg" | "noscript" | "iframe" | "template" => "skip",
                _ => "other",
            },
            _ => "skip",
        };

        match kind {
            "text" => {
                if let Node::Text(t) = child.value() {
                    out.push_str(&t.text);
                }
            }
            "br" => out.push(' '),
            "strong" | "em" => {
                let mut inner = String::new();
                walk_inline(child, &mut inner, base, depth + 1);
                let inner = inner.trim();
                if !inner.is_empty() {
                    if kind == "strong" {
                        out.push_str(&format!("**{inner}**"));
                    } else {
                        out.push_str(&format!("*{inner}*"));
                    }
                }
            }
            "code" => {
                let mut inner = String::new();
                walk_inline(child, &mut inner, base, depth + 1);
                let inner = inner.trim();
                if !inner.is_empty() {
                    // 内容里已有反引号时换用双反引号，避免截断
                    if inner.contains('`') {
                        out.push_str(&format!("`` {inner} ``"));
                    } else {
                        out.push_str(&format!("`{inner}`"));
                    }
                }
            }
            "a" => {
                let href = ElementRef::wrap(child)
                    .and_then(|el| el.value().attr("href").map(|s| s.to_string()))
                    .unwrap_or_default();
                let mut inner = String::new();
                walk_inline(child, &mut inner, base, depth + 1);
                let text = inner.trim();
                let url = absolutize(&href, base);
                if url.is_empty() || url.starts_with("javascript:") {
                    out.push_str(text);
                } else if text.is_empty() {
                    out.push_str(&url);
                } else if text == url {
                    out.push_str(text);
                } else {
                    out.push_str(&format!("[{text}]({url})"));
                }
            }
            "img" => {
                if let Some(el) = ElementRef::wrap(child) {
                    if let Some(src) = el.value().attr("src") {
                        let alt = el.value().attr("alt").unwrap_or("");
                        out.push_str(&format!("![{}]({})", alt.trim(), absolutize(src, base)));
                    }
                }
            }
            "skip" => {}
            _ => walk_inline(child, out, base, depth + 1),
        }
    }
}

fn render_table(el: ElementRef<'_>, out: &mut String, base: Option<&Url>) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for tr in el.select(&sel("tr")) {
        let cells: Vec<String> = tr
            .select(&sel("th,td"))
            .map(|c| render_inline(*c, base).replace('|', "\\|").trim().to_string())
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return;
    }
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    // 第一行当表头；原表如果没有 th 也照样当表头用，读起来更清楚
    for (i, row) in rows.iter().enumerate() {
        let mut line = String::from("|");
        for c in 0..width {
            line.push(' ');
            line.push_str(row.get(c).map(|s| s.as_str()).unwrap_or(""));
            line.push_str(" |");
        }
        out.push_str(&line);
        out.push('\n');
        if i == 0 {
            let mut sep = String::from("|");
            for _ in 0..width {
                sep.push_str(" --- |");
            }
            out.push_str(&sep);
            out.push('\n');
        }
    }
}

fn absolutize(href: &str, base: Option<&Url>) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if href.starts_with('#') || href.starts_with("mailto:") || href.starts_with("javascript:") {
        return href.to_string();
    }
    match base.and_then(|b| b.join(href).ok()) {
        Some(u) => u.to_string(),
        None => href.to_string(),
    }
}

/// 压缩空白：行尾空格去掉、连续空行最多留一个。
pub fn tidy(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank = 0;
    for line in md.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank = 0;
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn norm_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// 去掉 Markdown 记号，得到纯文本（用于全文检索与 token 估算）。
pub fn markdown_to_text(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_code = false;
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        let mut s = t.trim_start_matches('#').trim().to_string();
        if s.starts_with('>') {
            s = s.trim_start_matches('>').trim().to_string();
        }
        if s.starts_with("- ") || s.starts_with("* ") {
            s = s[2..].to_string();
        }
        if let Some(rest) = s.strip_prefix("| ") {
            s = rest.trim_end_matches(" |").replace(" | ", " ");
        }
        s = s.replace("**", "").replace('`', "");
        out.push_str(&s);
        out.push('\n');
    }
    tidy(&out)
}

fn collect_links(md: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let bytes = md.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 找 "]("
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] != b')' {
                j += 1;
            }
            let url = &md[i + 2..j.min(md.len())];
            if url.starts_with("http") {
                let start = md[..i].rfind('[').unwrap_or(i);
                let text = md[start + 1..i].to_string();
                links.push((text, url.to_string()));
            }
            i = j;
        }
        i += 1;
        if links.len() > 200 {
            break;
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_article() {
        let html = r#"
        <html><head><title>测试页</title></head>
        <body>
          <nav><a href="/a">导航一</a><a href="/b">导航二</a></nav>
          <article>
            <h1>正文章节</h1>
            <p>这是<strong>正文</strong>的第一段，包含足够多的文字来判断这是文章主体内容而不是导航区域。</p>
            <p>第二段，带<a href="/rel/path">相对链接</a>和一点标点。这里再多写一些字。</p>
            <ul><li>要点一</li><li>要点二</li></ul>
            <pre><code class="language-python">print(1)</code></pre>
          </article>
          <footer>版权</footer>
        </body></html>"#;
        let e = extract(html, "https://example.com/post");
        assert_eq!(e.title, "测试页");
        assert!(e.markdown.contains("正文章节"));
        assert!(e.markdown.contains("**正文**"), "got: {}", e.markdown);
        assert!(e.markdown.contains("https://example.com/rel/path"));
        assert!(e.markdown.contains("- 要点一"));
        assert!(e.markdown.contains("```python"));
        assert!(!e.markdown.contains("导航一"));
        assert!(!e.markdown.contains("版权"));
    }

    #[test]
    fn text_strips_marks() {
        let t = markdown_to_text("# 标题\n\n- 项 **加粗**\n");
        assert!(t.contains("标题"));
        assert!(t.contains("项 加粗"));
    }
}
