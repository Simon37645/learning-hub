//! MCP 工具包装：把外部服务器的工具伪装成本地工具。
//!
//! 工具名统一加前缀 `mcp__<服务器>__<工具名>`，好处有三：
//! 1. 与内置工具不会撞名
//! 2. 模型在工具列表里一眼能看出「这是外部的」
//! 3. 出问题时能立刻定位是哪个服务器

use crate::agent::event::Risk;
use crate::agent::registry::{Tool, ToolCtx, ToolOutput};
use crate::error::AppResult;
use crate::mcp::{McpClient, McpToolInfo};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub const PREFIX: &str = "mcp__";

/// 工具名 → 服务器名与原始工具名
pub fn split_prefixed(full: &str) -> Option<(String, String)> {
    let rest = full.strip_prefix(PREFIX)?;
    let (server, tool) = rest.split_once("__")?;
    Some((server.to_string(), tool.to_string()))
}

pub struct McpTool {
    client: Arc<McpClient>,
    /// 带前缀的对外名字
    exposed: String,
    /// 服务器那边的原始名字
    origin: String,
    description: String,
    schema: Value,
    /// Box::leak 过的 'static 名字/描述缓存。Tool::name 要求 'static，但它是
    /// 每轮对话都会调的 trait 方法——不放缓存的话泄漏量跟回合数成正比
    name_cache: std::sync::OnceLock<&'static str>,
    desc_cache: std::sync::OnceLock<&'static str>,
}

impl McpTool {
    pub fn new(client: Arc<McpClient>, info: McpToolInfo) -> Self {
        let server = client.name.clone();
        let exposed = format!("{PREFIX}{server}__{}", info.name);
        Self {
            client,
            exposed,
            origin: info.name,
            description: format!(
                "[来自 MCP 服务器「{server}」] {}",
                if info.description.trim().is_empty() {
                    "（该工具没有提供说明，请谨慎使用）".to_string()
                } else {
                    info.description.trim().to_string()
                }
            ),
            schema: info.input_schema,
            name_cache: std::sync::OnceLock::new(),
            desc_cache: std::sync::OnceLock::new(),
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &'static str {
        // Tool::name 要求 'static：Box::leak 换 'static，但只 leak 一次
        //（它每轮对话都会被调，泄漏量不能跟调用次数走）
        *self.name_cache.get_or_init(|| Box::leak(self.exposed.clone().into_boxed_str()))
    }

    fn description(&self) -> &'static str {
        *self.desc_cache.get_or_init(|| Box::leak(self.description.clone().into_boxed_str()))
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    /// 外部工具的能力不可知，一律当作「写入」级别：
    /// 在「每次确认」模式下会弹窗让用户过目。宁可多问一次，也不要让未知工具静默改东西。
    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, _input: &Value) -> String {
        format!(
            "调用外部工具 {}（MCP：{}）",
            self.origin, self.client.name
        )
    }

    async fn run(&self, _ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        match self.client.call_tool(&self.origin, input).await {
            Ok((text, is_error)) => Ok(if is_error {
                ToolOutput::err(text)
            } else {
                ToolOutput::ok(text)
            }),
            Err(e) => Ok(ToolOutput::err(format!("MCP 调用失败：{e}"))),
        }
    }
}
