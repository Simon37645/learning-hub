//! 演示页：agent 写出来的**本地可交互网页**（动画、3D、可拖参数），在内置浏览器里当真网页跑。
//!
//! 为什么单独开一条通路：普通本地 HTML 走的是 `sandbox=""` 的 srcdoc iframe——
//! 不执行脚本、也读不到同目录的兄弟文件（srcdoc 没有 URL 基准）。而「机制演示」这类页面
//! 恰恰要 WebGL、要 `import`、要 `fetch('./data.json')`。这里补上缺的那一半：
//!
//! 1. 注册 `lhpage://` 协议（WebView2 上表现为 `http://lhpage.localhost/…`），
//!    让页面有一个**真 origin**，能加载 ES 模块与同目录资源，也能离线 import 内置的 three.js；
//! 2. 用「通行证（token）→ 目录」的映射把可读范围钉死在**这一个页面自己的目录**里：
//!    模型写的页面拿不到用户工作区里的其它文件（`..`、绝对路径、盘符一律拒绝），
//!    想读别的文件只能由 agent 通过工具去读——那一层有沙箱与审批兜着。
//!
//! 安全姿态与远端网页一致：页面最终在**原生子 WebView**里打开（顶层浏览上下文），
//! label 是 `web-<tab_id>`、不在 capabilities 的 windows 列表里，因此拿不到任何 IPC 权限；
//! `on_navigation` 还会再挡一层。即便页面里写满了恶意脚本，它能做的也只是画点东西、
//! 读自己目录里的文件、访问以上网（和打开一个普通网页一样）。
//!
//! 通行证**只活在内存里**：应用重启后旧 token 失效，标签会在恢复时重新发布一个新的
//! （前端拿到的是新快照，用户无感）。

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::http::{header, Method, Response, StatusCode};

/// 自定义协议名。WebView2 不认识非标准 scheme，Tauri 会把它映射成
/// `http://<scheme>.localhost/…`（见 [`ORIGIN`]）。
pub const SCHEME: &str = "lhpage";
/// 页面在 WebView2 里的 origin 前缀。
const ORIGIN: &str = "http://lhpage.localhost";
/// 内置资产的固定前缀（页面里的 `/_vendor/three.js`）。
const VENDOR_PREFIX: &str = "_vendor/";

/// 随应用内置的第三方库：演示页离线可用，不依赖 CDN。
/// 只预置 three.js（含 OrbitControls），理由与更新方式见
/// `src-tauri/assets/vendor/README.md`——别把这里堆成 CDN。
const VENDOR: &[(&str, &str, &str)] = &[(
    "three.js",
    "text/javascript; charset=utf-8",
    include_str!("../../assets/vendor/three.bundle.min.js"),
)];

/// 演示页允许加载什么。
///
/// 放宽的那部分：内联脚本、内联样式、https 上的库（`<script src="https://cdn…">` 与
/// `import "https://…"` 都走 script-src）——这就是它的用途，收紧就没法做演示了。
///
/// **刻意收紧的那一处是 `connect-src`**：页面不能 `fetch`/XHR/WebSocket 到外网。
/// 因为「本地 .html 都会被当演示页跑起来」意味着用户**从别处下载来的**页面也会跑脚本——
/// 那种页面只要和讲义放在同一个目录里，就能把 `./讲义.pdf` 读走再发出去。
/// 只读自己目录 + 不许往外发，这条路的破坏力就和「打开一个静态网页」差不多。
/// （要联网取数据的演示页做不到：让 agent 先把数据抓下来写进页面目录。）
const PAGE_CSP: &str = "default-src 'self' data: blob: https:; \
     script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' https:; \
     style-src 'self' 'unsafe-inline' https:; \
     img-src 'self' data: blob: https:; \
     font-src 'self' data:; \
     connect-src 'self' data: blob:; \
     media-src 'self' data: blob: https:; \
     worker-src 'self' blob:";

// ---------------------------------------------------------------- 通行证

#[derive(Default)]
struct Inner {
    /// 目录 → token（同一个目录复用同一个 token，反复打开同一个页面不会堆垃圾）
    by_dir: HashMap<PathBuf, String>,
    /// token → 目录（协议处理器只认这张表）
    by_token: HashMap<String, PathBuf>,
}

