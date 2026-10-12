//! 远程接口：**桌面端那份前端产物** + 一个 IPC 桥 + 一条事件流。
//!
//! 手机上打开的不是另一套界面，而是同一份 `dist/`（编译进二进制的那个），
//! 只在 `<head>` 里多插一个 `shim.js`——它把 `window.__TAURI_INTERNALS__`
//! 换成「HTTP + SSE」的版本（见 `web/shim.js`）。所以：
//!
//! ```text
//! GET  /                    桌面端前端产物（+ shim），不需要令牌
//! GET  /assets/*  /shim.js  同上（静态资源一律不需要令牌：里面没有秘密）
//! GET  /api/ping            活着吗
//! GET  /api/events          SSE：桌面端的事件原样扇过来（hub://agent 等）
//! POST /api/invoke          IPC 桥：{cmd, args} → 桌面端前端执行（白名单在 bridge.rs）
//! GET  /api/file            读工作区内的文件（消息里的图片、资料预览）
//! GET  /api/tab-bytes       读内置浏览器某个标签对应的原始字节（PDF / 图片）
//! ```
//!
//! 分界线还是老规矩：**`/api/*` 一律要令牌，静态页面不要**（页面里没有秘密，
//! 秘密都在接口后面；而且「打开链接先看到一句人话」比一个 401 JSON 好懂得多）。
//!
//! 为什么不自己维护一套「手机版接口」：那等于两套实现、两处参数名、两个
//! 需要同步的能力边界。现在手机上点「发送」走的就是桌面端那个 `agent_send`，
//! 审批、复习打分、开标签页同理——**同一批函数，同一份对话文件**。

use super::bridge;
use super::http::{Request, Response};
use super::{http, RemoteService};
use crate::state::AppCore;
use serde_json::json;
use std::sync::Arc;

/// 路由结果：普通响应，或「这条连接交给 SSE」。
pub enum Route {
    Reply(Response),
    Sse,
}

pub async fn route(core: &Arc<AppCore>, remote: &Arc<RemoteService>, req: Request) -> Route {
    // ---------------- 静态部分：不需要令牌 ----------------
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => return Route::Reply(page(core)),
        ("GET", "/shim.js") => {
            return Route::Reply(Response::new(
                200,
                "application/javascript; charset=utf-8",
                super::SHIM_JS,
            ))
        }
        ("GET", "/icon.svg") => {
            return Route::Reply(Response::new(200, "image/svg+xml", super::WEB_ICON))
        }
        ("GET", "/manifest.webmanifest") => return Route::Reply(manifest()),
        // 手机浏览器有时会自己来问一句，回个 204 省得它把 404 写进控制台
        ("GET", "/favicon.ico") => return Route::Reply(Response::new(204, "image/x-icon", Vec::new())),
        ("GET", path) if path.starts_with("/assets/") => return Route::Reply(asset(core, path)),
        _ => {}
    }

    // ---------------- 以下全部要令牌 ----------------
    let expected = core.config_read().remote.token;
    let got = req.token().unwrap_or_default();
    if !http::token_matches(&expected, got) {
        if remote.note_auth_failure() {
            return Route::Reply(Response::error(
                429,
                "令牌错误次数太多，等一分钟再试（重新扫一次桌面上的二维码即可）",
            ));
        }
        return Route::Reply(Response::error(
            401,
            "需要访问令牌：请重新扫一次桌面端「移动端远程控制」里的二维码",
        ));
    }
    remote.note_auth_success();

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/ping") => Route::Reply(Response::json(&json!({
            "ok": true,
            "name": "学习中枢",
            "version": env!("CARGO_PKG_VERSION"),
        }))),
        ("GET", "/api/events") => Route::Sse,
        ("POST", "/api/invoke") => Route::Reply(invoke(core, remote, &req).await),
        ("GET", "/api/file") => Route::Reply(file(core, &req)),
        ("GET", "/api/tab-bytes") => Route::Reply(tab_bytes(core, &req).await),
        _ => Route::Reply(Response::error(404, format!("没有这个接口：{} {}", req.method, req.path))),
    }
}

