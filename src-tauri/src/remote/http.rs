//! 手写的迷你 HTTP/1.1 服务器。
//!
//! 只实现这个功能真正需要的那一小块：请求行 + 头 + `Content-Length` 正文、
//! 定长响应、以及 SSE 用的分块流。**不做**分块请求体、keep-alive 复用、
//! 压缩、multipart —— 客户端是我们自己发的那个页面，范围写死在注释里比
//! 引一个框架更好交代（和 MCP 客户端同样的取舍）。
//!
//! 为什么不用现成的框架：`hyper`/`axum` 会带进来几十个 crate，而这里需要的
//! 只是「读一个请求 → 写一个响应」，外加一条长连接推送。反过来说，
//! **协议解析必须自己扛住脏输入**（半截请求、超长头、假的 Content-Length），
//! 所以下面每处上限都有单测。
//!
//! 连接模型：一个请求一条连接，响应写完就关（`Connection: close`）。
//! 手机上那几个请求的握手开销可以忽略，换来的是「不用管流水线与粘包」。

use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// 请求头（含请求行）的字节上限。正常浏览器 < 2KB，32KB 已经很宽容。
pub const MAX_HEADER_BYTES: usize = 32 * 1024;
/// 请求体上限。贴一张 1568px 的截图 base64 之后约 2-5MB，留一倍余量。
pub const MAX_BODY_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// 已百分号解码的路径（不含查询串）
    pub path: String,
    pub query: HashMap<String, String>,
    /// 头名统一小写
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }

    pub fn query_get(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(|s| s.as_str())
    }

    /// 解析 JSON 正文（正文不是 JSON 时给一句人话错误）。
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> AppResult<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| AppError::invalid(format!("请求内容不是合法的 JSON：{e}")))
    }

    /// 访问令牌：`?t=` / `Authorization: Bearer` / `X-Remote-Token` 三选一。
    ///
    /// 为什么三种都收：`EventSource` 不能自定义请求头（只能走查询串），
    /// 而 `fetch` 用头更干净（不会留在日志里）。
    pub fn token(&self) -> Option<&str> {
        if let Some(t) = self.query_get("t").filter(|s| !s.is_empty()) {
            return Some(t);
        }
        if let Some(h) = self.header("authorization") {
            if let Some(rest) = h.strip_prefix("Bearer ") {
                return Some(rest.trim());
            }
        }
        self.header("x-remote-token").map(|s| s.trim()).filter(|s| !s.is_empty())
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
}

impl Response {
    pub fn new(status: u16, content_type: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            body: body.into(),
            extra_headers: Vec::new(),
        }
    }

    pub fn json<T: serde::Serialize>(value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self::new(200, "application/json; charset=utf-8", body),
            // 序列化自己的结构体失败只可能是代码问题，不该把连接挂在那里
            Err(e) => Self::error(500, format!("服务端序列化失败：{e}")),
        }
    }

    /// 给远程页面看的错误：正文是 JSON（页面统一按 JSON 处理）。
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        let message = message.into();
        let body = serde_json::json!({ "error": message, "status": status });
        Self::new(status, "application/json; charset=utf-8", serde_json::to_vec(&body).unwrap_or_default())
    }

    pub fn text(body: impl Into<String>) -> Self {
        Self::new(200, "text/plain; charset=utf-8", body.into())
    }

    pub fn html(body: &'static str) -> Self {
        Self::new(200, "text/html; charset=utf-8", body.as_bytes().to_vec())
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }
}

/// 请求还没读完就失败时的错误（带一个该回给客户端的状态码）。
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
}

impl From<std::io::Error> for HttpError {
    fn from(e: std::io::Error) -> Self {
        HttpError::new(400, format!("读取请求失败：{e}"))
    }
}

pub fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

/// 读一个完整请求。头按行读（上限 `MAX_HEADER_BYTES`），正文按 `Content-Length` 读。
pub async fn read_request(stream: &mut BufReader<&mut TcpStream>) -> Result<Request, HttpError> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = stream.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Err(HttpError::new(400, "连接在请求头读完之前断开了"));
        }
        if head.len() + line.len() > MAX_HEADER_BYTES {
            return Err(HttpError::new(431, "请求头太大"));
        }
        let end = line == b"\r\n" || line == b"\n";
        head.extend_from_slice(&line);
        if end {
            break;
        }
    }

    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().trim_end();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_ascii_uppercase();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(HttpError::new(400, "请求行不合法"));
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target.as_str(), ""),
    };
    let path = percent_decode(raw_path);
    if !path.starts_with('/') {
        return Err(HttpError::new(400, "只接受绝对路径"));
    }
    let query = parse_query(raw_query);

    let mut headers = HashMap::new();
    for line in lines {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let len = match headers.get("content-length") {
        Some(v) => v.trim().parse::<usize>().map_err(|_| HttpError::new(400, "Content-Length 不合法"))?,
        None => 0,
    };
    if len > MAX_BODY_BYTES {
        return Err(HttpError::new(413, "请求体太大（图片请先压缩到长边 1568px 再发）"));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut body).await.map_err(|e| HttpError::new(400, format!("请求体读不完整：{e}")))?;
    }

    Ok(Request { method, path, query, headers, body })
}

fn percent_decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw).decode_utf8_lossy().to_string()
}

/// 查询串 → map。同名参数只保留第一个（我们自己的页面不会发重复参数）。
fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let k = percent_decode(&k.replace('+', " "));
        if k.is_empty() || out.contains_key(&k) {
            continue;
        }
        out.insert(k, percent_decode(&v.replace('+', " ")));
    }
    out
}

