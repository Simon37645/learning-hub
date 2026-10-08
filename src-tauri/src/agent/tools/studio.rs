//! 工坊模式的工具：读规范、发布技能、发布 MCP 服务器。
//!
//! 三件事按「照着规范做 → 发布 → 看结果」排成一条链：
//! `spec_read` 拿到规范 → 用 fs_* 在练习目录里写 → `skill_publish` / `mcp_publish` 发布，
//! 后者顺手重连 MCP 并把连接状态带回来——这就是工坊里唯一能做的「测试」。
//!
//! 这几个工具只在工坊模式出现（`ToolScope::Studio`）：它们改的是应用自己的配置与技能目录，
//! 和学习内容无关，放进学习模式的工具表只会占上下文。

use crate::agent::registry::{
    arg_str, arg_str_array, arg_str_req, object_schema, str_array_prop, str_prop, Tool, ToolCtx,
    ToolOutput, ToolScope,
};
use crate::agent::event::Risk;
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

fn studio_of(ctx: &ToolCtx) -> crate::studio::Studio {
    crate::studio::Studio::new(ctx.core.config_read().workspace_root.clone())
}

// ---------------------------------------------------------------- spec_read

pub struct SpecRead;

#[async_trait]
impl Tool for SpecRead {
    fn name(&self) -> &'static str {
        "spec_read"
    }

    fn description(&self) -> &'static str {
        "读工坊内置的规范文档（技能格式 / MCP 协议）。\n\
         造技能前先读 skill，造 MCP 服务器前先读 mcp——两份文档都是照着本应用的实现写的，\n\
         包含目录约定、参数怎么填、以及哪些做法在这套实现里行不通。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "doc": str_prop("skill（技能规范）/ mcp（MCP 规范）/ list（有哪几份，默认）")
            }),
            &[],
        )
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Studio
    }

    fn summarize(&self, input: &Value) -> String {
        format!("读规范文档（{}）", arg_str(input, "doc").unwrap_or_else(|| "list".into()))
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let doc = arg_str(&input, "doc").unwrap_or_default();
        Ok(ToolOutput::ok(studio_of(ctx).spec(&doc)?))
    }
}

// ---------------------------------------------------------------- skill_publish

pub struct SkillPublish;