/// 手机页面：把桌面端那份产物原样送出去，只在 `<head>` 里插三样东西。
///
/// 为什么插的是「移动端 meta + shim」而不是改产物：
/// - `viewport` 那份产物里已经有（`width=device-width`），手机不会当桌面页缩着看；
/// - `apple-mobile-web-app-*` 与 `manifest` 让「添加到主屏幕」之后是个全屏应用；
/// - `referrer: no-referrer` 是安全相关的：**令牌就在地址栏里**，
///   页面里的任何外链（用户点开的资料链接、演示卡片）都不该把地址带出去；
/// - shim 必须在产物之前跑，它提供的是产物启动就要用的 `__TAURI_INTERNALS__`。
fn page(core: &Arc<AppCore>) -> Response {
    let Some(asset) = core.app.asset_resolver().get("/index.html".to_string()) else {
        return Response::error(500, "取不到前端产物（dist 里没有 index.html）");
    };
    let html = String::from_utf8_lossy(&asset.bytes).into_owned();
    let head = concat!(
        "<meta name=\"referrer\" content=\"no-referrer\" />",
        "<meta name=\"color-scheme\" content=\"dark light\" />",
        "<meta name=\"apple-mobile-web-app-capable\" content=\"yes\" />",
        "<meta name=\"apple-mobile-web-app-title\" content=\"学习中枢\" />",
        "<link rel=\"manifest\" href=\"/manifest.webmanifest\" />",
        "<link rel=\"icon\" href=\"/icon.svg\" type=\"image/svg+xml\" />",
        "<link rel=\"apple-touch-icon\" href=\"/icon.svg\" />",
        "<script src=\"/shim.js\"></script>",
    );
    let body = match html.find("</head>") {
        Some(at) => format!("{}{head}{}", &html[..at], &html[at..]),
        // 产物里没有 </head>（不该发生）：插在最前面也不会更糟
        None => format!("{head}{html}"),
    };
    Response::new(200, "text/html; charset=utf-8", body)
}

/// 产物里的静态资源（`/assets/index-xxx.js` 之类）。走 Tauri 的资源解析器，
/// 拿到的是**同一份编译进二进制的文件**，所以手机上跑的一定和电脑上同版本。
fn asset(core: &Arc<AppCore>, path: &str) -> Response {
    match core.app.asset_resolver().get(path.to_string()) {
        Some(asset) => Response::new(200, asset.mime_type.clone(), asset.bytes.clone())
            // 文件名带内容哈希：改了内容就是新地址，可以放心长期缓存
            .with_header("Cache-Control", "private, max-age=31536000, immutable"),
        None => Response::error(404, format!("没有这个资源：{path}")),
    }
}

/// PWA 清单：手机可以「添加到主屏幕」，打开就是全屏，像个小应用。
///
/// `start_url` 里**不带令牌**（清单是可以不鉴权拿到的，塞进去等于把令牌挂在门口）；
/// 页面首次通过二维码打开时把令牌存进 localStorage，之后从桌面图标进来自己接上。
fn manifest() -> Response {
    Response::new(
        200,
        "application/manifest+json; charset=utf-8",
        serde_json::to_vec(&json!({
            "name": "学习中枢 · 远程控制",
            "short_name": "学习中枢",
            "start_url": "/?src=pwa",
            "scope": "/",
            "display": "standalone",
            "background_color": "#12131a",
            "theme_color": "#12131a",
            "icons": [
                { "src": "/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }
            ]
        }))
        .unwrap_or_default(),
    )
}

/// IPC 桥的入口：白名单（[`bridge::check`]）→ 交给桌面端前端执行 → 原样返回。
///
/// 返回的信封是 `{ok, value}` / `{ok:false, error}`，和 Tauri 的 invoke 语义对齐
/// （成功给值、失败给字符串，前端的 `errText` 直接能显示）。
async fn invoke(core: &Arc<AppCore>, remote: &Arc<RemoteService>, req: &Request) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        cmd: String,
        #[serde(default)]
        args: serde_json::Value,
    }
    let body: Body = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, format!("请求格式不对：{e}")),
    };
    if let Err(why) = bridge::check(&body.cmd, &body.args) {
        return Response::error(403, why);
    }
    match bridge::invoke(core, &remote.bridge, body.cmd, body.args).await {
        Ok(out) if out.ok => Response::json(&json!({ "ok": true, "value": out.payload })),
        // 命令自己报的错：**不写日志**（用户点错了按钮不该刷一屏），
        // 但要原样把中文原因给手机，界面上的 toast 直接显示它。
        Ok(out) => Response::json(&json!({ "ok": false, "error": payload_text(&out.payload) })),
        Err(e) => Response::error(500, e.to_string()),
    }
}

