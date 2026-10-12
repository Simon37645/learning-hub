//! 移动端远程接口。
//!
//! 这一层只做三件事：**鉴权 → 分发 → 复用已有的领域逻辑**。
//! 所有实际动作（发消息、审批、读对话）都调桌面端同一批函数，不另写一套——
//! 手机上看到的对话与电脑上的是同一份 jsonl，手机上点「允许」走的是同一个
//! `respond_approval`（谁先答谁生效）。
//!
//! 能力边界（刻意的）：能读主题/对话、发消息、审批工具、取消当前轮、读工作区内的文件。
//! **不能**改配置、不能开关技能/MCP、不能删主题、不能写文件——
//! 那些是「坐在电脑前才该做的事」，远程只给「用」不给「改」。

use super::http::{Request, Response};
use super::{http, RemoteService};
use crate::agent::registry::AgentMode;
use crate::agent::TurnRequest;
use crate::error::AppResult;
use crate::state::AppCore;
use serde_json::json;
use std::sync::Arc;
use tauri::Emitter;

/// 路由结果：普通响应，或「这条连接交给 SSE」。
pub enum Route {
    Reply(Response),
    Sse,
}

pub async fn route(core: &Arc<AppCore>, remote: &Arc<RemoteService>, req: Request) -> Route {
    // 静态页面与图标不需要令牌：页面里没有秘密（秘密都在 /api/* 里），
    // 而「打开链接先看到一句『需要令牌』」比一个 401 JSON 好懂得多。
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => return Route::Reply(Response::html(super::WEB_INDEX)),
        ("GET", "/app.css") => {
            return Route::Reply(Response::new(200, "text/css; charset=utf-8", super::WEB_CSS))
        }
        ("GET", "/app.js") => {
            return Route::Reply(Response::new(
                200,
                "application/javascript; charset=utf-8",
                super::WEB_JS,
            ))
        }
        ("GET", "/icon.svg") => {
            return Route::Reply(Response::new(200, "image/svg+xml", super::WEB_ICON))
        }
        ("GET", "/manifest.webmanifest") => return Route::Reply(manifest()),
        // 手机浏览器有时会自己来问一句，回个 204 省得它把 404 写进控制台
        ("GET", "/favicon.ico") => return Route::Reply(Response::new(204, "image/x-icon", Vec::new())),
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
        ("GET", "/api/state") => Route::Reply(match state(core, remote) {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(500, e.to_string()),
        }),
        ("GET", "/api/chats") => Route::Reply(match chats(core, &req) {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(400, e.to_string()),
        }),
        ("GET", "/api/transcript") => Route::Reply(match transcript(core, &req) {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(400, e.to_string()),
        }),
        ("GET", "/api/file") => Route::Reply(file(core, &req)),
        ("GET", "/api/events") => Route::Sse,
        ("POST", "/api/send") => Route::Reply(match send(core, &req).await {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(400, e.to_string()),
        }),
        ("POST", "/api/cancel") => {
            let turn_id = req.json::<serde_json::Value>().ok().and_then(|v| {
                v.get("turnId").and_then(|x| x.as_str()).map(|s| s.to_string())
            });
            match turn_id {
                Some(id) => Route::Reply(Response::json(&json!({ "cancelled": core.agent.cancel(&id) }))),
                None => Route::Reply(Response::error(400, "缺少 turnId")),
            }
        }
        ("POST", "/api/render") => Route::Reply(match render(core, remote, &req).await {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(400, e.to_string()),
        }),
        ("POST", "/api/approve") => Route::Reply(match approve(core, &req) {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(400, e.to_string()),
        }),
        ("POST", "/api/new_chat") => Route::Reply(Response::json(&json!({
            "chatId": uuid::Uuid::new_v4().to_string()
        }))),
        _ => Route::Reply(Response::error(404, format!("没有这个接口：{} {}", req.method, req.path))),
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

/// 主题列表 + 运行中的轮次 + 一点环境信息。
fn state(core: &Arc<AppCore>, remote: &Arc<RemoteService>) -> AppResult<serde_json::Value> {
    let cfg = core.config_read();
    let topics = core.workspace().list()?;
    let running: Vec<serde_json::Value> = core
        .agent
        .running_turns()
        .into_iter()
        .map(|t| json!({ "turnId": t.turn_id, "runningMs": t.started_at_ms }))
        .collect();
    let profile = cfg.active_profile().ok().map(|p| json!({
        "name": p.name,
        "model": p.model,
    }));

    Ok(json!({
        "app": {
            "name": "学习中枢",
            "version": env!("CARGO_PKG_VERSION"),
            "userName": cfg.user_name,
        },
        "topics": topics,
        "running": running,
        "profile": profile,
        "permissionMode": cfg.agent.permission_mode,
        "permissionLabel": cfg.agent.permission_mode.label(),
        "clients": remote.clients(),
    }))
}

fn chats(core: &Arc<AppCore>, req: &Request) -> AppResult<serde_json::Value> {
    let slug = req.query_get("topic").map(str::trim).filter(|s| !s.is_empty());
    // 远程只管学习模式：工坊是「坐在电脑前造东西」的场景，手机上不铺开
    let items = crate::agent::chats::collect(core, slug, AgentMode::Study, true)?;
    Ok(serde_json::to_value(items).unwrap_or_default())
}

fn transcript(core: &Arc<AppCore>, req: &Request) -> AppResult<serde_json::Value> {
    let chat = req
        .query_get("chat")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::error::AppError::invalid("缺少 chat 参数"))?;
    let slug = req.query_get("topic").map(str::trim).filter(|s| !s.is_empty());
    let messages = crate::agent::read_transcript(core, slug, chat)?;
    Ok(serde_json::to_value(messages).unwrap_or_default())
}

