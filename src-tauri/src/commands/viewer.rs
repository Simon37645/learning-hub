//! 内置浏览器命令：前端渲染 + 状态回写。

use crate::agent::event::ViewerEvent;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::viewer::{OpenRequest, PageText, TabView, ViewerKind, ViewerSnapshot};
use tauri::ipc::Response;
use tauri::State;

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
        if let Ok(topic) = core.workspace().resolve(&slug) {
            req.path = Some(crate::paths::strip_locator_if_missing(&topic.dir, &path));
        }
    }
    let tab = core.viewer.open(req).await?;
    core.emit_viewer_sync().await;
    Ok(TabView::from(&tab))
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
    core.viewer
        .report_snapshot(&tab_id, content, pages, total_pages, error)
        .await;
    // 让前端把「已读 N 字」这类信息刷新一下
    if let Some(t) = core.viewer.find(&tab_id).await {
        core.emit_viewer(ViewerEvent::Updated { tab: TabView::from(&t) });
    }
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
    let abs = crate::paths::resolve_in_root(&topic.dir, &rel)?;
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
    let abs = crate::paths::resolve_in_root(&topic.dir, &rel)?;
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
        let abs = crate::paths::resolve_in_root(&topic.dir, &rel)?;
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
