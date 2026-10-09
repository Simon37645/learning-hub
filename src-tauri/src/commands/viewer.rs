//! 内置浏览器命令：前端渲染 + 状态回写。

use crate::agent::event::ViewerEvent;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::viewer::{OpenRequest, PageText, RenderedPage, TabView, ViewerKind, ViewerSnapshot};
use tauri::ipc::Response;
use serde::Serialize;
use tauri::State;

/// 标签页的「主题内相对路径」：老的标签可能存着带章节定位的写法（例如 `…pdf 第三章`，
/// 那是模型把章节当页码写了），加载时再归一一次，历史标签也能自愈，不必手动关掉重开。
fn tab_rel(core: &std::sync::Arc<crate::state::AppCore>, slug: &str, rel: &str) -> String {
    match core.workspace().resolve(slug) {
        Ok(topic) => crate::paths::strip_locator_if_missing(&topic.dir, rel),
        Err(_) => rel.to_string(),
    }
}

/// 单次送进前端的文件大小上限（防止把内存吃爆）。
const MAX_INLINE_BYTES: u64 = 160 * 1024 * 1024;

#[tauri::command]
pub async fn viewer_snapshot(state: State<'_, AppState>) -> AppResult<ViewerSnapshot> {
    Ok(state.0.viewer.snapshot().await)
}

#[tauri::command]
pub async fn viewer_open(state: State<'_, AppState>, mut req: OpenRequest) -> AppResult<TabView> {
    let core = state.0.clone();
    // 引用里常见的「路径 + 章节」写法（例句：…pdf 第三章）：原路径存在就原样用，
    // 不存在才去掉尾巴上的定位词。放在这一层是因为只有这里拿得到工作区。
    if let (Some(slug), Some(path)) = (req.topic_slug.clone(), req.path.clone()) {
        req.path = Some(tab_rel(&core, &slug, &path));
    }
    let tab = core.viewer.open(req).await?;
    core.emit_viewer_sync().await;
    Ok(TabView::from(&tab))
}


#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCheck {
    /// 能不能嵌进内置浏览器
    pub embeddable: bool,
    /// 不能嵌的原因（直接给用户看）
    pub reason: String,
}