/// 「哪个目录可以被哪个 token 读到」。进程级单例——协议处理器是自由函数，
/// 拿不到 `AppState`，而这份映射只跟 URL 有关，放全局最省事。
#[derive(Default)]
pub struct PageStore {
    inner: Mutex<Inner>,
}

static STORE: OnceLock<PageStore> = OnceLock::new();

pub fn pages() -> &'static PageStore {
    STORE.get_or_init(PageStore::default)
}

impl PageStore {
    /// 给一个目录发通行证（同一目录复用）。目录不存在时也会发出——
    /// 读的时候再报 404，调用方不用先判断。
    pub fn publish(&self, dir: &Path) -> String {
        let mut g = self.inner.lock();
        if let Some(t) = g.by_dir.get(dir) {
            return t.clone();
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        g.by_dir.insert(dir.to_path_buf(), token.clone());
        g.by_token.insert(token.clone(), dir.to_path_buf());
        token
    }

    /// token + 相对路径 → 目录内的真实文件；越界/编码非法一律 `None`。
    pub fn resolve(&self, token: &str, rel: &str) -> Option<PathBuf> {
        let dir = self.inner.lock().by_token.get(token)?.clone();
        safe_join(&dir, rel)
    }

    /// 页面文件 → 可以交给 WebView 的 URL。
    ///
    /// 通行证发给**文件所在的目录**：页面因此能读到自己目录里的兄弟文件
    /// （`./chart.js`、`./data.json`），读不到目录之外的任何东西。
    pub fn url_for_file(&self, file: &Path) -> Option<String> {
        let dir = file.parent()?;
        let name = file.file_name()?.to_str()?;
        let token = self.publish(dir);
        Some(format!("{ORIGIN}/{token}/{}", encode_path_segment(name)))
    }
}

/// 这个文件是不是「可以当网页跑」的类型。只有 HTML 才给 URL——
/// 其它扩展名交给原来的渲染器（图片/PDF/Markdown 各有各的预览）。
pub fn is_page_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
            .as_str(),
        "html" | "htm"
    )
}

/// 给一次「打开本地文件」的请求补上演示页 URL（只有 HTML 才有）。
///
/// 两个入口（前端命令 `viewer_open` 与 agent 的 `viewer_open` 工具）都走这里，
/// 别各写一份——漏了一处的症状是「同一个页面，从对话里打开能跑脚本、从资料面板打开不能」。
pub fn attach_page_url(req: &mut super::OpenRequest, abs: &Path) {
    if is_page_file(abs) {
        if let Some(url) = pages().url_for_file(abs) {
            req.url = Some(url);
        }
    }
}

/// URL 是不是我们发的通行证 URL（区别于远端 http 页面）。
pub fn is_page_url(url: &str) -> bool {
    url.starts_with(ORIGIN) || url.starts_with(&format!("{SCHEME}://"))
}

/// 这一轮对话的演示页目录：有主题就放到主题的 `.hub/pages/` 下，
/// 没有主题（首页/日常问答）就放到工作区的 `.hub/pages/`。
pub fn pages_dir(root: &Path) -> PathBuf {
    root.join(".hub").join("pages")
}

// ---------------------------------------------------------------- 协议处理

/// 处理一次 `lhpage://` 请求。注册在 `lib.rs` 的 Builder 上（异步变体：允许阻塞读文件）。
pub fn handle(request: tauri::http::Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    responder.respond(route(&method, &path));
}

/// 路由与响应（抽成纯函数：单测不需要 Tauri）。
pub fn route(method: &Method, path: &str) -> Response<Vec<u8>> {
    if method != Method::GET && method != Method::HEAD {
        return reply(StatusCode::METHOD_NOT_ALLOWED, "text/plain; charset=utf-8", b"only GET".to_vec(), None);
    }
    let raw = path.trim_start_matches('/');
    // 内置资产：/_vendor/three.js
    if let Some(name) = raw.strip_prefix(VENDOR_PREFIX) {
        return match VENDOR.iter().find(|(n, _, _)| *n == name) {
            Some((_, mime, body)) => reply(StatusCode::OK, mime, body.as_bytes().to_vec(), None),
            None => not_found(&format!("没有这个内置资产：{name}（只预置了 three.js）")),
        };
    }
    // 通行证：/<token>/<相对路径>
    let Some((token, rel)) = raw.split_once('/') else {
        return not_found("演示页地址形如 /<通行证>/index.html");
    };
    let Some(file) = pages().resolve(token, rel) else {
        return not_found("通行证无效，或路径越出了这个页面的目录（请让 agent 重新打开这个演示页）");
    };
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mime = mime_of(&file);
            let csp = if mime.starts_with("text/html") { Some(PAGE_CSP) } else { None };
            reply(StatusCode::OK, mime, bytes, csp)
        }
        Err(e) => not_found(&format!("读不到文件：{}（{e}）", file.display())),
    }
}

