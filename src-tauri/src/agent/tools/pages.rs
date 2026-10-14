//! 演示页：让 agent 写一个**能跑的本地网页**（动画、三维、可拖参数的机制演示），
//! 在内置浏览器里打开给用户看。
//!
//! 和对话里的 ```html 演示卡片的区别（两者都要，别混）：
//! - 卡片是 `sandbox=""` 的 iframe、**不执行脚本**，只适合纯 CSS/SVG 的小示意，
//!   好处是张开就看见、不用点；
//! - 这个工具产出的是**真网页**（`lhpage://` 协议给了它一个真 origin，见 `viewer::pages`）：
//!   JS、WebGL、`import` three.js、读同目录的数据文件都行，代价是要在内置浏览器里打开。
//!   机制类内容（分子结构、波动、算法步进、受力分析）值得这么做。
//!
//! 落盘位置：`<主题>/.hub/pages/<id>/`。放 `.hub` 下而不是 `lessons/`，是因为一个演示页
//! 往往是**一个目录**（index.html + 模块 + 数据），而 `lessons/` 那边是给人看的单文件；
//! 反正打开后标签页会一直在内置浏览器里，用户不依赖目录结构去找它。

use crate::agent::event::Risk;
use crate::agent::registry::{
    arg_bool, arg_str, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::store;
use crate::viewer::{pages, OpenRequest, ViewerKind};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 单个页面的文件数上限：一页演示不该有几百个文件，多了多半是模型在瞎写。
const MAX_FILES: usize = 60;
/// 单个文件大小上限（字符）：够放一份 three.js 场景 + 一堆数据了。
const MAX_FILE_CHARS: usize = 600_000;

pub struct PageCreate;

#[async_trait]
impl Tool for PageCreate {
    fn name(&self) -> &'static str {
        "page_create"
    }

    fn description(&self) -> &'static str {
        "建一个**会跑的**本地网页（动画 / 三维 / 可交互的机制演示），并在内置浏览器里打开给用户看。\n\
         与对话里的 ```html 演示卡片不同：这里的页面**真的执行 JavaScript**——能画 WebGL、\
         能用 requestAnimationFrame 做动画、能加按钮和滑块让用户拖、能 `import` 内置的 three.js\
         （离线可用：`import { Scene, PerspectiveCamera, WebGLRenderer, OrbitControls } from \"/_vendor/three.js\"`），\
         也能 `fetch('./data.json')` 读同目录的文件。\n\
         什么时候用它：机制讲不清、要动起来才看得见（分子/晶体结构、波与振动、受力与轨迹、\
         算法逐步执行、坐标系变换、参数怎么影响结果）。纯文字能说清的就别做页面。\n\
         页面怎么写好：① 自包含在一个目录里，入口是 index.html；② 页面里写清「这是什么、\
         怎么操作」（标题 + 一行说明 + 必要的图例），用户可能是隔几天回来看的；\
         ③ 深浅色都要看得清（背景用深色画布或按 prefers-color-scheme 适配）；\
         ④ 画布铺满窗口、跟随窗口大小变化；⑤ 加 OrbitControls 之类的交互，能让用户自己转着看。\
         注意：页面**不能联网取数据**（CSP 挡掉了 fetch/XHR 到外网；CDN 上的库仍可加载，\
         那是脚本请求）——要用的数据（JSON / CSV / 贴图）先写进同一个目录，页面用相对路径读。\n\
         写完一定要自查：用 web_eval 跑几行 JS 确认关键对象建出来了（例如 renderer 的 \
         domElement 宽度、场景里有多少个物体）、再用 web_screenshot 看一眼画面；\
         改完代码再调一次 page_create（同一个 id 就是覆盖重写并自动刷新）。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("页面标题（也是标签页名字）：例如「DNA 双螺旋结构」"),
                "html": str_prop("index.html 的内容。单文件页面就用它（最常用）"),
                "files": {
                    "type": "object",
                    "description": "多文件页面：{\"index.html\": \"…\", \"app.js\": \"…\", \"data.json\": \"…\"}。与 html 二选一（同时给以 files 为准）",
                    "additionalProperties": { "type": "string" }
                },
                "id": str_prop("目录名（可选）：同一个 id 再调一次＝覆盖重写。不传就按标题生成"),
                "open": { "type": "boolean", "description": "是否立刻在内置浏览器打开/刷新，默认 true" },
                "topic": str_prop("写到另一个主题时传它的名字（默认当前主题）"),
            }),
            &["title"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("建演示页「{}」", arg_str(&input, "title").unwrap_or_default())
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let title = arg_str(&input, "title").unwrap_or_default().trim().to_string();
        if title.is_empty() {
            return Err(AppError::invalid("title 不能为空——它是标签页名字，用户靠它认这个页面"));
        }
        let topic_arg = arg_str(&input, "topic");
        let topic = ctx.topic_or(topic_arg.as_deref())?;
        let slug = topic.slug();

        // 文件清单：files 优先，其次 html（= 一个 index.html）
        let mut files: Vec<(String, String)> = Vec::new();
        if let Some(Value::Object(map)) = input.get("files") {
            for (k, v) in map {
                let Some(text) = v.as_str() else {
                    return Err(AppError::invalid(format!("files 里「{k}」的内容必须是字符串")));
                };
                files.push((k.clone(), text.to_string()));
            }
        } else if let Some(html) = arg_str(&input, "html") {
            files.push(("index.html".into(), html));
        }
        if files.is_empty() {
            return Err(AppError::invalid(
                "需要 html（单文件）或 files（多文件，里面要有 index.html）之一",
            ));
        }
        if files.len() > MAX_FILES {
            return Err(AppError::invalid(format!("文件太多了（{} 个）", files.len())));
        }

        let id = match arg_str(&input, "id") {
            Some(raw) if !raw.trim().is_empty() => crate::studio::sanitize_id(&raw)?,
            _ => crate::studio::sanitize_id(&title)?,
        };
        let dir_rel = format!(".hub/pages/{id}");
        let dir_abs = crate::paths::resolve_in_root(&topic.dir, &dir_rel)?;
        crate::paths::ensure_dir(&dir_abs)?;

        let mut written: Vec<String> = Vec::new();
        for (rel, text) in &files {
            let rel_clean = clean_rel(rel)?;
            if text.chars().count() > MAX_FILE_CHARS {
                return Err(AppError::invalid(format!(
                    "「{rel_clean}」太大了（{} 字符）——单页演示不该这么大，拆成几个文件",
                    text.chars().count()
                )));
            }
            let abs = crate::paths::resolve_in_root(&dir_abs, &rel_clean)?;
            if let Some(parent) = abs.parent() {
                crate::paths::ensure_dir(parent)?;
            }
            store::atomic_write(&abs, text.as_bytes())?;
            written.push(rel_clean);
        }

        // 入口：优先 index.html，其次唯一的那个 .html
        let entry = written
            .iter()
            .find(|p| p.eq_ignore_ascii_case("index.html"))
            .cloned()
            .or_else(|| {
                let html: Vec<&String> = written
                    .iter()
                    .filter(|p| pages::is_page_file(Path::new(p)))
                    .collect();
                if html.len() == 1 {
                    Some(html[0].clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                AppError::invalid("页面里得有一个 index.html（或唯一的那个 .html）当入口，我按它打开")
            })?;

        let mut out = format!(
            "演示页已写入：{dir_rel}/{entry}（{slug} 主题，共 {} 个文件）\n",
            written.len()
        );
        if written.len() > 1 {
            out.push_str(&format!("同目录还有：{}\n", written.join("、")));
        }

        if !arg_bool(&input, "open").unwrap_or(true) {
            out.push_str("（按你的要求没有打开。用户要看时可以 viewer_open 这个路径。）\n");
            return Ok(ToolOutput::ok(out));
        }

        let abs_entry = crate::paths::resolve_in_root(&dir_abs, &entry)?;
        let file_rel = format!("{dir_rel}/{entry}");
        // 已经开过这个页面：重写之后要让前端刷新（否则用户看到的还是旧代码）
        let existed = ctx
            .core
            .viewer
            .list()
            .await
            .iter()
            .any(|t| t.path.as_deref() == Some(file_rel.as_str()) && t.topic_slug.as_deref() == Some(slug.as_str()));

        let mut req = OpenRequest {
            url: None,
            path: Some(file_rel.clone()),
            topic_slug: Some(slug.clone()),
            title: Some(title.clone()),
            kind: Some(ViewerKind::Web),
            page: None,
            new_tab: false,
        };
        pages::attach_page_url(&mut req, &abs_entry);
        let tab = ctx.core.viewer.open(req).await?;
        ctx.core.emit_viewer_sync().await;
        if existed {
            // 同一标签重开不会自动刷新，补一个 reload（前端收到就重载原生视图）
            ctx.core.emit_viewer(crate::agent::event::ViewerEvent::Reload { tab_id: tab.id.clone() });
        }

        out.push_str(&format!(
            "已在内置浏览器打开（标签 id：{}）。页面里的 JS 真的会跑，用户可以拖鼠标/点按钮交互。\n\
             自查（重要）：\n\
             - `web_eval`（tab_id=\"{0}\"）跑几行 JS，确认关键东西真建出来了：画布有没有、\
             有多宽（`document.querySelector('canvas')?.width`）、场景里有多少物体；\
             `<script type=\"module\">` 里的顶层变量不在 `window` 上，要自查就把关键对象\
             显式挂到 `window`（例如 `window.scene = scene`）；\n\
             - `web_screenshot`（tab_id=\"{0}\"）截一张图，自己看一眼画面对不对——空白画布、\
             全黑、坐标跑到屏幕外这三种是最常见的错；\n\
             - 有问题就改代码再调一次 page_create（同一个 id 覆盖重写，会自动刷新）。\n\
             自查通过后再告诉用户「可以看了、怎么操作」。",
            tab.id
        ));
        Ok(ToolOutput::ok(out))
    }
}

/// 页面内的相对路径：只允许老老实实的「目录/文件」，不许跳出页面目录。
fn clean_rel(raw: &str) -> AppResult<String> {
    let s = raw.trim().replace('\\', "/");
    let s = s.trim_start_matches('/').to_string();
    if s.is_empty() {
        return Err(AppError::invalid("文件名不能为空"));
    }
    if s.contains(':') {
        return Err(AppError::invalid(format!("文件名里不能有冒号：{raw}")));
    }
    for seg in s.split('/') {
        if seg == ".." {
            return Err(AppError::invalid(format!("不允许跳出页面目录：{raw}")));
        }
    }
    Ok(s)
}

/// 页面目录的绝对路径（给 `page_create` 之外的调用方用，例如测试）。
pub fn page_dir_for(root: &Path, id: &str) -> AppResult<PathBuf> {
    Ok(crate::paths::resolve_in_root(&pages::pages_dir(root), id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_rel_rejects_escapes() {
        assert_eq!(clean_rel("index.html").unwrap(), "index.html");
        assert_eq!(clean_rel("assets/app.js").unwrap(), "assets/app.js");
        assert_eq!(clean_rel("assets\\app.js").unwrap(), "assets/app.js");
        assert_eq!(clean_rel("/assets/app.js").unwrap(), "assets/app.js");
        assert!(clean_rel("../secret.txt").is_err());
        assert!(clean_rel("a/../../secret.txt").is_err());
        assert!(clean_rel("C:/x.txt").is_err());
        assert!(clean_rel("  ").is_err());
    }

    #[test]
    fn sanitize_plus_pages_dir_stays_inside() {
        // 标题里的奇怪字符不会把目录带到别处去
        let id = crate::studio::sanitize_id("../../etc/passwd").unwrap_err();
        let _ = id; // 直接拒绝，符合预期
        let id = crate::studio::sanitize_id("DNA 双螺旋").unwrap();
        let dir = page_dir_for(Path::new("E:/ws/主题"), &id).unwrap();
        assert!(dir.starts_with("E:/ws/主题/.hub/pages"), "{}", dir.display());
    }
}
