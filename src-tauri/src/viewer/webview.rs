//! 远端网页的原生渲染：每个网页标签对应一个挂在主窗口上的子 WebView。
//!
//! 为什么不用 iframe：Bing / GitHub / 知乎这些站点会发 `X-Frame-Options` 或
//! `CSP frame-ancestors` 拒绝被嵌入，主 WebView 里的 iframe 只能画出一张
//! 「拒绝了我们的连接请求」。子 WebView 是**顶层浏览上下文**（真 WebView2），
//! 不受这些响应头约束——JS 首页也能正常跑，不再被迫切到服务端提取的阅读模式。
//!
//! 设计与代价：
//! - 子 WebView 浮在主 WebView 的 DOM 之上，盖不住它（Modal / 大图 / Toast 都会被压住），
//!   所以前端在显示它、弹出浮层、退出专注模式这几个时机必须显式调
//!   `viewer_webview_set_visible` 让它让路；bounds 由前端量宿主矩形（CSS px），
//!   这里按 `scale_factor` 换算成物理像素再 `set_bounds`。
//! - 回调（导航 / 标题 / 加载 / 弹窗）在事件循环线程触发，**只做同步判断**，
//!   状态更新与事件全部丢给 `tauri::async_runtime::spawn`，绝不阻塞、绝不直接 await。
//! - 回调捕获 `AppHandle` + `Arc<ViewerService>`，**不捕获 `Arc<AppCore>`**——
//!   AppCore 马上要持有本模块的 WebviewManager，再捕获它就是引用环。
//! - 安全边界：子 WebView 的 label（`web-<tab_id>`）不在 capabilities 的 windows
//!   列表里，拿不到任何 IPC 权限；`on_navigation` 再挡一层，只放行 http/https
//!   （`http://ipc.localhost` 这类内部地址一并拒绝），弹窗一律拒绝并转交系统浏览器。

use crate::error::{AppError, AppResult};
use crate::viewer::ViewerService;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, Wry};
use tauri_plugin_opener::OpenerExt;

/// 前端量出来的宿主矩形（CSS 像素，相对视口）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectCss {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// CSS 像素 → 物理像素。x/y/w/h 全部四舍五入；负的宽高按 0 处理。
///
/// 独立成纯函数：domain 无关，可脱离 Tauri 单测（换算错了 webview 会整体错位半行）。
pub fn to_physical_rect(scale: f64, r: RectCss) -> (i32, i32, u32, u32) {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let x = (r.x * s).round() as i32;
    let y = (r.y * s).round() as i32;
    let w = (r.w.max(0.0) * s).round() as u32;
    let h = (r.h.max(0.0) * s).round() as u32;
    (x, y, w, h)
}

/// 宽或高不足 2 个 CSS 像素视为「没有可见区域」（面板被收起 / 宽度拖到极限），
/// 此时直接 `hide()` 而不设 bounds——把一个 1×1 的原生层摆上来只会闪黑边。
pub fn is_tiny(r: RectCss) -> bool {
    r.w < 2.0 || r.h < 2.0
}

/// 只放行普通网页；`tauri://`、`ipc://` 等应用内部协议一律拒绝。
///
/// 子 WebView 本就不在 capabilities 的 windows 列表里（IPC 权限为零），
/// 这里是第二道闸：万一页面里真有代码试图导航到内部地址，导航直接不发生。
pub fn navigation_allowed(u: &url::Url) -> bool {
    match u.scheme() {
        // WebView2 上应用 IPC 走 http://ipc.localhost，Host 也要挡掉
        "http" | "https" => u.host_str() != Some("ipc.localhost"),
        _ => false,
    }
}

/// 一个字符串 URL 是否是可以交给系统浏览器的普通网页。
fn is_web_url(s: &str) -> bool {
    url::Url::parse(s).map(|u| navigation_allowed(&u)).unwrap_or(false)
}