/// 检查网址能否被内嵌（见文件头注释）。
#[tauri::command]
pub async fn web_frame_check(state: State<'_, AppState>, url: String) -> AppResult<FrameCheck> {
    let core = state.0.clone();
    let url = crate::net::normalize_url(&url)?;
    // Range 只取 0 字节：只要响应头，不把整页拉下来
    let resp = core
        .http
        .get(&url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await?;
    let headers = resp.headers();

    if let Some(xfo) = headers.get("x-frame-options").and_then(|v| v.to_str().ok()) {
        let v = xfo.trim().to_ascii_lowercase();
        if v.contains("deny") || v.contains("sameorigin") {
            return Ok(FrameCheck {
                embeddable: false,
                reason: format!("站点声明 X-Frame-Options: {}", xfo.trim()),
            });
        }
    }

    if let Some(csp) = headers.get("content-security-policy").and_then(|v| v.to_str().ok()) {
        if let Some(dir) = csp
            .split(';')
            .map(str::trim)
            .find(|s| s.to_ascii_lowercase().starts_with("frame-ancestors"))
        {
            let value = dir.splitn(2, ' ').nth(1).unwrap_or("").trim().to_string();
            // 只有明确放开（含 *）才认为可嵌；'self' / 'none' / 具体域名都不行
            if !value.contains('*') {
                return Ok(FrameCheck {
                    embeddable: false,
                    reason: format!("站点声明 Content-Security-Policy: frame-ancestors {value}"),
                });
            }
        }
    }

    Ok(FrameCheck {
        embeddable: true,
        reason: String::new(),
    })
}
#[tauri::command]
pub async fn viewer_close(state: State<'_, AppState>, tab_id: String) -> AppResult<()> {
    let core = state.0.clone();
    core.viewer.close(&tab_id).await?;
    core.emit_viewer_sync().await;
    Ok(())
}

#[tauri::command]
pub async fn viewer_activate(state: State<'_, AppState>, tab_id: String) -> AppResult<TabView> {
    let core = state.0.clone();
    let tab = core.viewer.activate(&tab_id).await?;
    core.emit_viewer_sync().await;
    Ok(TabView::from(&tab))
}

#[tauri::command]
pub async fn viewer_set_visible(state: State<'_, AppState>, visible: bool) -> AppResult<()> {
    state.0.viewer.set_visible(visible).await;
    Ok(())
}

#[tauri::command]
pub async fn viewer_report_state(
    state: State<'_, AppState>,
    tab_id: String,
    page: Option<u32>,
    scroll: Option<f32>,
    total_pages: Option<u32>,
) -> AppResult<()> {
    state
        .0
        .viewer
        .report_state(&tab_id, page, scroll, total_pages)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn viewer_report_snapshot(
    state: State<'_, AppState>,
    tab_id: String,
    content: Option<String>,
    pages: Option<Vec<PageText>>,
    total_pages: Option<u32>,
    error: Option<String>,
) -> AppResult<()> {
    let core = state.0.clone();
    // 前端把逐页文本交上来了：存一份到主题里，知识库建索引时就能带上真页码
    // （Rust 侧的 pdf-extract 不保留分页，这是拿得到页码的唯一来源）
    if let Some(page_list) = &pages {
        if let Some(tab) = core.viewer.find(&tab_id).await {
            if let (Some(slug), Some(rel)) = (tab.topic_slug.clone(), tab.path.clone()) {
                if let Ok(topic) = core.workspace().resolve(&slug) {
                    if let Ok(abs) = crate::paths::resolve_in_root(&topic.dir, &rel) {
                        let rows: Vec<(u32, String)> =
                            page_list.iter().map(|p| (p.page, p.text.clone())).collect();
                        if let Err(e) = crate::kb::save_pages_cache(&topic.dir, &abs, &rows) {
                            eprintln!("[kb] 分页缓存写入失败 {}：{e}", abs.display());
                        }
                    }
                }
            }
        }
    }
    core.viewer
        .report_snapshot(&tab_id, content, pages, total_pages, error)
        .await;
    // 让前端把「已读 N 字」这类信息刷新一下
    if let Some(t) = core.viewer.find(&tab_id).await {
        core.emit_viewer(ViewerEvent::Updated { tab: TabView::from(&t) });
    }
    Ok(())
}

/// 前端把 `pdf_screenshot` 要的那一页交回来（PNG base64），或说明为什么渲染不了。
///
/// 这一趟是**请求-响应**：工具侧先登记一个等待位（`register_render_pending`），
/// 发 `render_request` 事件，然后在这里被唤醒。所以这个命令必须在没有图片时也能返回——
/// 前端渲染失败（页不存在、文档没加载出来）时报 `error`，工具会把它转成一句人话给模型。
#[tauri::command]
pub async fn viewer_report_render(
    state: State<'_, AppState>,
    request_id: String,
    data: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    error: Option<String>,
) -> AppResult<()> {
    let result = match (data, error) {
        (Some(b64), _) => crate::agent::attachment::b64_decode(&b64).map(|bytes| RenderedPage {
            data: bytes,
            width: width.unwrap_or(0),
            height: height.unwrap_or(0),
        }),
        (None, Some(e)) => Err(AppError::other(e)),
        (None, None) => Err(AppError::other("前端没有给出图片数据")),
    };
    state.0.viewer.report_render(&request_id, result);
    Ok(())
}

/// 读取标签页对应的文本内容（Markdown / 纯文本 / 本地 HTML）。
///
/// 顺手把正文报给 Rust 侧做快照，agent 之后 `viewer_read` 就能立刻拿到。
#[tauri::command]
pub async fn viewer_load_text(state: State<'_, AppState>, tab_id: String) -> AppResult<String> {
    let core = state.0.clone();
    let tab = core
        .viewer
        .find(&tab_id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{tab_id}")))?;

    // 远端网页由前端 iframe 直接渲染，不走这里
    let Some(slug) = tab.topic_slug.clone() else {
        return Err(AppError::invalid("这个标签页不是本地文件"));
    };
    let rel = tab
        .path
        .clone()
        .ok_or_else(|| AppError::invalid("标签页缺少文件路径"))?;
    let topic = core.workspace().resolve(&slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &tab_rel(&core, &slug, &rel))?;
    if !abs.exists() {
        return Err(AppError::NotFound(format!("文件不存在：{rel}")));
    }
    let meta = std::fs::metadata(&abs)?;
    if meta.len() > MAX_INLINE_BYTES {
        return Err(AppError::invalid("文件太大，请用系统程序打开"));
    }

    let text = crate::store::read_text(&abs)?;
    if tab.kind == ViewerKind::Markdown || tab.kind == ViewerKind::Text {
        core.viewer
            .report_snapshot(&tab_id, Some(text.clone()), None, None, None)
            .await;
    }
    Ok(text)
}

/// 读取标签页对应的二进制内容（PDF / 图片），原样交给前端渲染。
#[tauri::command]
pub async fn viewer_load_bytes(state: State<'_, AppState>, tab_id: String) -> AppResult<Response> {
    let core = state.0.clone();
    let tab = core
        .viewer
        .find(&tab_id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{tab_id}")))?;
    let slug = tab
        .topic_slug
        .clone()
        .ok_or_else(|| AppError::invalid("这个标签页不是本地文件"))?;
    let rel = tab
        .path
        .clone()
        .ok_or_else(|| AppError::invalid("标签页缺少文件路径"))?;
    let topic = core.workspace().resolve(&slug)?;
    let abs = crate::paths::resolve_in_root(&topic.dir, &tab_rel(&core, &slug, &rel))?;
    if !abs.exists() {
        return Err(AppError::NotFound(format!("文件不存在：{rel}")));
    }
    let meta = std::fs::metadata(&abs)?;
    if meta.len() > MAX_INLINE_BYTES {
        return Err(AppError::invalid(format!(
            "文件太大（{}），请用系统程序打开",
            crate::paths::human_size(meta.len())
        )));
    }
    let bytes = tokio::fs::read(&abs)
        .await
        .map_err(|e| AppError::Io(std::io::Error::new(e.kind(), format!("{rel} — {e}"))))?;
    Ok(Response::new(bytes))
}

/// agent 视角的正文（用于界面上的「agent 读到了什么」面板与阅读模式）。
#[tauri::command]
pub async fn viewer_get_content(state: State<'_, AppState>, tab_id: String) -> AppResult<String> {
    let core = state.0.clone();
    let tab = core
        .viewer
        .find(&tab_id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("标签页不存在：{tab_id}")))?;
    if !tab.content.trim().is_empty() {
        return Ok(tab.content);
    }
    // 还没有快照：本地文件即时提取，网页即时抓取
    if let (Some(slug), Some(rel)) = (tab.topic_slug.clone(), tab.path.clone()) {
        let topic = core.workspace().resolve(&slug)?;
        let abs = crate::paths::resolve_in_root(&topic.dir, &tab_rel(&core, &slug, &rel))?;
        let text = crate::agent::tools::fs::read_document(&abs).await?;
        core.viewer
            .report_snapshot(&tab_id, Some(text.clone()), None, None, None)
            .await;
        return Ok(text);
    }
    if let Some(url) = tab.url.clone() {
        let page = crate::net::fetch_readable(&core.http, &url, 200_000).await?;
        let body = format!("# {}\n\n来源：{}\n\n{}", page.title, page.final_url, page.markdown);
        core.viewer
            .report_snapshot(&tab_id, Some(body.clone()), None, None, None)
            .await;
        return Ok(body);
    }
    Ok(String::new())
}

/// 让前端重新加载某个标签（agent 改了文件之后用）。
#[tauri::command]
pub async fn viewer_reload(state: State<'_, AppState>, tab_id: String) -> AppResult<()> {
    let core = state.0.clone();
    core.viewer.set_loading(&tab_id, true, None).await;
    core.emit_viewer(ViewerEvent::Reload { tab_id });
    Ok(())
}

/// 打开设置里配置的主页。
#[tauri::command]
pub async fn viewer_open_home(state: State<'_, AppState>) -> AppResult<TabView> {
    let core = state.0.clone();
    let url = core.config_read().viewer.home_url.clone();
    let tab = core
        .viewer
        .open(OpenRequest {
            url: Some(url),
            ..Default::default()
        })
        .await?;
    core.emit_viewer_sync().await;
    Ok(TabView::from(&tab))
}