#[async_trait]
impl Tool for SkillPublish {
    fn name(&self) -> &'static str {
        "skill_publish"
    }

    fn description(&self) -> &'static str {
        "把练习目录里的一个技能目录发布到正式技能目录（工作区 `.hub/skills/<名字>/`）。\n\
         目录里必须有 SKILL.md。同名技能会被覆盖（旧版本进回收站）。\n\
         发布后技能立刻生效：下一轮对话起，它的名字与说明会出现在系统提示词里。\n\
         发布前先读 `spec_read`（skill），确认 frontmatter 写了 name 与 description。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "dir": str_prop("练习目录里的技能目录，例如 skills/lecture-to-cards"),
            }),
            &["dir"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Studio
    }

    fn summarize(&self, input: &Value) -> String {
        format!("发布技能 {}", arg_str(input, "dir").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let dir = arg_str_req(&input, "dir")?;
        // 相对路径以练习目录为根；也允许用户/模型直接给工作区内的路径
        let src = if crate::paths::looks_absolute(&dir) {
            crate::paths::absolutize(std::path::Path::new(&dir))?
        } else {
            crate::paths::resolve_in_root(&ctx.root, &dir)?
        };
        let studio = studio_of(ctx);
        let published = crate::studio::publish_skill(&src, &studio.skills_dir(), &studio.trash_dir())?;

        // 重扫：让「技能面板 / 侧栏角标」与下一轮的提示词都立刻看到它
        let total = ctx.core.reload_skills();
        ctx.core.emit_topics(crate::agent::event::TopicsEvent::Refresh);

        let cfg = ctx.core.config_read();
        let visible = ctx.core.skills().iter().any(|s| s.id == published.id);
        let mut out = format!(
            "已发布技能「{}」（id={}）：\n- 位置：{}\n- 文件：{} 个\n- 说明：{}\n",
            published.name,
            published.id,
            published.dir_text(),
            published.files,
            if published.description.trim().is_empty() {
                "（没读到 description——SKILL.md 的 frontmatter 里要写一行 `description: 什么时候用这个技能`，否则模型看不出该不该用它）".to_string()
            } else {
                published.description.clone()
            }
        );
        if !cfg.agent.skills_enabled {
            out.push_str(
                "- ⚠ 技能总开关是**关着**的：技能已经装好，但现在不会生效，\
                 需要在侧栏「技能」面板里打开总开关。\n",
            );
        } else if visible {
            let disabled = cfg.agent.disabled_skills.iter().any(|d| d == &published.id);
            if disabled {
                out.push_str("- ⚠ 这个技能被全局禁用了，在「技能」面板里打开它才会生效。\n");
            } else {
                out.push_str(&format!(
                    "- 已生效：当前共 {total} 个技能，它下一轮对话起就会出现在系统提示词里。\n"
                ));
            }
        } else {
            out.push_str("- ⚠ 重扫后没在技能清单里找到它，检查一下目录名与 SKILL.md 的位置。\n");
        }
        out.push_str("用户可以随时在侧栏「技能」面板里查看、开关或编辑它。");
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- mcp_publish

pub struct McpPublish;

#[async_trait]
impl Tool for McpPublish {
    fn name(&self) -> &'static str {
        "mcp_publish"
    }

    fn description(&self) -> &'static str {
        "把一个 MCP 服务器登记进应用（写进配置的 agent.mcpServers），并**立刻重连**、\n\
         返回连接状态与它暴露的工具名——这就是服务器唯一的测试手段。\n\
         服务器代码放练习目录 `mcp/<id>/` 里；传 `dir` 会把它搬进正式目录 `.hub/mcp/<id>/`\n\
         并把 args/cwd 里指向练习目录的路径自动改写过去。\n\
         同名服务器会被覆盖（视为改配置）。连接失败时它会返回服务器的 stderr 摘要，照着改。\n\
         参数怎么写、服务器要满足什么协议，先读 `spec_read`（mcp）。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "name": str_prop("服务器名：也是配置里的键与工具名前缀（mcp__<name>__<tool>），用英文短名"),
                "command": str_prop("可执行文件，例如 python / py / node / npx / uvx，或 .exe 的完整路径"),
                "dir": str_prop("可选：练习目录里的服务器目录（例如 mcp/wordbook）。传了就会搬进 .hub/mcp/<id>/"),
                "args": str_array_prop("命令行参数，例如 [\"server.py\"]（相对路径按 cwd 解析）"),
                "env": json!({
                    "type": "object",
                    "description": "额外环境变量，例如 {\"PYTHONIOENCODING\":\"utf-8\",\"PYTHONUNBUFFERED\":\"1\"}",
                    "additionalProperties": { "type": "string" }
                }),
                "cwd": str_prop("可选：工作目录。不传时，搬过来的服务器以它自己的目录为 cwd"),
            }),
            &["name", "command"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Studio
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "登记 MCP 服务器 {}（{}）",
            arg_str(input, "name").unwrap_or_default(),
            arg_str(input, "command").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let name = arg_str_req(&input, "name")?;
        let command = arg_str_req(&input, "command")?;
        let id = crate::studio::sanitize_id(&name)?;
        if id != name {
            return Err(AppError::invalid(format!(
                "服务器名 {name} 里有不合适的字符（会变成配置里的键与工具名前缀），建议用英文短名"
            )));
        }
        let studio = studio_of(ctx);
        let mut args = arg_str_array(&input, "args");
        let mut cwd = arg_str(&input, "cwd").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let env: HashMap<String, String> = input
            .get("env")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // 1) 有草稿目录就先搬进正式位置（同名旧版本进回收站）
        let mut notes: Vec<String> = Vec::new();
        if let Some(dir) = arg_str(&input, "dir").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            let src = if crate::paths::looks_absolute(&dir) {
                crate::paths::absolutize(std::path::Path::new(&dir))?
            } else {
                crate::paths::resolve_in_root(&ctx.root, &dir)?
            };
            let (dest, files) = crate::studio::publish_dir(&src, &studio.mcp_dir(), &studio.trash_dir())?;
            notes.push(format!("代码：{}（{files} 个文件）", dest.display()));
            // 搬过家之后，参数里指向练习目录的路径要跟过去，否则一运行就是「文件不存在」
            args = args
                .iter()
                .map(|a| crate::studio::rewrite_paths_after_move(a, &src, &dest))
                .collect();
            if let Some(c) = cwd.clone() {
                let rewritten = crate::studio::rewrite_paths_after_move(&c, &src, &dest);
                cwd = Some(if crate::paths::looks_absolute(&c) {
                    // 绝对路径：前缀已经改写过了
                    rewritten
                } else {
                    // 相对路径：它本来就是相对练习目录写的，搬到正式目录后以新目录为基准
                    dest.join(rewritten).to_string_lossy().to_string()
                });
            } else {
                // 没给 cwd 时默认就在服务器自己的目录里跑（`python server.py` 才能找到脚本）
                cwd = Some(dest.to_string_lossy().to_string());
            }
        }
        if let Some(c) = cwd.clone() {
            if !std::path::Path::new(&c).is_dir() {
                notes.push(format!("⚠ 工作目录不存在：{c}（服务器很可能起不来）"));
            }
        }

        // 2) 登记进配置（同名＝改配置）
        let server = crate::mcp::McpServerConfig {
            name: name.clone(),
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            cwd: cwd.clone(),
            enabled: true,
        };
        let existed = ctx
            .core
            .config_read()
            .agent
            .mcp_servers
            .iter()
            .any(|s| s.name == name);
        ctx.core.update_config(|c| {
            match c.agent.mcp_servers.iter_mut().find(|s| s.name == name) {
                Some(slot) => *slot = server.clone(),
                None => c.agent.mcp_servers.push(server.clone()),
            }
        })?;

        // 3) 重连并回报状态——「发布 + 状态」一步到位，省掉一轮往返
        let status = ctx.core.mcp_reload().await;
        let entry = status.iter().find(|s| s.name == name);

        let mut out = format!(
            "已{} MCP 服务器「{}」：\n- 命令：{} {}\n",
            if existed { "更新" } else { "登记" },
            name,
            command,
            args.join(" ")
        );
        if let Some(c) = &cwd {
            out.push_str(&format!("- 工作目录：{c}\n"));
        }
        for n in &notes {
            out.push_str(&format!("- {n}\n"));
        }
        match entry {
            Some(e) if e.connected => {
                out.push_str(&format!("- 连接成功（{}）\n", e.server_info));
                out.push_str(&format!("- 提供 {} 个工具：{}\n", e.tool_count, e.tools.join("、")));
                out.push_str(
                    "- 这些工具从**下一轮**起可用，名字是 `mcp__<服务器名>__<工具名>`，\
                     可以直接调用它们验证行为。",
                );
            }
            Some(e) => {
                out.push_str(&format!(
                    "- ✗ 连接失败：{}\n\
                     排查顺序：① 命令能不能直接跑起来（python / node 是否在 PATH 上）；\
                     ② 服务器有没有往 **stdout** 打非协议内容（日志必须走 stderr）；\
                     ③ `initialize` 是否按规范回了结果。改完再 `mcp_publish` 一次即可。",
                    e.error.clone().unwrap_or_else(|| "未知原因（没有返回错误详情）".into())
                ));
            }
            None => {
                out.push_str("- ✗ 重连后没有它的状态记录，通常是配置没写进去（可以看 `mcp_status`）。\n");
            }
        }
        out.push_str("\n用户可以在侧栏「MCP 服务器」面板里查看、开关或删除它。");
        Ok(ToolOutput::ok(out))
    }
}