/// tab_id → 子 WebView 句柄。`Webview` 是可 Clone 的轻句柄，存这里就能
/// 在任意命令里对它 navigate / set_bounds / hide / close。
///
/// `create_lock` 把「创建」串行化：前端 StrictMode 下 effect 会跑两遍，
/// 两个 ensure 并发进来会在同一个 label 上建出两个子 WebView（第二个直接报错）。
#[derive(Default)]
pub struct WebviewManager {
    webviews: Mutex<HashMap<String, tauri::Webview<Wry>>>,
    create_lock: Mutex<()>,
}

impl WebviewManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn main_window(app: &AppHandle) -> AppResult<tauri::Window<Wry>> {
        // 注意拿的是 Window 而不是 WebviewWindow：add_child 挂在 Window 上
        app.get_window("main")
            .ok_or_else(|| AppError::other("找不到主窗口，原生网页视图无处安放（请重启应用再试）"))
    }

    /// 确保某个标签的子 WebView 存在、对齐到宿主矩形。
    ///
    /// `reload == true` 时强制刷新。网址不在这里管：一个标签固定一个网址
    /// （换网址是 viewer_open 开新标签），页面自己的跳转由 webview 自己走，
    /// 我们不往回拨——SPA 的 pushState 不会回报成 tab.url，比对比出来的
    /// 永远是「该回去」，重新 ensure 一次就把用户弹回旧地址。
    pub fn ensure(
        &self,
        app: &AppHandle,
        service: &Arc<ViewerService>,
        tab_id: &str,
        url: &str,
        css: RectCss,
        reload: bool,
    ) -> AppResult<()> {
        let window = Self::main_window(app)?;
        let scale = window
            .scale_factor()
            .map_err(|e| AppError::other(format!("读取屏幕缩放失败：{e}")))?;
        let (px, py, pw, ph) = to_physical_rect(scale, css);
        let parsed: url::Url = url
            .parse()
            .map_err(|e| AppError::invalid(format!("网址无法解析：{e}")))?;

        // 先看有没有现成的（拿到句柄就放手，别攥着 map 锁调 dispatcher）；只认 reload
        let existing = self.webviews.lock().get(tab_id).cloned();
        if let Some(wv) = existing {
            if reload {
                wv.reload()
                    .map_err(|e| AppError::other(format!("刷新网页失败：{e}")))?;
            }
            return self.align(&wv, px, py, pw, ph, true);
        }

        // 创建路径：串行化，避免并发 ensure 在同一 label 上建出两个子 WebView
        let _guard = self.create_lock.lock();
        // 拿到锁再看一眼：可能别的请求刚建好（同样只认 reload，理由见上）
        if let Some(wv) = self.webviews.lock().get(tab_id).cloned() {
            if reload {
                wv.reload().map_err(|e| AppError::other(format!("刷新网页失败：{e}")))?;
            }
            return self.align(&wv, px, py, pw, ph, true);
        }

        let builder = Self::builder(app, service, tab_id, &label_of(tab_id), parsed)?;
        let wv = window
            .add_child(
                builder,
                Position::Physical(PhysicalPosition::new(px, py)),
                Size::Physical(PhysicalSize::new(pw, ph)),
            )
            .map_err(|e| AppError::other(format!("创建原生网页视图失败：{e}。本次将退回内嵌方式渲染，功能不受影响")))?;
        self.webviews.lock().insert(tab_id.to_string(), wv.clone());
        if pw < 2 || ph < 2 {
            wv.hide().map_err(|e| AppError::other(format!("隐藏网页视图失败：{e}")))?;
        }
        Ok(())
    }

    /// 组装子 WebView 的构建器：所有回调都只做同步判断 + spawn，见模块头注释。
    ///
    /// 闭包里捕获的是 `AppHandle` + `Arc<ViewerService>`，**不是 `Arc<AppCore>`**——
    /// AppCore 持有 WebviewManager，再捕获它就是引用环。
    fn builder(
        app: &AppHandle,
        service: &Arc<ViewerService>,
        tab_id: &str,
        label: &str,
        url: url::Url,
    ) -> AppResult<WebviewBuilder<Wry>> {
        let service = service.clone();
        let b = WebviewBuilder::new(label, tauri::WebviewUrl::External(url))
            .on_navigation({
                let app = app.clone();
                let service = service.clone();
                let tab_id = tab_id.to_string();
                move |u: &url::Url| {
                    let allow = navigation_allowed(u);
                    if allow {
                        // 导航即「页面要换了」：更新标签 url 并清掉旧正文快照，
                        // 之后 agent 的 viewer_read / viewer_get_content 会重新提取新页面
                        spawn_state_update(
                            app.clone(),
                            service.clone(),
                            tab_id.clone(),
                            Some(u.to_string()),
                            None,
                            None,
                        );
                    }
                    allow
                }
            })
            .on_document_title_changed({
                let app = app.clone();
                let service = service.clone();
                let tab_id = tab_id.to_string();
                move |_wv, title| {
                    spawn_state_update(app.clone(), service.clone(), tab_id.clone(), None, Some(title), None);
                }
            })
            .on_page_load({
                let app = app.clone();
                let service = service.clone();
                let tab_id = tab_id.to_string();
                move |_wv, payload| {
                    let loading = matches!(payload.event(), PageLoadEvent::Started);
                    // 把最终 URL 一并报上去：重定向之后标签上的旧地址才追得上
                    spawn_state_update(
                        app.clone(),
                        service.clone(),
                        tab_id.clone(),
                        Some(payload.url().to_string()),
                        None,
                        Some(loading),
                    );
                }
            })
            .on_new_window({
                let app = app.clone();
                move |u: url::Url, _features| {
                    // 弹窗一概不允许出现在应用里：转交系统浏览器开
                    let app = app.clone();
                    let url = u.to_string();
                    tauri::async_runtime::spawn(async move {
                        if is_web_url(&url) {
                            if let Err(e) = app.opener().open_url(&url, None::<&str>) {
                                eprintln!("[viewer] 转交系统浏览器打开弹窗失败：{e}");
                            }
                        }
                    });
                    tauri::webview::NewWindowResponse::Deny
                }
            });
        Ok(b)
    }

    /// 把子 WebView 对齐到物理像素矩形；太小就藏起来。
    ///
    /// `show` 只在 ensure（前端明确要显示）时为 true：单纯的 bounds 更新
    /// （窗口缩放、拖侧栏）不许顺手把藏着的原生层亮出来——那会把浮层盖住。
    fn align(&self, wv: &tauri::Webview<Wry>, px: i32, py: i32, pw: u32, ph: u32, show: bool) -> AppResult<()> {
        if pw < 2 || ph < 2 {
            return wv.hide().map_err(|e| AppError::other(format!("隐藏网页视图失败：{e}")));
        }
        wv.set_bounds(tauri::Rect {
            position: Position::Physical(PhysicalPosition::new(px, py)),
            size: Size::Physical(PhysicalSize::new(pw, ph)),
        })
        .map_err(|e| AppError::other(format!("调整网页视图位置失败：{e}")))?;
        if show {
            wv.show().map_err(|e| AppError::other(format!("显示网页视图失败：{e}")))?;
        }
        Ok(())
    }

    /// 只改位置与尺寸（前端 ResizeObserver / 拖侧栏时高频调用）。
    /// 不改可见性：显示与否由 `ensure` / `set_visible` 管。
    pub fn set_bounds_css(&self, app: &AppHandle, tab_id: &str, css: RectCss) -> AppResult<()> {
        let Some(wv) = self.webviews.lock().get(tab_id).cloned() else {
            return Ok(()); // 还没建：等下一次 ensure 一起带上
        };
        let window = Self::main_window(app)?;
        let scale = window
            .scale_factor()
            .map_err(|e| AppError::other(format!("读取屏幕缩放失败：{e}")))?;
        let (px, py, pw, ph) = to_physical_rect(scale, css);
        self.align(&wv, px, py, pw, ph, false)
    }

    /// 显示 / 隐藏（浮层弹出、切阅读模式、切标签时都要让路；页面状态保留）。
    pub fn set_visible(&self, tab_id: &str, visible: bool) -> AppResult<()> {
        let Some(wv) = self.webviews.lock().get(tab_id).cloned() else {
            return Ok(());
        };
        let r = if visible { wv.show() } else { wv.hide() };
        r.map_err(|e| AppError::other(format!("{}网页视图失败：{e}", if visible { "显示" } else { "隐藏" })))
    }

    pub fn reload(&self, tab_id: &str) -> AppResult<()> {
        let Some(wv) = self.webviews.lock().get(tab_id).cloned() else {
            return Ok(());
        };
        wv.reload().map_err(|e| AppError::other(format!("刷新网页失败：{e}")))
    }

    /// 关闭并销毁子 WebView（标签被关掉时调用；句柄同时从表里移除）。
    pub fn close(&self, tab_id: &str) {
        let Some(wv) = self.webviews.lock().remove(tab_id) else { return };
        if let Err(e) = wv.close() {
            eprintln!("[viewer] 关闭网页视图失败（可能是窗口已在退出）：{e}");
        }
    }

    /// 标签是否存在原生视图（调试 / 前端兜底判断用）。
    pub fn has(&self, tab_id: &str) -> bool {
        self.webviews.lock().contains_key(tab_id)
    }
}

