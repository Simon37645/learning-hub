//! 网页操作工具：agent 的「浏览器之手」——对标外部浏览器操作类 MCP
//! （scan 页面 / 执行 JS / 截图）的思路，但**纯 Rust 随应用内置，零外部依赖**。
//!
//! 能力来自原生子 WebView 的两条程序化通道（见 `viewer::webview`）：
//! - `eval_js`：往页面里注入 JS，拿回完成值 —— `web_scan` / `web_eval` 在这上面；
//! - `cdp_call`：走 Chrome DevTools Protocol —— `web_screenshot` 用
//!   `Page.captureScreenshot` 截整页。
//!
//! 只对**远端网页**标签生效（本地 HTML / PDF / Markdown 一律报错，见
//! `require_remote_web_tab` 的三种文案）。与 `web_fetch` 的分工：那个拿的是
//! 「服务端渲染的正文副本」，这里操作的是**用户眼前这个活的页面**——
//! 登录态、滚动位置、点开之后的 DOM，都只有这条路上有。

use crate::agent::registry::{
    arg_bool, arg_str, arg_str_req, bool_prop, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

// 复用 viewer 工具的「用哪个标签」解析（显式 id > 当前激活标签），别复制一份两处漂移
use super::viewer::resolve_tab;

/// eval / 扫描的超时：页面没加载完时宁可快点失败，让模型决定重试还是换路。
const EVAL_WAIT: Duration = Duration::from_secs(12);

/// CDP 调用（截图）的超时：整页截图对长页面要多次合成，比 eval 宽松些。
const CDP_WAIT: Duration = Duration::from_secs(20);

/// eval 结果给模型的上限（字符）。一个 querySelectorAll 展开就能吐几十万字符，
/// 不截断的话一次调用就吃掉大半上下文预算。
const EVAL_RESULT_MAX: usize = 20_000;

// ---------------------------------------------------------------- web_scan

/// 注入页面的扫描脚本（立即执行函数，返回 JSON 字符串）。
///
/// 为什么这么写：
/// - **先 `JSON.stringify` 再返回**——eval 的完成值会被 JSON 编码一次，
///   字符串形式能原样穿越那一层，Rust 侧解一次引号就是干净的 JSON；
/// - **全程 try/catch**——Windows 上 WebView2 的 eval 会把页面异常**吞掉**
///   （完成值变成 null），想让模型知道「哪里没扫到」就只能自己接住自己报；
/// - **标记复用**——`data-lh-ref` 编号挂在 `window.__lhRefSeq` 上全局递增、
///   已标过的元素不重标：连着两次扫描时，前一次拿到的 ref 在页面上仍然有效；
/// - **不等待 DOMContentLoaded**——readyState 带回来即可，加载到哪算哪，
///   模型看到 readyState 是 "loading" 自己会决定等会儿再扫。
const SCAN_JS: &str = r#"(() => {
  const out = { title: '', url: '', readyState: '', text: '', headings: [], links: [], inputs: [] };
  try {
    out.title = String(document.title || '');
    out.url = String(location.href || '');
    out.readyState = String(document.readyState || '');
    try { out.text = String((document.body && document.body.innerText) || '').trim().slice(0, 6000); } catch (e) {}
    const clip = (s, n) => String(s == null ? '' : s).trim().replace(/\s+/g, ' ').slice(0, n);
    const ref = (el) => {
      let r = el.getAttribute('data-lh-ref');
      if (!r) {
        window.__lhRefSeq = (window.__lhRefSeq || 0) + 1;
        r = 'e' + window.__lhRefSeq;
        el.setAttribute('data-lh-ref', r);
      }
      return r;
    };
    try {
      for (const h of document.querySelectorAll('h1,h2,h3')) {
        const t = clip(h.innerText || h.textContent, 80);
        if (t) {
          out.headings.push(t);
          if (out.headings.length >= 20) break;
        }
      }
    } catch (e) {}
    try {
      const seen = new Set();
      for (const a of document.querySelectorAll('a[href]')) {
        const href = String(a.href || '');
        if (!href || seen.has(href)) continue;
        seen.add(href);
        out.links.push({ ref: ref(a), text: clip(a.innerText, 60), href: href });
        if (out.links.length >= 40 || seen.size > 2000) break;
      }
    } catch (e) {}
    try {
      for (const el of document.querySelectorAll('input,textarea,select,button,[onclick]')) {
        if (String(el.type) === 'hidden') continue;
        const r = el.getBoundingClientRect();
        if (r && (r.width < 2 || r.height < 2)) continue;
        out.inputs.push({
          ref: ref(el),
          tag: String(el.tagName || '').toLowerCase(),
          type: String(el.getAttribute('type') || ''),
          label: clip(el.getAttribute('placeholder') || el.value || el.innerText, 40),
          ariaLabel: String(el.getAttribute('aria-label') || '')
        });
        if (out.inputs.length >= 30) break;
      }
    } catch (e) {}
  } catch (e) {
    return JSON.stringify({ error: String((e && e.message) || e) });
  }
  return JSON.stringify(out);
})()"#;

