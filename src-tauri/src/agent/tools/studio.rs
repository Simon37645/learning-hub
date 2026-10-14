//! 工坊模式的工具：读规范、发布技能、发布 MCP 服务器。
//!
//! 三件事按「照着规范做 → 发布 → 看结果」排成一条链：
//! `spec_read` 拿到规范 → 用 fs_* 在练习目录里写 → `skill_publish` / `mcp_publish` 发布，
//! 后者顺手重连 MCP 并把连接状态带回来——这就是工坊里唯一能做的「测试」。
//!
//! 这几个工具只在工坊模式出现（`ToolScope::Studio`）：它们改的是应用自己的配置与技能目录，
//! 和学习内容无关，放进学习模式的工具表只会占上下文。

use crate::agent::registry::{
    arg_str, arg_str_array, arg_str_req, object_schema, str_array_prop, str_prop, Access, Tool,
    ToolCtx, ToolOutput, ToolScope,
};
use crate::agent::event::Risk;
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

fn studio_of(ctx: &ToolCtx) -> crate::studio::Studio {
    crate::studio::Studio::new(ctx.core.config_read().workspace_root.clone())
}

/// 参数里这个值「像不像一个相对的文件名」：不含通配符与选项前缀，带扩展名或路径分隔符，
/// 且不是绝对路径。用来决定「没给 cwd 时该不该默认到服务器目录」。
fn looks_like_relative_file(arg: &str) -> bool {
    let a = arg.trim();
    if a.is_empty() || a.starts_with('-') || a.starts_with('~') {
        return false;
    }
    if crate::paths::looks_absolute(a) {
        return false;
    }
    a.contains('.') || a.contains('/') || a.contains('\\')
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
         返回连接状态与它暴露的工具名。\n\
         服务器代码放练习目录 `mcp/<id>/` 里；传 `dir` 会把它搬进正式目录 `.hub/mcp/<id>/`\n\
         并把 args/cwd 里指向练习目录的路径自动改写过去。\n\
         同名服务器会被覆盖（视为改配置）。连接失败时它会返回子进程的 **stderr 与退出码**，照着改。\n\
         更稳的顺序：先用 `mcp_probe` 试到通（不写配置），再 `mcp_publish` 登记。\n\
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
        } else if cwd.is_none() {
            // 没传 dir、也没给 cwd：如果 args 里有**相对的文件名**（`-File search.ps1`、
            // `server.py` 这种），相对的是**应用的当前目录**，几乎必然找不到——那就默认
            // 以「同名的正式目录」为工作目录（`workspace/.hub/mcp/<name>/`）。
            // 这个坑真踩过：一次发布完，界面上只有「已退出」三个字。
            let dir = studio.mcp_dir().join(&name);
            if dir.is_dir() && args.iter().any(|a| looks_like_relative_file(a)) {
                notes.push(format!(
                    "没给 cwd，但参数里有相对文件名——默认以 {} 为工作目录",
                    dir.display()
                ));
                cwd = Some(dir.to_string_lossy().to_string());
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
                     改完再 mcp_publish 一次即可（也可以先用 `mcp_probe` 试，不必写配置）。\n",
                    e.error.clone().unwrap_or_else(|| "未知原因（没有返回错误详情）".into())
                ));
                // 子进程自己的输出：以前这些行只写进应用的控制台，而发布版没有控制台，
                // 于是「起不来」永远只有一个词。现在照原样带回来。
                if !e.log.is_empty() {
                    out.push_str("\n它自己的输出（stderr / 启动事件，最近的在后面）：\n```\n");
                    out.push_str(&e.log.join("\n"));
                    out.push_str("\n```\n");
                }
                out.push_str(
                    "常见三类原因：① 命令没解析到（看日志里的「启动：」与「起不来」）；\
                     ② 服务器往 **stdout** 打了非协议内容（MCP stdio 一行一个 JSON-RPC，日志走 stderr）；\
                     ③ 脚本自己抛错或直接退出（看 stderr 尾巴与退出码）。",
                );
            }
            None => {
                out.push_str("- ✗ 重连后没有它的状态记录，通常是配置没写进去（可以看 `mcp_status`）。\n");
            }
        }
        out.push_str("\n用户可以在侧栏「MCP 服务器」面板里查看、开关或删除它。");
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- mcp_probe

