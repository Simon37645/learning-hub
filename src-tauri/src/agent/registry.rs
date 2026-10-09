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

/// agent 当前在哪种模式里干活。
///
/// - `Study`：学习模式。有主题（或首页的日常问答），相对路径以**主题目录**为根。
/// - `Studio`：工坊模式。独立于学习，用来造技能与 MCP 服务器；
///   相对路径以**工坊目录**（`<工作区>/.hub/workshop/`）为根。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Study,
    Studio,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Study => "study",
            AgentMode::Studio => "studio",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentMode::Study => "学习",
            AgentMode::Studio => "工坊",
        }
    }

    /// 认不出来的值一律当学习模式——老对话里没有这个字段，默认就是它。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "studio" | "工坊" | "workshop" => AgentMode::Studio,
            _ => AgentMode::Study,
        }
    }
}

/// 工具在哪个模式下出现。
///
/// 默认（不实现 `scope`）是**只给学习模式**：新加一个工具时不必记得来登记，
/// 它至少不会莫名其妙出现在工坊里、然后因为「没有主题」而报错。
/// 能在工坊里也成立的工具（读写文件、技能、MCP 状态、联网、记忆）显式声明 `Both`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolScope {
    Study,
    Studio,
    Both,
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
    /// 相对路径的根。学习模式 = 当前主题目录；工坊模式 = 工坊目录。
    /// 工具不该自己去拼这个根，一律走 [`ToolCtx::resolve_path`]。
    pub root: PathBuf,
    /// 当前模式，决定 `topic` 缺省时算不算错误。
    pub mode: AgentMode,
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

    /// 「不写路径时默认从哪儿开始」：学习模式＝当前主题目录（可用 `topic` 参数指别的主题），
    /// 工坊模式＝工坊目录。工具不许自己拼这个根。
    pub fn root_for(&self, topic: Option<&str>) -> AppResult<PathBuf> {
        if self.mode == AgentMode::Studio {
            return Ok(self.root.clone());
        }
        Ok(self.topic_or(topic)?.dir.clone())
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

        // 工坊模式：相对路径以工坊目录为根。那里没有主题，
        // 也不该拿主题的名义落盘——`topic_or` 在这种情况下会直接报错。
        if self.mode == AgentMode::Studio {
            return crate::paths::resolve_in_root(&self.root, raw);
        }

        let topic = self.topic_or(topic)?;
        crate::paths::resolve_in_root(&topic.dir, raw)
    }

    /// 给模型看的路径标签：主题内用相对路径，主题外用绝对路径。
    pub fn label(&self, path: &Path) -> String {
        if self.mode == AgentMode::Studio && crate::paths::is_within(&self.root, path) {
            // 工坊里的文件不属于任何主题：标明它相对工坊根的位置，
            // 模型才知道自己写的东西在哪儿（提示词里也是这么称呼它的）
            return format!("workshop/{}", crate::paths::rel_in_root(&self.root, path));
        }
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
    /// 结果里附带的图片（PDF 页截图这类「模型要亲眼看一眼」的东西）。
    /// 会跟着工具结果一起落盘，并按协议编码成图片块发给模型。
    pub images: Vec<crate::agent::message::ToolImage>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, images: Vec::new() }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, images: Vec::new() }
    }

    /// 带图片的成功结果。`content` 仍然要写清楚「这几张图是什么」——
    /// 模型读文字知道该看哪一张，用户回看时也看得懂。
    pub fn with_images(content: impl Into<String>, images: Vec<crate::agent::message::ToolImage>) -> Self {
        Self { content: content.into(), is_error: false, images }
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
    /// 这个工具在哪些模式下出现。默认只在学习模式（见 [`ToolScope`]）。
    fn scope(&self) -> ToolScope {
        ToolScope::Study
    }
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

    /// 这个工具在当前模式下该不该出现。
    fn visible(&self, name: &str, mode: AgentMode) -> bool {
        match self.tools.get(name).map(|t| t.scope()) {
            Some(ToolScope::Both) => true,
            Some(ToolScope::Study) => mode == AgentMode::Study,
            Some(ToolScope::Studio) => mode == AgentMode::Studio,
            None => false,
        }
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

    /// 某个模式下真正要发给模型的工具声明。
    ///
    /// 学习模式与工坊模式给的清单不同（工坊不需要「出卷」「写卡片」这些要主题的工具，
    /// 但需要发布技能/登记 MCP 的工具）。过滤在这里做，主循环只管拿。
    pub fn specs_for(&self, mode: AgentMode) -> Vec<ToolSpec> {
        self.order
            .iter()
            .filter(|n| self.visible(n, mode))
            .filter_map(|n| self.tools.get(n))
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.schema(),
            })
            .collect()
    }

    pub fn names_for(&self, mode: AgentMode) -> Vec<String> {
        self.order
            .iter()
            .filter(|n| self.visible(n, mode))
            .cloned()
            .collect()
    }

    /// 生成给系统提示词用的工具清单（模型不支持 function calling 时的降级说明）。
    pub fn describe_for_prompt(&self) -> String {
        self.describe_for_prompt_for(AgentMode::Study)
    }

    pub fn describe_for_prompt_for(&self, mode: AgentMode) -> String {
        self.order
            .iter()
            .filter(|n| self.visible(n, mode))
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