/// 写一个定长响应，然后由调用方关闭连接。
pub async fn write_response(stream: &mut TcpStream, res: &Response) -> std::io::Result<()> {
    let mut head = String::with_capacity(256);
    head.push_str(&format!("HTTP/1.1 {} {}\r\n", res.status, status_text(res.status)));
    head.push_str(&format!("Content-Type: {}\r\n", res.content_type));
    head.push_str(&format!("Content-Length: {}\r\n", res.body.len()));
    head.push_str("Connection: close\r\n");
    head.push_str("X-Content-Type-Options: nosniff\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\n");
    // 接口与页面都不缓存：手机浏览器对 HTML 的缓存很激进，
    // 版本升级后旧页面配新接口最容易出现「看起来没生效」。
    // 图片这类不可变内容会自己带一条 Cache-Control，别发两条打架。
    if !res.extra_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cache-control")) {
        head.push_str("Cache-Control: no-store\r\n");
    }
    for (k, v) in &res.extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if !res.body.is_empty() {
        stream.write_all(&res.body).await?;
    }
    stream.flush().await?;
    Ok(())
}

/// 开始一条 SSE 流：先把响应头写出去（此后不能再改状态码）。
pub async fn start_sse(stream: &mut TcpStream) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream; charset=utf-8\r\n\
                Connection: close\r\n\
                Cache-Control: no-store\r\n\
                X-Content-Type-Options: nosniff\r\n\
                X-Accel-Buffering: no\r\n\
                \r\n";
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// 发一帧 SSE。用 `Transfer-Encoding: chunked` 分块——不定长流的正经写法，
/// 中间经过 nginx 之类的反代也不会被当成「没读完的响应」。
pub async fn write_sse_event(stream: &mut TcpStream, payload: &str) -> std::io::Result<()> {
    // JSON 里不会有裸换行（serde 会转义），所以一帧一个 data: 行就够
    let frame = format!("data: {payload}\n\n");
    write_chunk(stream, frame.as_bytes()).await
}

/// SSE 心跳：注释行，浏览器会忽略，但能让中间的 NAT / 代理知道连接还活着。
pub async fn write_sse_ping(stream: &mut TcpStream) -> std::io::Result<()> {
    write_chunk(stream, b": ping\n\n").await
}

async fn write_chunk(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let head = format!("{:x}\r\n", data.len());
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(data).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await
}

/// 常量时间比较，别让「令牌对不对」从响应时间上被问出来。
pub fn token_matches(expected: &str, got: &str) -> bool {
    let a = expected.as_bytes();
    let b = got.as_bytes();
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// 起一个只回显请求的服务器，把原始字节写进去，看解析结果。
    async fn parse(raw: &[u8]) -> Result<Request, HttpError> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let raw = raw.to_vec();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = sock.write_all(&raw).await;
            let _ = sock.shutdown().await;
            // 留一点时间让对端读完
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        let mut sock = TcpStream::connect(addr).await.unwrap();
        let mut reader = BufReader::new(&mut sock);
        read_request(&mut reader).await
    }

    #[tokio::test]
    async fn parses_get_with_query_and_headers() {
        let req = parse(
            b"GET /api/invoke?cmd=topic_list&topic=%E7%BA%BF%E6%80%A7%E4%BB%A3%E6%95%B0 HTTP/1.1\r\nHost: x\r\nX-Remote-Token: tok\r\n\r\n",
        )
        .await
        .unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/invoke");
        assert_eq!(req.query_get("cmd"), Some("topic_list"));
        // 中文主题名（目录名就是中文）要能原样解出来
        assert_eq!(req.query_get("topic"), Some("线性代数"));
        assert_eq!(req.token(), Some("tok"));
    }

    #[tokio::test]
    async fn parses_body_by_content_length() {
        let req = parse(b"POST /api/invoke HTTP/1.1\r\nContent-Length: 13\r\n\r\n{\"text\":\"hi\"}").await.unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.body, b"{\"text\":\"hi\"}");
        let v: serde_json::Value = req.json().unwrap();
        assert_eq!(v["text"], "hi");
    }

    #[tokio::test]
    async fn token_from_bearer_and_query() {
        let req = parse(b"GET /api/ping HTTP/1.1\r\nAuthorization: Bearer abc123\r\n\r\n").await.unwrap();
        assert_eq!(req.token(), Some("abc123"));
        let req = parse(b"GET /api/ping?t=q1 HTTP/1.1\r\n\r\n").await.unwrap();
        assert_eq!(req.token(), Some("q1"));
    }

    #[tokio::test]
    async fn rejects_bad_request_line() {
        let err = parse(b"GARBAGE\r\n\r\n").await.unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[tokio::test]
    async fn rejects_oversized_header() {
        let mut raw = b"GET / HTTP/1.1\r\n".to_vec();
        raw.extend_from_slice(format!("X-Big: {}\r\n", "a".repeat(MAX_HEADER_BYTES)).as_bytes());
        raw.extend_from_slice(b"\r\n");
        let err = parse(&raw).await.unwrap_err();
        assert_eq!(err.status, 431);
    }

    #[tokio::test]
    async fn rejects_huge_declared_body() {
        let raw = format!("POST /api/invoke HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        let err = parse(raw.as_bytes()).await.unwrap_err();
        assert_eq!(err.status, 413);
    }

    #[test]
    fn query_parsing_edges() {
        let q = parse_query("a=1&b=%2Fpath%2F&c&d=1&d=2&empty=");
        assert_eq!(q.get("a").unwrap(), "1");
        assert_eq!(q.get("b").unwrap(), "/path/");
        assert_eq!(q.get("c").unwrap(), "");
        // 重复参数取第一个，后面的不覆盖
        assert_eq!(q.get("d").unwrap(), "1");
        assert_eq!(q.get("empty").unwrap(), "");
    }

    #[test]
    fn token_compare_is_exact() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
        assert!(!token_matches("", ""));
    }
}