/// 试运行一个 MCP 服务器：起一次、握手、拉一次工具清单、关掉。**不写配置、不重连**。
///
/// 为什么必须有：`mcp_publish` 会直接改配置——没有它的时候，agent 只能靠
/// 「发布 → 看状态」猜，猜错一次就污染一次配置（而且状态里没有任何原因：
/// 子进程的 stderr 以前只写进应用自己的控制台，发布版根本没有控制台）。
pub struct McpProbe;

#[async_trait]
impl Tool for McpProbe {
    fn name(&self) -> &'static str {
        "mcp_probe"
    }

    fn description(&self) -> &'static str {
        "试运行一个 MCP 服务器：按你给的命令起一次、握手、拉一次工具清单，然后关掉。\n\
         **不写配置、不重连**——发布（`mcp_publish`）之前先用它自测，改完再试，试通了再发布。\n\
         失败时会返回子进程 stderr 的尾巴、退出码，以及命令解析到哪个文件，照着改就行。\n\
         服务器代码放练习目录 `mcp/<id>/` 里；相对路径按 cwd 解析，不给 cwd 就用练习目录。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "name": str_prop("服务器名（只用于回报里显示，给个英文短名即可）"),
                "command": str_prop("可执行文件：node / python / npx，或 .exe 的完整路径"),
                "args": str_array_prop("命令行参数，例如 [\"server.js\"]"),
                "env": json!({
                    "type": "object",
                    "description": "额外环境变量，例如 {\"TAVILY_API_KEY\":\"tvly-…\"}",
                    "additionalProperties": { "type": "string" }
                }),
                "cwd": str_prop("可选：工作目录（相对练习目录）。不传就是练习目录"),
            }),
            &["name", "command"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "试运行 MCP：{} {}",
            arg_str(input, "command").unwrap_or_default(),
            arg_str_array(input, "args").join(" ")
        )
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Studio
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let cwd_arg = arg_str(&input, "cwd").unwrap_or_default();
        let cwd = if cwd_arg.trim().is_empty() {
            ctx.root.clone()
        } else {
            ctx.resolve_path(cwd_arg.trim(), None, Access::Read, "试运行 MCP 服务器的工作目录")
                .await?
        };
        let cfg = crate::mcp::McpServerConfig {
            name: arg_str(&input, "name").unwrap_or_else(|| "probe".into()),
            command: arg_str_req(&input, "command")?,
            args: arg_str_array(&input, "args"),
            env: input
                .get("env")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            cwd: Some(cwd.to_string_lossy().to_string()),
            enabled: true,
        };
        let report = crate::mcp::probe(&cfg).await;
        let mut out = format!("试运行结果（{} ms）：\n- {}\n", report.elapsed_ms, report.summary);
        if report.ok && !report.tools.is_empty() {
            out.push_str("- 工具清单就是上面那串；发布后它们的名字是 `mcp__<服务器名>__<工具名>`。\n");
        }
        if !report.log.is_empty() {
            out.push_str("\n它自己的输出（stderr / 启动事件，最近的在后面）：\n```\n");
            out.push_str(&report.log.join("\n"));
            out.push_str("\n```\n");
        }
        if !report.ok {
            out.push_str(
                "\n常见的三类原因：① 命令没解析到（看「启动：」那行与「起不来」）；\
                 ② 服务器往 **stdout** 打了非协议内容（MCP stdio 里一行一个 JSON-RPC，\
                 日志一律走 stderr）；③ 脚本自己抛错/直接退出（看 stderr 尾巴与退出码）。",
            );
        }
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- run_command