fn label_of(tab_id: &str) -> String {
    // tab_id 是 UUID（含 -），拼上 web- 前缀就是合法 label；不会被用户数据污染
    format!("web-{tab_id}")
}

/// 回调线程 → 异步任务：改 ViewerService 状态，有变化就发 Updated 事件给前端。
///
/// Updated 是现有事件，前端 handleViewerEvent 的 updated 分支会把新 TabView 写进
/// store（地址栏、标签标题随之自动刷新），不需要新事件变体。
fn spawn_state_update(
    app: AppHandle,
    service: Arc<crate::viewer::ViewerService>,
    tab_id: String,
    url: Option<String>,
    title: Option<String>,
    loading: Option<bool>,
) {
    tauri::async_runtime::spawn(async move {
        if let Some(tab) = service.apply_web_state(&tab_id, url, title, loading).await {
            let _ = app.emit(
                crate::agent::event::EVENT_VIEWER,
                crate::agent::event::ViewerEvent::Updated { tab },
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> RectCss {
        RectCss { x, y, w, h }
    }

    #[test]
    fn css_to_physical_rounds_each_component() {
        // 150% 缩放：0.5px 的取舍不能让整个 webview 错位
        assert_eq!(to_physical_rect(1.5, rect(10.2, 7.6, 401.3, 202.5)), (15, 11, 602, 304));
        // 100% 缩放就是原样取整
        assert_eq!(to_physical_rect(1.0, rect(3.4, 2.0, 100.0, 50.0)), (3, 2, 100, 50));
    }

    #[test]
    fn css_to_physical_guards_bad_input() {
        // 非有限缩放退回 1.0；负宽高按 0
        assert_eq!(to_physical_rect(0.0, rect(1.0, 1.0, 10.0, 10.0)), (1, 1, 10, 10));
        assert_eq!(to_physical_rect(f64::NAN, rect(1.0, 1.0, 10.0, 10.0)), (1, 1, 10, 10));
        assert_eq!(to_physical_rect(2.0, rect(-8.0, -4.0, -3.0, 5.0)), (-16, -8, 0, 10));
    }

    #[test]
    fn tiny_rects_are_hidden() {
        assert!(is_tiny(rect(0.0, 0.0, 0.0, 0.0)));
        assert!(is_tiny(rect(0.0, 0.0, 500.0, 1.5)));
        assert!(is_tiny(rect(0.0, 0.0, 1.99, 500.0)));
        assert!(!is_tiny(rect(0.0, 0.0, 2.0, 2.0)));
        assert!(!is_tiny(rect(0.0, 0.0, 320.0, 240.0)));
    }

    #[test]
    fn navigation_only_allows_plain_web() {
        let ok = |s: &str| navigation_allowed(&url::Url::parse(s).unwrap());
        assert!(ok("https://github.com/tauri-apps"));
        assert!(ok("http://example.com/x?y=1"));
        assert!(!ok("tauri://localhost"));
        assert!(!ok("ipc://localhost"));
        assert!(!ok("http://ipc.localhost")); // WebView2 的 IPC 通道长这样
        assert!(!ok("data:text/html,hi"));
        assert!(!ok("file:///C:/Windows/win.ini"));
        assert!(!ok("about:blank"));
    }
}