/// 桌面端回话里的错误载荷：Tauri 的 invoke 失败时抛的是字符串，
/// 但插件命令可能抛结构化对象——统一折成一行能显示的文本。
fn payload_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 读工作区内的一个文件（消息里的图片、资料预览都走这里）。
///
/// 路径必须是**工作区相对路径**，经 `resolve_in_root` 归一化后校验前缀——
/// 远程接口多一个能读任意文件的口子，就等于把「令牌泄露」的后果从
/// 「能聊天」放大成「能拖走整台电脑的文件」。
fn file(core: &Arc<AppCore>, req: &Request) -> Response {
    let Some(rel) = req.query_get("path").map(str::trim).filter(|s| !s.is_empty()) else {
        return Response::error(400, "缺少 path 参数");
    };
    let root = core.config_read().workspace_root.clone();
    let path = match crate::paths::resolve_in_root(&root, rel) {
        Ok(p) => p,
        Err(e) => return Response::error(403, format!("路径不在工作区里：{e}")),
    };
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Response::error(404, "文件不存在（可能已被删除）"),
    };
    // 上限 32MB：手机看一张截图/一页 PDF 足够，别让人顺着接口把整个工作区拖走
    const MAX_FILE: u64 = 32 * 1024 * 1024;
    if meta.len() > MAX_FILE {
        return Response::error(413, "这个文件太大了，手机上就别看了");
    }
    match std::fs::read(&path) {
        Ok(bytes) => Response::new(200, content_type(&path), bytes)
            .with_header("Cache-Control", "private, max-age=600"),
        Err(e) => Response::error(500, format!("读文件失败：{e}")),
    }
}

/// 内置浏览器某个标签对应的原始字节（PDF / 图片）——手机上 pdf.js 要用它。
///
/// 为什么单独开一条 HTTP 出口而不是走 IPC 桥：那个命令回的是**原始字节**
/// （上限 160MB），塞不进 JSON 事件通道；这里直接流字节，也顺便绕开了 base64。
/// 路径解析和命令共用 `ViewerService::load_bytes`，所以手机上打开的 PDF
/// 和电脑上打开的**一定是同一个文件**（含「引用里带第三章」那套容错）。
async fn tab_bytes(core: &Arc<AppCore>, req: &Request) -> Response {
    let Some(tab_id) = req.query_get("tab").map(str::trim).filter(|s| !s.is_empty()) else {
        return Response::error(400, "缺少 tab 参数");
    };
    let ws = core.workspace();
    let resolve = |slug: &str| ws.resolve(slug).map(|t| t.dir);
    match core.viewer.load_bytes(tab_id, resolve).await {
        Ok((bytes, mime)) => Response::new(200, mime, bytes)
            .with_header("Cache-Control", "private, max-age=600"),
        Err(e) => Response::error(404, e.to_string()),
    }
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_cover_what_messages_carry() {
        use std::path::Path;
        assert_eq!(content_type(Path::new("a/b.png")), "image/png");
        assert_eq!(content_type(Path::new("讲义.PDF")), "application/pdf");
        assert_eq!(content_type(Path::new("x.JPEG")), "image/jpeg");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }

    /// 错误载荷可能是字符串（命令报错）也可能是对象（插件命令），
    /// 两种都得折成一行能显示的文本，别把 JSON 原样糊到 toast 上。
    #[test]
    fn error_payload_becomes_text() {
        assert_eq!(payload_text(&json!("没有打开主题")), "没有打开主题");
        assert_eq!(payload_text(&json!({ "code": 1 })), "{\"code\":1}");
    }
}