/// 读工作区内的一个文件（消息里的图片、工具卡片里的 PDF 页截图都走这里）。
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
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/plain; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// 发一条消息（可以带图）。和桌面端 `agent_send` 是同一条路。
async fn send(core: &Arc<AppCore>, req: &Request) -> AppResult<serde_json::Value> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        #[serde(default)]
        topic: Option<String>,
        chat: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        images: Vec<crate::agent::ImageUpload>,
    }
    let body: Body = req.json()?;
    if body.text.trim().is_empty() && body.images.is_empty() {
        return Err(crate::error::AppError::invalid("消息不能为空"));
    }
    let turn = TurnRequest {
        topic_slug: body.topic.clone().filter(|s| !s.trim().is_empty()),
        chat_id: body.chat.clone(),
        text: body.text,
        stage: None,
        attachments: Vec::new(),
        images: body.images,
        mode: AgentMode::Study,
    };
    let (turn_id, message) = core.agent.start_turn(core.clone(), turn)?;
    // 让桌面端的侧栏也跟上（手机上开的新对话要出现在电脑的对话列表里）——
    // 桌面端自己发消息时是本地插一条，远程这条只能靠事件通知
    core.emit_topics(crate::agent::event::TopicsEvent::Refresh);
    Ok(json!({
        "turnId": turn_id,
        "chatId": body.chat,
        "message": message,
    }))
}

/// 请桌面端把一段图（Mermaid）渲染成 SVG。
///
/// 手机上不装渲染器：桌面端有一份**唯一**的 mermaid 入口（见 AGENTS.md 里
/// 「Mermaid 的渲染必须收口到一处」），拉一份进手机页面既重复又会版本漂移。
/// 这里的等待有上限（10 秒）——桌面端没回应就说清楚，别让手机上一直转圈。
async fn render(
    core: &Arc<AppCore>,
    remote: &Arc<RemoteService>,
    req: &Request,
) -> AppResult<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct Body {
        #[serde(default)]
        lang: String,
        source: String,
    }
    let body: Body = req.json()?;
    let lang = body.lang.trim().to_ascii_lowercase();
    if lang != "mermaid" {
        return Err(crate::error::AppError::invalid(format!("画不了这种图：{lang}")));
    }
    if body.source.trim().is_empty() {
        return Err(crate::error::AppError::invalid("图是空的"));
    }
    // 和 web_eval 一样给个上限：手机页面没必要把几万字的图塞过来
    if body.source.len() > 40_000 {
        return Err(crate::error::AppError::invalid("这张图太大了（超过 40000 字符）"));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let rx = remote.render.begin(&id);
    // 交给桌面端前端：那边有渲染器，也有「唯一 id + 串行队列」那套规矩
    let sent = core.app.emit(
        crate::agent::event::EVENT_REMOTE,
        &crate::agent::event::RemoteRenderRequest { id: id.clone(), lang, source: body.source },
    );
    if sent.is_err() {
        remote.render.cancel(&id);
        return Err(crate::error::AppError::other("桌面端没接住这个渲染请求（窗口可能正在关闭）"));
    }

    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(out)) if out.ok => Ok(json!({ "svg": out.payload })),
        Ok(Ok(out)) => Err(crate::error::AppError::invalid(format!(
            "桌面端画不出来：{}",
            out.payload
        ))),
        Ok(Err(_)) => Err(crate::error::AppError::other("渲染请求被取消了")),
        Err(_) => {
            remote.render.cancel(&id);
            Err(crate::error::AppError::other(
                "桌面端 10 秒内没画出这张图（它可能正忙，或者窗口关着）",
            ))
        }
    }
}

fn approve(core: &Arc<AppCore>, req: &Request) -> AppResult<serde_json::Value> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        request_id: String,
        allow: bool,
        #[serde(default)]
        always: bool,
    }
    let body: Body = req.json()?;
    let handled = core.agent.respond_approval(&body.request_id, body.allow, body.always);
    Ok(json!({ "handled": handled }))
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
}
