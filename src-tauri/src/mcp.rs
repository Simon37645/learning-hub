//! MCP（Model Context Protocol）客户端：把外部 MCP 服务器提供的工具接进 agent。
//!
//! 只实现 stdio 传输（规范里的主推方式，也是本地工具最常用的形态）：
//! 启动子进程 → initialize 握手 → tools/list → 把每个工具登记进工具注册表。
//! 之后模型调用它，我们就转发 `tools/call` 并把结果拿回来。
//!
//! 报文格式：**换行分隔的 JSON-RPC 2.0**（MCP stdio 的约定，不是 SSE 也不是 Content-Length）。
//!
//! 为什么自己写而不引第三方 crate：客户端只用得到四个方法，
//! 引一个 SDK 会把依赖树和版本节奏一起背进来；这里 300 行能写清楚，也好排查。

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

/// 我们支持的协议版本。服务器返回别的版本时按「以它为准、能跑就行」处理。
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// 单次请求超时
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// 握手超时（服务器起不来不该拖住启动）
const INIT_TIMEOUT: Duration = Duration::from_secs(20);

/// 配置里描述的一个 MCP 服务器。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 显示名，同时作为工具名前缀（`mcp__<name>__<tool>`）
    pub name: String,
    /// 可执行文件或命令，例如 `npx`、`python`、`C:\tools\server.exe`
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// 额外环境变量
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// 一个已连接的服务器。
pub struct McpClient {
    pub name: String,
    stdin: AsyncMutex<ChildStdin>,
    pending: Arc<AsyncMutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>,
    next_id: AtomicI64,
    /// 子进程句柄，Drop 时会被 kill（放在 Mutex 里以便显式关闭）
    child: AsyncMutex<Option<Child>>,
    pub tools: Vec<McpToolInfo>,
    /// 服务器自己的名字与版本（握手后填一次），界面上显示用
    server_info: std::sync::OnceLock<String>,
}

impl McpClient {
    /// 启动并完成握手。
    pub async fn connect(cfg: &McpServerConfig) -> AppResult<Arc<McpClient>> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Windows 下别弹控制台窗口
            .kill_on_drop(true);
        if let Some(cwd) = &cfg.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AppError::other(format!(
                "启动 MCP 服务器「{}」失败（{}）：{e}",
                cfg.name, cfg.command
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::other("MCP 子进程没有 stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::other("MCP 子进程没有 stdout"))?;

        // stderr 单独抽干：不读的话管道满了会把服务器卡死，而且日志本身很有用
        if let Some(stderr) = child.stderr.take() {
            let name = cfg.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[mcp:{name}] {line}");
                }
            });
        }

        let pending: Arc<AsyncMutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));

        let client = Arc::new(McpClient {
            name: cfg.name.clone(),
            stdin: AsyncMutex::new(stdin),
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            child: AsyncMutex::new(Some(child)),
            tools: Vec::new(),
            server_info: std::sync::OnceLock::new(),
        });

        // 读循环：把响应派发给等待中的请求
        {
            let pending = pending.clone();
            let name = cfg.name.clone();
            tokio::spawn(async move {
                read_loop(stdout, pending, name).await;
            });
        }

        client
            .initialize()
            .await
            .map_err(|e| AppError::other(format!("MCP 服务器「{}」握手失败：{e}", cfg.name)))?;

        Ok(client)
    }

    async fn initialize(&self) -> AppResult<()> {
        let result = tokio::time::timeout(
            INIT_TIMEOUT,
            self.request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "clientInfo": { "name": "learning-hub", "version": env!("CARGO_PKG_VERSION") },
                }),
            ),
        )
        .await
        .map_err(|_| AppError::other("握手超时"))??;

        // 服务器可以自己报名字与版本
        let server_name = result
            .pointer("/serverInfo/name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let server_version = result
            .pointer("/serverInfo/version")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // 把 server_info 写进去（这里还没 Arc 共享，用 Cell 不行，改成先返回）
        let info = if server_version.is_empty() {
            server_name.to_string()
        } else {
            format!("{server_name} {server_version}")
        };
        let _ = self.server_info.set(info);

        // 按规范，initialize 之后要发这条通知
        self.notify("notifications/initialized", json!({})).await?;
        Ok(())
    }

    /// 拉取工具清单。
    pub async fn list_tools(&self) -> AppResult<Vec<McpToolInfo>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                Some(McpToolInfo {
                    name,
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                })
            })
            .collect())
    }

    /// 调用一个工具，返回拼好的文本结果。
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> AppResult<(String, bool)> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // content 是块数组：text / image / resource
        let mut text = String::new();
        if let Some(blocks) = result.get("content").and_then(|c| c.as_array()) {
            for b in blocks {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                    Some("image") => {
                        let mime = b.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image");
                        text.push_str(&format!("\n[图片结果：{mime}（MCP 返回的图片暂不显示）]"));
                    }
                    Some("resource") => {
                        let uri = b
                            .pointer("/resource/uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or("?");
                        text.push_str(&format!("\n[资源：{uri}]"));
                    }
                    _ => {}
                }
                // 有些服务器把结构化结果放在 structuredContent
                if let Some(sc) = b.get("structuredContent") {
                    if !sc.is_null() {
                        text.push_str(&format!("\n{}", serde_json::to_string_pretty(sc).unwrap_or_default()));
                    }
                }
            }
        }
        if text.trim().is_empty() {
            text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "（MCP 无输出）".into());
        }
        Ok((text, is_error))
    }

    /// 发一个请求并等响应。
    async fn request(&self, method: &str, params: Value) -> AppResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_line(&msg).await?;

        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(AppError::other(e)),
            Ok(Err(_)) => Err(AppError::other("MCP 连接已断开")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(AppError::other(format!("MCP 请求超时：{method}")))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        self.write_line(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn write_line(&self, msg: &Value) -> AppResult<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AppError::other(format!("写入 MCP 失败：{e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| AppError::other(format!("刷新 MCP 失败：{e}")))?;
        Ok(())
    }

    /// 服务器自报的名字与版本。
    pub fn server_info(&self) -> String {
        self.server_info.get().cloned().unwrap_or_else(|| "（未握手）".into())
    }

    /// 关掉子进程（退出应用或禁用该服务器时）。
    pub async fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

async fn read_loop(
    stdout: ChildStdout,
    pending: Arc<AsyncMutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>,
    name: String,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[mcp:{name}] 跳过无法解析的一行：{e} | {line}");
                        continue;
                    }
                };
                // 只有带 id 的才是响应；通知（如 logging）忽略
                let Some(id) = msg.get("id").and_then(|i| i.as_i64()) else {
                    continue;
                };
                let mut map = pending.lock().await;
                if let Some(tx) = map.remove(&id) {
                    let outcome = if let Some(err) = msg.get("error") {
                        Err(format!(
                            "MCP 报错：{}",
                            err.get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("未知错误")
                        ))
                    } else {
                        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = tx.send(outcome);
                }
            }
            Ok(None) => {
                // 进程退出：把所有等待中的请求叫醒，避免一直挂着
                let mut map = pending.lock().await;
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(format!("MCP 服务器「{name}」已退出")));
                }
                break;
            }
            Err(e) => {
                eprintln!("[mcp:{name}] 读取失败：{e}");
                break;
            }
        }
    }
}
