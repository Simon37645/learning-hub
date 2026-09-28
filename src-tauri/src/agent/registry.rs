//! 工具注册表：agent 能做的事都在这里登记。
//!
//! 加一个新能力 = 写一个 `impl Tool` + 在 [`super::tools::registry`] 里注册，
//! 不需要改 agent 主循环。

use crate::agent::event::Risk;
use crate::domain::topic::Topic;
use crate::error::{AppError, AppResult};
use crate::state::AppCore;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 访问模式，决定越权申请时怎么向用户描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Delete,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
            Access::Delete => "delete",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Access::Read => "读取",
            Access::Write => "写入",
            Access::Delete => "删除",
        }
    }
}

/// 暴露给模型的工具声明（会转成各家 function calling 的格式）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// 一次工具调用的执行环境。
pub struct ToolCtx {
    pub core: Arc<AppCore>,
    /// 当前主题（没有主题时的「布置」类工具会拒绝执行）
    pub topic: Option<Topic>,
    pub turn_id: String,
    pub chat_id: String,
}

impl ToolCtx {
    pub fn topic(&self) -> AppResult<&Topic> {
        self.topic
            .as_ref()
            .ok_or_else(|| AppError::invalid("当前没有打开主题，请先新建或选择一个主题"))
    }

    /// 工具需要落到某个主题时用它拿到「目录 + 元数据」。
    pub fn topic_or(&self, slug_or_name: Option<&str>) -> AppResult<Topic> {
        match slug_or_name {
            Some(s) if !s.trim().is_empty() => self.core.config_read().workspace().resolve(s),
            _ => self.topic().cloned(),
        }
    }

    /// 解析工具给的路径。三种写法：
    ///
    /// - `notes/a.md`（相对）→ 当前主题（或 `topic` 参数指定的主题）内
    /// - `D:\课件\x.pdf` / `~/Downloads/x.pdf`（绝对）→ 受沙箱管辖，
    ///   在工作区之外且未获授权时，会向用户发起越权申请
    /// - 相对路径里的 `..` 一律拒绝（要跨主题请用 `topic` 参数）
    pub async fn resolve_path(
        &self,
        raw: &str,
        topic: Option<&str>,
        access: Access,
        reason: &str,
    ) -> AppResult<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(AppError::invalid("路径为空"));
        }

        if crate::paths::looks_absolute(raw) || raw.starts_with('~') {
            let expanded = crate::paths::expand_home(raw);
            let path = crate::paths::absolutize(Path::new(&expanded))?;
            if self.core.needs_escalation(&path) {
                let granted = self
                    .core
                    .agent
                    .request_sandbox(&self.core, &self.turn_id, &path, access.label(), reason)
                    .await;
                if !granted {
                    return Err(AppError::Denied(format!(
                        "用户没有批准访问工作区之外的 {}。\
                         如果只是要一份资料，更稳妥的做法是请用户把它放进主题的 materials/ 目录。",
                        path.display()
                    )));
                }
            }
            return Ok(path);
        }

        let topic = self.topic_or(topic)?;
        crate::paths::resolve_in_root(&topic.dir, raw)
    }

    /// 给模型看的路径标签：主题内用相对路径，主题外用绝对路径。
    pub fn label(&self, path: &Path) -> String {
        if let Some(t) = &self.topic {
            if crate::paths::is_within(&t.dir, path) {
                return t.rel(path);
            }
        }
        let ws = self.core.workspace();
        if crate::paths::is_within(&ws.root, path) {
            // 工作区内但不在当前主题：标成 `主题名/相对路径`，模型看得懂
            return path
                .strip_prefix(&ws.root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
        }
        path.to_string_lossy().to_string()
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }

    /// 结构化结果：序列化成 JSON 交给模型（模型对 JSON 的解析最稳）。
    pub fn json<T: Serialize>(value: &T) -> Self {
        match serde_json::to_string_pretty(value) {
            Ok(s) => Self::ok(s),
            Err(e) => Self::err(format!("结果序列化失败：{e}")),
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名，模型看到的就是它（用英文蛇形，跨服务商最稳）。
    fn name(&self) -> &'static str;
    /// 给模型看的说明。写清楚「什么时候用」「参数是什么」「返回什么」。
    fn description(&self) -> &'static str;
    /// JSON Schema 形式的入参。
    fn schema(&self) -> Value;
    /// 风险等级，决定要不要用户确认。
    fn risk(&self) -> Risk {
        Risk::Read
    }
    /// 人类可读的调用摘要，显示在确认弹窗和工具卡片上。
    fn summarize(&self, input: &Value) -> String;
    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput>;
}

#[derive(Default)]
pub struct ToolRegistry {
    order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        let name = tool.name().to_string();
        if !self.tools.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.tools.insert(name, tool);
        self
    }

    /// 按前缀移除工具（MCP 重连时用）。
    pub fn unregister_prefix(&mut self, prefix: &str) -> usize {
        let victims: Vec<String> = self.order.iter().filter(|n| n.starts_with(prefix)).cloned().collect();
        for name in &victims {
            self.tools.remove(name);
        }
        self.order.retain(|n| !n.starts_with(prefix));
        victims.len()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.schema(),
            })
            .collect()
    }

    /// 生成给系统提示词用的工具清单（模型不支持 function calling 时的降级说明）。
    pub fn describe_for_prompt(&self) -> String {
        self.order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .map(|t| format!("- {}｜{}｜参数：{}", t.name(), t.description(), compact_schema(&t.schema())))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn compact_schema(schema: &Value) -> String {
    let props = schema.get("properties").and_then(|p| p.as_object());
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let Some(props) = props else {
        return "{}".into();
    };
    let parts: Vec<String> = props
        .iter()
        .map(|(k, v)| {
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("any");
            let star = if required.contains(&k.as_str()) { "必填" } else { "可选" };
            format!("{k}:{ty}({star})")
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// 常用 schema 片段，减少各工具里的重复。
pub fn object_schema(props: serde_json::Value, required: &[&str]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

pub fn str_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "string", "description": desc })
}

pub fn bool_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "boolean", "description": desc })
}

pub fn num_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "integer", "description": desc })
}

pub fn str_array_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

/// 取字符串参数，兼容模型偶尔给出的非字符串形态。
pub fn arg_str(input: &Value, key: &str) -> Option<String> {
    match input.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

pub fn arg_str_req(input: &Value, key: &str) -> AppResult<String> {
    arg_str(input, key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::invalid(format!("缺少参数 {key}")))
}

pub fn arg_u32(input: &Value, key: &str) -> Option<u32> {
    input.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_u64().map(|x| x as u32),
        Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    })
}

pub fn arg_f32(input: &Value, key: &str) -> Option<f32> {
    input.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64().map(|x| x as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    })
}

pub fn arg_bool(input: &Value, key: &str) -> Option<bool> {
    match input.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "是" => Some(true),
            "false" | "0" | "no" | "否" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn arg_str_array(input: &Value, key: &str) -> Vec<String> {
    match input.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split([',', '，', '\n'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}
