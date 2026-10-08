//! 技能与 MCP 的元工具：让 agent 自己发现「我还能用什么」。
//!
//! 技能走渐进式披露：系统提示词里只列名字 + 一句话说明，
//! 需要时用 `skill_read` 把正文读进来。MCP 的工具则直接注册成普通工具，
//! `mcp_status` 只用来排查「为什么那个工具没出现」。

use crate::agent::registry::{arg_str, arg_str_req, object_schema, str_prop, Tool, ToolCtx, ToolOutput};
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct SkillList;

#[async_trait]
impl Tool for SkillList {
    fn name(&self) -> &'static str {
        "skill_list"
    }

    // 工坊模式的整个工作就是造技能与 MCP 服务器，这三个工具在那边更要紧
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "列出可用的技能（每项只有名字与适用场景）。技能是「怎么做某类事」的成文经验，\n\
         当任务与某个技能的说明对得上时，用 `skill_read` 读它的正文再照做。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "查看可用技能".into()
    }

    async fn run(&self, ctx: &ToolCtx, _input: Value) -> AppResult<ToolOutput> {
        let skills = ctx.core.skills_for(ctx.topic.as_ref());
        if skills.is_empty() {
            return Ok(ToolOutput::ok(
                "当前没有可用技能。可以把技能放进「工作区/.hub/skills/<名字>/SKILL.md」，\
                 或用户目录的「.agents/skills/」下（与其它 agent 工具共用同一套格式）。"
                    .to_string(),
            ));
        }
        let mut out = format!("共 {} 个技能：\n\n", skills.len());
        for s in &skills {
            out.push_str(&format!("- {}（{}）：{}\n", s.name, s.id, s.description));
            if !s.files.is_empty() {
                out.push_str(&format!("    附带文件 {} 个\n", s.files.len()));
            }
        }
        out.push_str("\n决定要用某个技能后，先 `skill_read` 把正文读完，再按它的步骤做。");
        Ok(ToolOutput::ok(out))
    }
}

pub struct SkillRead;

#[async_trait]
impl Tool for SkillRead {
    fn name(&self) -> &'static str {
        "skill_read"
    }

    // 工坊模式的整个工作就是造技能与 MCP 服务器，这三个工具在那边更要紧
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "读一个技能的完整说明（正文 + 附带文件清单）。读完请严格按里面的步骤执行，\n\
         如果技能里引用了附加文件，再用 fs_read 读对应文件（路径要拼上技能目录）。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({ "name": str_prop("技能名或目录名，来自 skill_list") }),
            &["name"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("读技能「{}」", arg_str(input, "name").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let name = arg_str_req(&input, "name")?;
        let skills = ctx.core.skills_for(ctx.topic.as_ref());
        let skill = crate::skills::find(&skills, &name)
            .ok_or_else(|| AppError::NotFound(format!("找不到技能「{name}」")))?;
        Ok(ToolOutput::ok(crate::skills::read_body(skill)?))
    }
}

pub struct McpStatus;

#[async_trait]
impl Tool for McpStatus {
    fn name(&self) -> &'static str {
        "mcp_status"
    }

    // 工坊模式的整个工作就是造技能与 MCP 服务器，这三个工具在那边更要紧
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "查看 MCP 服务器的连接状态与它们提供的工具。\n\
         如果用户说「我配的那个工具怎么不能用」，先用它排查。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "查看 MCP 连接状态".into()
    }

    async fn run(&self, ctx: &ToolCtx, _input: Value) -> AppResult<ToolOutput> {
        let status = ctx.core.mcp_status().await;
        if status.is_empty() {
            return Ok(ToolOutput::ok(
                "还没有配置 MCP 服务器。可以在「设置 → MCP」里添加（填命令与参数，例如 npx / uvx 起的服务）。"
                    .to_string(),
            ));
        }
        let mut out = String::new();
        for s in &status {
            out.push_str(&format!(
                "- {}（{}）：{}，提供 {} 个工具\n",
                s.name,
                s.command,
                if s.connected {
                    format!("已连接 · {}", s.server_info)
                } else {
                    format!("未连接：{}", s.error.clone().unwrap_or_else(|| "未知原因".into()))
                },
                s.tool_count
            ));
            for t in &s.tools {
                out.push_str(&format!("    · {}\n", t));
            }
        }
        Ok(ToolOutput::ok(out))
    }
}
