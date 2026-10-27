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

/// 服务商对 function/tool 名的硬性规则：`^[a-zA-Z0-9_-]{1,64}$`，违反的请求
/// **整体** 400——不是这一个工具被忽略，而是那一轮对话直接失败。所以名字必须在
/// 出本地这一侧就保证合规：服务器名在入口校验，工具名做「救能救的」清洗。
pub const TOOL_NAME_MAX: usize = 64;

/// 服务器名能不能用作工具名前缀（`mcp__<名>__<工具>`）。
///
/// 只放行 `[a-zA-Z0-9_-]`：中文、点、空格拼出来的工具名服务商一律 400（本应用
/// 是中文优先产品，用户自然会起「翻译」这样的名字，必须在入口处拦下并说清为什么）。
/// 另外不许含 `__`（分隔符，见 [`split_prefixed`]），长度上限 40 是给工具名留余量：
/// 超过的话 `mcp__<名>__` 自己就快占满 64 字符，几乎所有工具都登记不上。
pub fn is_server_name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 40
        && !name.contains("__")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 外部工具名 → 合规的对外名字。清洗不了的（前缀已把 64 字符占满）返回 None，
/// 调用方跳过该工具并记日志，**不能**把不合规的名字放出去。
pub fn exposed_tool_name(server: &str, tool: &str) -> Option<String> {
    let prefix = format!("{PREFIX}{server}__");
    let budget = TOOL_NAME_MAX.saturating_sub(prefix.len());
    if budget == 0 {
        return None;
    }
    // 非法字符（点、空格、非 ASCII…）统一换成 '_'：MCP 规范允许工具名到 128 字符、
    // 不限字符集，带点的 `com.example.x` 很常见，硬拒绝会让一整台服务器全灭。
    let mut cleaned: String = tool
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    cleaned.truncate(budget); // 上面只保留 ASCII，按字符截断不会切出半个多字节
    if cleaned.is_empty() {
        cleaned.push('_'); // 全是非法字符时至少占住位，别让两个工具都变空
    }
    Some(format!("{prefix}{cleaned}"))
}

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
    /// 按服务商的规则清洗对外名字；前缀已把 64 字符占满（服务器名太长）时返回
    /// None——调用方跳过该工具并记日志，不能把会 400 的名字登记进工具表。
    pub fn try_new(client: Arc<McpClient>, info: McpToolInfo) -> Option<Self> {
        let server = client.name.clone();
        let exposed = exposed_tool_name(&server, &info.name)?;
        Some(Self {
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
        })
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

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn server_name_follows_provider_charset() {
        assert!(is_server_name_valid("fetch"));
        assert!(is_server_name_valid("agent-browser"));
        // 中文 / 点 / 空格拼进 mcp__<名>__<工具> 之后服务商整体 400
        assert!(!is_server_name_valid("翻译"));
        assert!(!is_server_name_valid("com.example.x"));
        assert!(!is_server_name_valid("有 空格"));
        // __ 是前缀的分隔符，进名字会让 split_prefixed 切错
        assert!(!is_server_name_valid("a__b"));
        assert!(!is_server_name_valid(""));
        assert!(!is_server_name_valid(&"x".repeat(41)));
        assert!(is_server_name_valid(&"x".repeat(40)));
    }

    #[test]
    fn exposed_name_is_sanitized_and_capped() {
        assert_eq!(
            exposed_tool_name("fetch", "com.example.lookup").unwrap(),
            "mcp__fetch__com_example_lookup"
        );
        // 拼上前后缀后不能超过服务商的 64 字符上限
        let long_tool = "t".repeat(200);
        let exposed = exposed_tool_name("fetch", &long_tool).unwrap();
        assert!(exposed.len() <= TOOL_NAME_MAX);
        // 服务器名占满预算 → 这个工具登记不上（调用方跳过并记日志）
        assert_eq!(exposed_tool_name(&"s".repeat(64), "tool"), None);
        // 全是非法字符也不给空名：4 个中文字 → 4 个下划线
        assert_eq!(exposed_tool_name("fetch", "中文工具").unwrap(), format!("mcp__fetch__{}", "_".repeat(4)));
    }
}