/// 跑一条命令。工坊里唯一的「真试跑」手段（agent 没有终端）。
///
/// 风险等级给到 `Destructive`：这是真的在本机执行、能改任何东西，
/// 「每次确认」模式下要用户点头；用户选了完全访问才会一路绿灯。
pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "在这台电脑上跑一条命令，把 stdout / stderr / 退出码拿回来。\n\
         默认工作目录是练习目录（工坊）；Windows 上交给 `cmd /C`，macOS / Linux 交给 `sh -c`。\n\
         装依赖、编译、试跑服务器、复现报错都用它——**别让用户自己动手**。\n\
         注意：① 这是真的在本机执行，不是沙箱；② 长驻进程（等 stdin 的服务器）会一直跑到超时，\
         用 `timeout_secs` 控制，或者干脆用 `mcp_probe` 去测服务器。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "command": str_prop("命令行，例如 \"node --version\"、\"npm install\"、\"python -m pytest -q\""),
                "cwd": str_prop("可选：工作目录（相对练习目录）。不传就是练习目录"),
                "timeout_secs": json!({
                    "type": "integer",
                    "description": "超时秒数（默认 60，最长 600）；到点就把进程放掉"
                }),
            }),
            &["command"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Destructive
    }

    fn summarize(&self, input: &Value) -> String {
        format!("运行命令：{}", arg_str(input, "command").unwrap_or_default())
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Studio
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let line = arg_str_req(&input, "command")?;
        let cwd_arg = arg_str(&input, "cwd").unwrap_or_default();
        let cwd = if cwd_arg.trim().is_empty() {
            ctx.root.clone()
        } else {
            ctx.resolve_path(cwd_arg.trim(), None, Access::Read, "命令的工作目录").await?
        };
        let timeout = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(1, 600);

        #[cfg(windows)]
        let (program, args) = ("cmd.exe", vec!["/C".to_string(), line.clone()]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c".to_string(), line.clone()]);

        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 超时/出错时别留孤儿进程
            .kill_on_drop(true);
        crate::proc::no_console(cmd.as_std_mut());

        let outcome = tokio::time::timeout(Duration::from_secs(timeout), cmd.output()).await;
        let output = match outcome {
            Err(_) => {
                return Ok(ToolOutput::ok(format!(
                    "命令超过 {timeout} 秒还没结束，已经把它放掉了（要跑长任务就调大 `timeout_secs`；\
                     如果是「启动一个等 stdin 的服务器」这类长驻进程，请改用 `mcp_probe` 去试）。\n\
                     命令：{line}"
                )));
            }
            Ok(Err(e)) => {
                return Ok(ToolOutput::ok(format!(
                    "起不来：{e}\n（在 {cwd:?} 下执行：{line}）",
                    cwd = cwd.display()
                )));
            }
            Ok(Ok(o)) => o,
        };

        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "（被信号终止）".into());
        let mut text = format!("命令：{line}\n退出码：{code}\n");
        // 按本地代码页兜底解码：cmd / PowerShell 5.1 的中文输出不是 UTF-8（见 proc::decode_output）
        let stdout = crate::proc::decode_output(&output.stdout);
        let stderr = crate::proc::decode_output(&output.stderr);
        if !stdout.trim().is_empty() {
            text.push_str("\n[stdout]\n");
            text.push_str(&clamp_tail(&stdout, 12000));
        }
        if !stderr.trim().is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&clamp_tail(&stderr, 8000));
        }
        if stdout.trim().is_empty() && stderr.trim().is_empty() {
            text.push_str("\n（没有任何输出）");
        }
        Ok(ToolOutput::ok(text))
    }
}

/// 太长的输出留头也留尾：**报错通常在最后几行**。
fn clamp_tail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.trim_end().to_string();
    }
    let head: String = s.chars().take(max / 3).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(max - max / 3)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}\n…（中间省略）…\n{tail}")
}