pub struct WebScan;

#[async_trait]
impl Tool for WebScan {
    fn name(&self) -> &'static str {
        "web_scan"
    }

    // 和 viewer_* 一样按 tab_id 干活，不依赖主题：工坊里也能操作用户开着的网页
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "扫描内置浏览器里当前网页的结构化概览：标题、URL、正文节选、标题层级、链接列表、\
         可交互元素（输入框/按钮/下拉）。**每个链接和可交互元素都会被打上 data-lh-ref 标记**\
         （形如 e1、e2），后续用 web_eval 按 [data-lh-ref=\"eN\"] 定位去点击或输入。\
         要替用户在网页上完成一个操作（搜索、翻页、点按钮），先 scan 拿到 ref，再 eval 执行。\
         只对远端网页生效；本地 HTML / PDF / Markdown 不适用。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
            }),
            &[],
        )
    }

    fn summarize(&self, _input: &Value) -> String {
        "扫描网页结构".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;
        let raw = ctx
            .core
            .webviews
            .eval_js(&ctx.core.viewer, &tab.id, SCAN_JS, EVAL_WAIT)
            .await?;

        // eval 完成值是 JSON 编码：脚本返回的是字符串（页面 JSON），先解一层引号
        let page_json: String = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(_) => {
                return Ok(ToolOutput::err(format!(
                    "扫描结果不是预期的 JSON（页面可能还没加载完）。原始返回（截断）：\n{}",
                    truncate_chars(&raw, 2_000)
                )));
            }
        };
        let value: Value = serde_json::from_str(&page_json).map_err(|e| {
            AppError::other(format!(
                "扫描结果的 JSON 解析失败：{e}。原始内容（截断）：\n{}",
                truncate_chars(&page_json, 2_000)
            ))
        })?;
        if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
            return Err(AppError::other(format!("扫描脚本在页面上出错：{err}")));
        }

        let mut body = serde_json::to_string_pretty(&value)
            .map_err(|e| AppError::other(format!("结果序列化失败：{e}")))?;
        body.push_str(
            "\n\n提示：要点击/输入，用 web_eval 执行 \
             document.querySelector('[data-lh-ref=\"eN\"]').click()；\
             对输入框先 el.value = '…' 再 \
             el.dispatchEvent(new Event('input',{bubbles:true}))，\
             下拉框同理把 'input' 换成 'change'。",
        );
        Ok(ToolOutput::ok(body))
    }
}

// ---------------------------------------------------------------- web_eval

pub struct WebEval;

#[async_trait]
impl Tool for WebEval {
    fn name(&self) -> &'static str {
        "web_eval"
    }

    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "在内置浏览器的网页里执行任意 JS，返回**完成值**（最后一条表达式的值，JSON 序列化）。\
         点击、填表、滚动、读页面状态都在这里做。注意：这里不是函数体，`return x` 无效——\
         要拿值请把代码包成 `(() => { ...; return x })()`。先用 web_scan 拿元素的 \
         data-lh-ref 标记，再 document.querySelector('[data-lh-ref=\"eN\"]') 定位。\
         不要在里面写超长循环；页面异常在 Windows 上会被吞掉，自己 try/catch 才拿得到报错。\
         只对远端网页生效。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "code": str_prop("要执行的 JS 代码"),
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
            }),
            &["code"],
        )
    }

    fn summarize(&self, _input: &Value) -> String {
        "在网页里执行 JS".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let code = arg_str_req(&input, "code")?;
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;

        let raw = ctx
            .core
            .webviews
            .eval_js(&ctx.core.viewer, &tab.id, &code, EVAL_WAIT)
            .await?;

        // raw 本身就是完成值的 JSON 编码。解析一遍是为了漂亮打印（模型读结构化
        // JSON 更稳）；解析失败（理论上是非法 JSON 的完成值不存在）就原样截断返回，
        // 别让模型拿到一堆转义地狱。
        let mut out = match serde_json::from_str::<Value>(&raw) {
            Ok(v) => ToolOutput::json(&v),
            Err(_) => ToolOutput::ok(truncate_chars(&raw, EVAL_RESULT_MAX)),
        };
        if out.content.chars().count() > EVAL_RESULT_MAX {
            out.content = format!(
                "{}\n\n…（结果超过 {EVAL_RESULT_MAX} 字符已截断。精确取值：把代码改成只返回需要的字段）",
                out.content.chars().take(EVAL_RESULT_MAX).collect::<String>()
            );
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- web_screenshot

pub struct WebScreenshot;

#[async_trait]
impl Tool for WebScreenshot {
    fn name(&self) -> &'static str {
        "web_screenshot"
    }

    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "给内置浏览器里的网页**截一张图**交给你看（走 DevTools 的页面截图，不是截屏窗口）。\
         用来读渲染后的视觉信息：图表、 canvas 画的东西、排版、登录后的页面状态。\
         full_page=true 截整页（长页面会很长，按需用）。图片会附在结果里并落盘，用户也看得到。\
         只对远端网页生效；读文字优先用 web_scan / viewer_read（省得多）。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "tab_id": str_prop("标签页 id，默认当前激活的标签"),
                "full_page": bool_prop("是否截整页（含视口外的部分），默认 false"),
            }),
            &[],
        )
    }

    fn summarize(&self, _input: &Value) -> String {
        "截图网页".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let tab = resolve_tab(ctx, arg_str(&input, "tab_id")).await?;
        let params = if arg_bool(&input, "full_page").unwrap_or(false) {
            r#"{"format":"png","captureBeyondViewport":true}"#
        } else {
            r#"{"format":"png"}"#
        };
        let raw = ctx
            .core
            .webviews
            .cdp_call(&ctx.core.viewer, &tab.id, "Page.captureScreenshot", params, CDP_WAIT)
            .await?;

        let value: Value = serde_json::from_str(&raw)
            .map_err(|e| AppError::other(format!("截图结果解析失败：{e}")))?;
        let data = value
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                let why = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("页面没有返回图片");
                AppError::other(format!(
                    "截图失败（{why}）。把内置浏览器面板展开、窗口别最小化，等页面加载完再试一次。"
                ))
            })?;
        let bytes = crate::agent::attachment::b64_decode(data)?;
        if bytes.is_empty() {
            return Err(AppError::other(
                "截图返回了空图片。把内置浏览器面板展开、窗口别最小化，再试一次。",
            ));
        }

        let (width, height) = png_dimensions(&bytes).unwrap_or((0, 0));
        // 附件按工作区定位（和 pdf_screenshot 同一套约定：字节在磁盘，消息里只存路径）
        let attachments = crate::agent::attachment::Attachments::new(
            ctx.core.config_read().workspace_root.clone(),
        );
        let label = format!("{} 网页截图", tab.title);
        let stored = attachments.save(&label, &bytes)?;

        let mut content = format!(
            "已把「{}」截成图片交给你（见下方附件，{}×{} 像素）。以图为准回答视觉问题；\
             要读里面的文字也可以对着图读。",
            tab.title, width, height
        );
        if width == 0 {
            content.push_str("（图片尺寸没能解析出来，不影响查看）");
        }
        Ok(ToolOutput::with_images(
            content,
            vec![crate::agent::message::ToolImage {
                path: stored.rel,
                media_type: stored.media_type,
                name: label,
                bytes: stored.bytes,
                width: Some(width),
                height: Some(height),
            }],
        ))
    }
}