fn reply(
    status: StatusCode,
    mime: &str,
    body: Vec<u8>,
    csp: Option<&str>,
) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        // 页面是 sandbox 之外的真 origin，同目录取资源走的是 CORS——不过这一层
        // 只服务「自己有通行证」的东西，放开无妨。
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        // 不缓存：agent 改完页面要让 `viewer_reload` 立刻看到新版本
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(csp) = csp {
        b = b.header(header::CONTENT_SECURITY_POLICY, csp);
    }
    b.body(body).unwrap_or_else(|_| Response::new(Vec::new()))
}

fn not_found(msg: &str) -> Response<Vec<u8>> {
    reply(
        StatusCode::NOT_FOUND,
        "text/plain; charset=utf-8",
        msg.as_bytes().to_vec(),
        None,
    )
}

/// 按扩展名给 MIME。模块脚本必须是 JS 的 MIME（浏览器对 `<script type=module>`
/// 和 `import` 做严格 MIME 检查，给错就是一整页白屏）。
pub fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "bin" => "application/octet-stream",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------- 路径

/// 把 URL 里的一段路径还原成原文件名（只处理文件名这一级）。
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `%XX` 解码（URL 路径用）。非法编码返回 `None`——宁可 404，也不要猜。
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `dir` + URL 相对路径 → 真实文件。**只允许老老实实待在这个目录里**：
/// `..`、绝对路径、盘符、UNC、Windows 数据流（`x:y`）一律拒绝。
///
/// 为什么不用 `resolve_in_root`：那个是给「工作区里的相对路径」用的，
/// 面向的是 agent 的调用；这里是**来自页面的 URL**（可能带 `%2e%2e`），
/// 判断条件更严也更简单：没有任何例外。
fn safe_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    let decoded = percent_decode(rel)?;
    if decoded.contains('\\') {
        return None;
    }
    let mut out = dir.to_path_buf();
    let mut any = false;
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains(':') || seg.contains('\0') {
            return None;
        }
        out.push(seg);
        any = true;
    }
    if !any {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_spaces_and_rejects_junk() {
        assert_eq!(percent_decode("a%20b.js").unwrap(), "a b.js");
        assert_eq!(percent_decode("中文.js").unwrap(), "中文.js");
        assert!(percent_decode("%zz").is_none());
        assert!(percent_decode("abc%2").is_none());
    }

    #[test]
    fn safe_join_stays_inside_the_directory() {
        let dir = Path::new("E:/学习中枢/workspace/线性代数/.hub/pages/demo");
        assert_eq!(
            safe_join(dir, "index.html").unwrap(),
            dir.join("index.html")
        );
        assert_eq!(safe_join(dir, "sub/a.js").unwrap(), dir.join("sub").join("a.js"));
        // 编码过的 .. 也不行
        assert!(safe_join(dir, "%2e%2e/secret.txt").is_none());
        assert!(safe_join(dir, "../secret.txt").is_none());
        assert!(safe_join(dir, "a/../../secret.txt").is_none());
        assert!(safe_join(dir, "C:/Windows/win.ini").is_none());
        assert!(safe_join(dir, "a\\b.txt").is_none());
        assert!(safe_join(dir, "sub/file.txt:ads").is_none());
        assert!(safe_join(dir, "").is_none());
        assert!(safe_join(dir, "/").is_none());
    }

    #[test]
    fn publish_is_stable_per_directory_and_scopes_reads() {
        let store = PageStore::default();
        let a = PathBuf::from("E:/ws/主题/.hub/pages/one");
        let b = PathBuf::from("E:/ws/主题/.hub/pages/two");
        let ta = store.publish(&a);
        assert_eq!(ta, store.publish(&a), "同一个目录要复用同一个通行证");
        let tb = store.publish(&b);
        assert_ne!(ta, tb);

        assert_eq!(store.resolve(&ta, "index.html").unwrap(), a.join("index.html"));
        // 另一个页面的通行证读不到这个目录
        assert!(store.resolve(&tb, "index.html").unwrap().starts_with(&b));
        // 未知通行证
        assert!(store.resolve("nope", "index.html").is_none());
        // 越界
        assert!(store.resolve(&ta, "../two/index.html").is_none());
    }

    #[test]
    fn url_for_file_points_at_its_own_directory() {
        let store = PageStore::default();
        let file = PathBuf::from("E:/ws/主题/.hub/pages/演示 页/index.html");
        let url = store.url_for_file(&file).unwrap();
        assert!(url.starts_with(ORIGIN), "{url}");
        assert!(url.ends_with("/index.html"), "{url}");
        // 文件名里的空格被编码（URL 里不能出现裸空格）
        assert!(!url.contains(' '), "{url}");
        // 同一个文件两次发布拿到同一个 URL
        assert_eq!(url, store.url_for_file(&file).unwrap());
    }

    #[test]
    fn route_serves_vendor_and_404s_the_rest() {
        let head = route(&Method::GET, "/_vendor/three.js");
        assert_eq!(head.status(), StatusCode::OK);
        let mime = head.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(mime.starts_with("text/javascript"), "模块脚本的 MIME 必须是 JS：{mime}");
        assert!(head.body().len() > 100_000, "three.js 应该是完整那份（{} 字节）", head.body().len());
        assert!(head.headers().get(header::CACHE_CONTROL).is_some());

        assert_eq!(route(&Method::GET, "/_vendor/nope.js").status(), StatusCode::NOT_FOUND);
        assert_eq!(route(&Method::GET, "/").status(), StatusCode::NOT_FOUND);
        assert_eq!(route(&Method::GET, "/badtoken/x.html").status(), StatusCode::NOT_FOUND);
        assert_eq!(route(&Method::POST, "/_vendor/three.js").status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn route_serves_a_page_with_csp_and_blocks_escape() {
        let dir = std::env::temp_dir().join(format!("lh-pages-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><h1>hi</h1>").unwrap();
        std::fs::write(dir.join("app.js"), "export const a = 1;").unwrap();
        // 目录外面放一份「用户的资料」：页面绝不能读到
        std::fs::write(dir.parent().unwrap().join("secret.txt"), "我不想被页面读到").unwrap();

        let token = pages().publish(&dir);
        let html = route(&Method::GET, &format!("/{token}/index.html"));
        assert_eq!(html.status(), StatusCode::OK);
        assert!(html.headers().get(header::CONTENT_SECURITY_POLICY).is_some(), "HTML 要带 CSP");
        assert!(String::from_utf8_lossy(html.body()).contains("hi"));

        let js = route(&Method::GET, &format!("/{token}/app.js"));
        let mime = js.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(mime.starts_with("text/javascript"), "{mime}");

        assert_eq!(route(&Method::GET, &format!("/{token}/../secret.txt")).status(), StatusCode::NOT_FOUND);
        assert_eq!(route(&Method::GET, &format!("/{token}/%2e%2e/secret.txt")).status(), StatusCode::NOT_FOUND);
        assert_eq!(route(&Method::GET, &format!("/{token}/missing.png")).status(), StatusCode::NOT_FOUND);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(dir.parent().unwrap().join("secret.txt")).ok();
    }

    #[test]
    fn page_urls_are_recognized() {
        assert!(is_page_url("http://lhpage.localhost/abc/index.html"));
        assert!(is_page_url("lhpage://localhost/abc/index.html"));
        assert!(!is_page_url("https://example.com/a.html"));
        assert!(!is_page_url("http://tauri.localhost/index.html"));
    }

    #[test]
    fn only_html_is_a_page() {
        assert!(is_page_file(Path::new("a/b/index.html")));
        assert!(is_page_file(Path::new("a/b/INDEX.HTM")));
        assert!(!is_page_file(Path::new("a/b/notes.md")));
        assert!(!is_page_file(Path::new("a/b/app.js")));
    }
}