// ---------------------------------------------------------------- png_dimensions

/// 从 PNG 字节里解析尺寸（IHDR 块：字节 16..20 宽、20..24 高，大端）。
///
/// 纯函数、零依赖：`image` crate 只为读两个 u32 不划算，PNG 头是固定布局。
/// 解不出（坏文件 / 不是 PNG）返回 None，调用方按「尺寸未知」处理，不让截图白截。
pub fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if data.len() < 24 || data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((w, h))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…（已截断）");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_reads_ihdr() {
        // 最小合法头：签名 + 长度 + "IHDR" + 宽高（3×7）
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]); // IHDR 数据长度
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&7u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((3, 7)));

        // 大尺寸不受字节序摆布：0x0000_0001 × 0xFFFF_FFFF 是真实可表达的
        let mut big = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        big.extend_from_slice(&[0, 0, 0, 13]);
        big.extend_from_slice(b"IHDR");
        big.extend_from_slice(&1u32.to_be_bytes());
        big.extend_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(png_dimensions(&big), Some((1, u32::MAX)));
    }

    #[test]
    fn png_dimensions_rejects_garbage() {
        assert_eq!(png_dimensions(b""), None);
        assert_eq!(png_dimensions(b"not a png at all"), None);
        // 签名对但缺 IHDR
        let mut no_chunk = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        no_chunk.extend_from_slice(b"JFIF");
        no_chunk.extend_from_slice(&[0u8; 16]);
        assert_eq!(png_dimensions(&no_chunk), None);
        // 头 24 字节被截断
        let mut short = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        short.extend_from_slice(&[0, 0, 0, 13]);
        short.extend_from_slice(b"IH");
        assert_eq!(png_dimensions(&short), None);
    }

    #[test]
    fn eval_result_truncation_is_char_safe() {
        // 中文按字符截，不劈开 UTF-8
        let s = "汉".repeat(30);
        let t = truncate_chars(&s, 10);
        assert_eq!(t.chars().count(), 10 + 6); // 截断提示自身 6 个字符
        assert!(t.ends_with("…（已截断）"));
        assert_eq!(truncate_chars("短文本", 10), "短文本");
    }

    /// target_tab 在工具里只用于类型占位（实际逻辑在 WebviewManager 里），
    /// 这里确认它没有产生未使用告警级别的死代码。
    #[test]
    fn scan_js_marks_refs_and_caps() {
        // 关键约定写死在脚本里：标记属性名、计数器挂载点、上限
        assert!(SCAN_JS.contains("data-lh-ref"));
        assert!(SCAN_JS.contains("__lhRefSeq"));
        assert!(SCAN_JS.contains("6000"));
        assert!(SCAN_JS.contains("JSON.stringify"));
    }
}
