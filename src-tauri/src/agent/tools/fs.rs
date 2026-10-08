//! 文件工具：agent 在主题目录里的读写手。
//!
//! 路径规则（见 [`ToolCtx::resolve_path`]）：
//! - `notes/a.md` 这类相对路径 = 当前主题内的文件
//! - 想读写别的主题：用 `topic` 参数指定主题名
//! - 绝对路径（`D:\课件\x.pdf`、`~/Downloads/x.pdf`）会触发**越权申请**，
//!   用户批准后整个目录长期有效
//!
//! PDF / 文本 / 代码统一走 [`read_document`]，模型不需要关心文件类型。

use crate::agent::event::Risk;
use crate::agent::registry::{
    arg_str, arg_str_req, arg_u32, num_prop, object_schema, str_prop, Access, Tool, ToolCtx,
    ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::paths::{ensure_dir, human_size};
use crate::store;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

/// 单次读入模型上下文的字符上限。
const READ_CAP: usize = 60_000;

/// 读出文件内容：Markdown/文本直读，PDF 走文本提取。
pub async fn read_document(path: &Path) -> AppResult<String> {
    let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return read_pdf(path).await;
    }

    if !store::is_texty(path) {
        let size = std::fs::metadata(path).map(|m| human_size(m.len())).unwrap_or_default();
        return Err(AppError::invalid(format!(
            "这不是可读文本文件（{size}）：{}",
            path.display()
        )));
    }
    store::read_text_capped(path, READ_CAP as u64 * 4)
}

/// PDF 文本提取（CPU 密集，丢到阻塞线程池）。
pub async fn read_pdf(path: &Path) -> AppResult<String> {
    let p = path.to_path_buf();
    let display = p.display().to_string();
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text(&p))
        .await
        .map_err(|e| AppError::other(format!("PDF 解析任务失败：{e}")))?
        .map_err(|e| AppError::other(format!("PDF 文本提取失败（可能是扫描件）：{e}")))?;
    if text.trim().is_empty() {
        return Err(AppError::other(format!(
            "这份 PDF 提取不到文字，可能是扫描图片：{display}。可以先用内置浏览器打开，让用户自己看。"
        )));
    }
    Ok(text)
}

/// 从路径猜一个人类可读的类型标签。
pub fn kind_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn truncate_for_model(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n\n…（内容过长已截断，可用 fs_search 定位具体段落）");
    out
}

// ---------------------------------------------------------------- fs_list

pub struct FsList;

#[async_trait]
impl Tool for FsList {
    fn name(&self) -> &'static str {
        "fs_list"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "列出目录里的文件与子目录（含大小）。路径相对当前主题目录；\
         想列别的主题就传 topic；绝对路径会触发越权申请。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("相对主题目录的路径，默认主题根目录，例如 materials 或 notes"),
                "topic": str_prop("要看另一个主题时传它的名字"),
                "depth": num_prop("递归深度，默认 2，最大 5"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "列出目录 {}",
            arg_str(input, "path").unwrap_or_else(|| ".".into())
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic_arg = arg_str(&input, "topic");
        let rel = arg_str(&input, "path").unwrap_or_default();
        let depth = arg_u32(&input, "depth").unwrap_or(2).clamp(1, 5) as usize;

        let dir = if rel.trim().is_empty() {
            ctx.root_for(topic_arg.as_deref())?
        } else {
            ctx.resolve_path(&rel, topic_arg.as_deref(), Access::Read, "列目录")
                .await?
        };
        if !dir.is_dir() {
            return Err(AppError::NotFound(format!("目录不存在：{}", ctx.label(&dir))));
        }

        let label = ctx.label(&dir);
        let mut lines = format!("目录：{label}（深度 {depth}）\n");
        let mut count = 0;
        for entry in walkdir::WalkDir::new(&dir)
            .max_depth(depth)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
            .flatten()
        {
            if entry.depth() == 0 {
                continue;
            }
            let indent = "  ".repeat(entry.depth().saturating_sub(1));
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_dir() {
                lines.push_str(&format!("{indent}{name}/\n"));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                lines.push_str(&format!("{indent}{name}  ({})\n", human_size(size)));
                count += 1;
            }
        }
        if count == 0 {
            lines.push_str("（空目录）\n");
        }
        Ok(ToolOutput::ok(lines))
    }
}

// ---------------------------------------------------------------- fs_read

pub struct FsRead;

#[async_trait]
impl Tool for FsRead {
    fn name(&self) -> &'static str {
        "fs_read"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "读取一个文件（Markdown / 文本 / 代码 / PDF 均可，PDF 会自动提取文字）。\
         路径相对当前主题；读别的主题传 topic；读工作区之外的绝对路径会先向你申请授权。\
         读长文档前先用 fs_search 定位。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("文件路径，例如 notes/特征值.md 或 materials/ch1.pdf"),
                "topic": str_prop("读另一个主题时传它的名字"),
                "max_chars": num_prop("最多返回多少字符，默认 40000"),
            }),
            &["path"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("读取 {}", arg_str(input, "path").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let rel = arg_str_req(&input, "path")?;
        let topic_arg = arg_str(&input, "topic");
        let path = ctx
            .resolve_path(&rel, topic_arg.as_deref(), Access::Read, "读取文件内容")
            .await?;
        if !path.exists() {
            return Err(AppError::NotFound(format!("文件不存在：{}", ctx.label(&path))));
        }
        let max = arg_u32(&input, "max_chars").unwrap_or(40_000) as usize;
        let text = read_document(&path).await?;
        Ok(ToolOutput::ok(format!(
            "文件：{}\n\n{}",
            ctx.label(&path),
            truncate_for_model(&text, max)
        )))
    }
}

// ---------------------------------------------------------------- fs_write

pub struct FsWrite;

#[async_trait]
impl Tool for FsWrite {
    fn name(&self) -> &'static str {
        "fs_write"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "写入文件。mode=create 仅在文件不存在时新建（默认，最安全）；\
         mode=overwrite 整体覆盖；mode=append 追加到末尾。父目录会自动创建。\
         覆盖已有文件前务必先用 fs_read 看过内容。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("相对主题目录的文件路径，例如 notes/特征值.md"),
                "content": str_prop("要写入的完整内容"),
                "mode": {
                    "type": "string",
                    "enum": ["create", "overwrite", "append"],
                    "description": "写入模式，默认 create"
                },
                "topic": str_prop("写到另一个主题时传它的名字"),
            }),
            &["path", "content"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        let path = arg_str(input, "path").unwrap_or_default();
        let mode = arg_str(input, "mode").unwrap_or_else(|| "create".into());
        let chars = arg_str(input, "content").map(|c| c.chars().count()).unwrap_or(0);
        let verb = match mode.as_str() {
            "overwrite" => "覆盖",
            "append" => "追加到",
            _ => "写入",
        };
        format!("{verb} {path}（约 {chars} 字）")
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let rel = arg_str_req(&input, "path")?;
        let content = arg_str(&input, "content").unwrap_or_default();
        let mode = arg_str(&input, "mode").unwrap_or_else(|| "create".into());
        let topic_arg = arg_str(&input, "topic");
        let path = ctx
            .resolve_path(&rel, topic_arg.as_deref(), Access::Write, "写入文件")
            .await?;

        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }

        let label = ctx.label(&path);
        let existed = path.exists();
        match mode.as_str() {
            "append" => {
                let mut existing = store::read_text_opt(&path)?.unwrap_or_default();
                if !existing.is_empty() && !existing.ends_with('\n') {
                    existing.push('\n');
                }
                existing.push_str(&content);
                store::atomic_write(&path, existing.as_bytes())?;
            }
            "overwrite" => {
                store::atomic_write(&path, content.as_bytes())?;
            }
            _ => {
                if existed {
                    return Err(AppError::invalid(format!(
                        "{label} 已存在。请先 fs_read 看内容，然后再决定用 mode=append 追加还是 mode=overwrite 覆盖。"
                    )));
                }
                store::atomic_write(&path, content.as_bytes())?;
            }
        }

        let size = std::fs::metadata(&path).map(|m| human_size(m.len())).unwrap_or_default();
        Ok(ToolOutput::ok(format!(
            "已{} {label}（{}，现在共 {}）",
            if existed { "更新" } else { "创建" },
            content.chars().count(),
            size
        )))
    }
}

// ---------------------------------------------------------------- fs_mkdir

pub struct FsMkdir;

#[async_trait]
impl Tool for FsMkdir {
    fn name(&self) -> &'static str {
        "fs_mkdir"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "创建目录（已存在则忽略）。用于按章节整理资料，例如 materials/第三章。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("相对主题目录的目录路径"),
                "topic": str_prop("在另一个主题里创建时传它的名字"),
            }),
            &["path"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!("创建目录 {}", arg_str(input, "path").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let rel = arg_str_req(&input, "path")?;
        let topic_arg = arg_str(&input, "topic");
        let path = ctx
            .resolve_path(&rel, topic_arg.as_deref(), Access::Write, "创建目录")
            .await?;
        ensure_dir(&path)?;
        Ok(ToolOutput::ok(format!("目录已就绪：{}", ctx.label(&path))))
    }
}

// ---------------------------------------------------------------- fs_move

pub struct FsMove;

#[async_trait]
impl Tool for FsMove {
    fn name(&self) -> &'static str {
        "fs_move"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "移动或重命名文件/目录。目标已存在时会失败，避免误覆盖。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "from": str_prop("源路径（相对主题目录）"),
                "to": str_prop("目标路径（相对主题目录）"),
                "topic": str_prop("操作另一个主题时传它的名字"),
            }),
            &["from", "to"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "移动 {} → {}",
            arg_str(input, "from").unwrap_or_default(),
            arg_str(input, "to").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let from_rel = arg_str_req(&input, "from")?;
        let to_rel = arg_str_req(&input, "to")?;
        let topic_arg = arg_str(&input, "topic");
        let from = ctx
            .resolve_path(&from_rel, topic_arg.as_deref(), Access::Write, "移动文件（源）")
            .await?;
        let to = ctx
            .resolve_path(&to_rel, topic_arg.as_deref(), Access::Write, "移动文件（目标）")
            .await?;
        if !from.exists() {
            return Err(AppError::NotFound(format!("源路径不存在：{}", ctx.label(&from))));
        }
        if to.exists() {
            return Err(AppError::invalid(format!("目标已存在：{}", ctx.label(&to))));
        }
        if let Some(parent) = to.parent() {
            ensure_dir(parent)?;
        }
        std::fs::rename(&from, &to)?;
        Ok(ToolOutput::ok(format!(
            "已移动 {} → {}",
            ctx.label(&from),
            ctx.label(&to)
        )))
    }
}

// ---------------------------------------------------------------- fs_delete

pub struct FsDelete;

#[async_trait]
impl Tool for FsDelete {
    fn name(&self) -> &'static str {
        "fs_delete"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "删除主题内的文件或**空**目录。这是不可恢复操作（主题内的→回收站，主题外的→真删），\
         只应在用户明确要求时使用。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "path": str_prop("相对主题目录的路径"),
                "topic": str_prop("操作另一个主题时传它的名字"),
            }),
            &["path"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Destructive
    }

    fn summarize(&self, input: &Value) -> String {
        format!("删除 {}", arg_str(input, "path").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let rel = arg_str_req(&input, "path")?;
        let topic_arg = arg_str(&input, "topic");
        let path = ctx
            .resolve_path(&rel, topic_arg.as_deref(), Access::Delete, "删除文件")
            .await?;
        let label = ctx.label(&path);
        if !path.exists() {
            return Err(AppError::NotFound(format!("路径不存在：{label}")));
        }

        // 工作区内的删除走回收站，可恢复；工作区外的只能真删（用户已明确授权）
        let inside_ws = ctx.core.is_inside_workspace(&path);
        if path.is_dir() {
            let mut entries = std::fs::read_dir(&path)?;
            if entries.next().is_some() {
                return Err(AppError::Denied(format!(
                    "目录非空，出于安全考虑不递归删除：{label}"
                )));
            }
            std::fs::remove_dir(&path)?;
        } else if inside_ws {
            let trash = ctx
                .core
                .workspace()
                .root
                .join(crate::domain::topic::DIR_INTERNAL)
                .join("trash");
            store::move_to_trash(&trash, &path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        Ok(ToolOutput::ok(format!(
            "已删除 {label}{}",
            if inside_ws { "（进了回收站，可手动恢复）" } else { "" }
        )))
    }
}

// ---------------------------------------------------------------- fs_search

pub struct FsSearch;

#[async_trait]
impl Tool for FsSearch {
    fn name(&self) -> &'static str {
        "fs_search"
    }

    // 工坊模式也能用：那里的相对路径以工坊目录为根（见 ToolCtx::root）
    fn scope(&self) -> crate::agent::registry::ToolScope {
        crate::agent::registry::ToolScope::Both
    }

    fn description(&self) -> &'static str {
        "在主题的笔记与资料里做全文检索（支持中文子串与正则），返回命中文件、行号与上下文。\
         PDF 也会被检索（按提取出的文字）。回答用户问题前先查一遍，能避免重复劳动、也能引用已有笔记。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("检索词，默认按子串匹配；regex=true 时按正则解释"),
                "path": str_prop("限定目录，默认整个主题"),
                "topic": str_prop("检索另一个主题时传它的名字"),
                "regex": { "type": "boolean", "description": "是否按正则匹配，默认 false" },
                "case_sensitive": { "type": "boolean", "description": "是否区分大小写，默认 false" },
                "limit": num_prop("最多返回多少条命中，默认 40"),
            }),
            &["query"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("检索「{}」", arg_str(input, "query").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let query = arg_str_req(&input, "query")?;
        let regex_mode = input.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
        let case_sensitive = input.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(false);
        let limit = arg_u32(&input, "limit").unwrap_or(40).min(200) as usize;
        let topic_arg = arg_str(&input, "topic");

        let root = match arg_str(&input, "path") {
            Some(p) if !p.trim().is_empty() => {
                ctx.resolve_path(&p, topic_arg.as_deref(), Access::Read, "检索目录")
                    .await?
            }
            _ => ctx.root_for(topic_arg.as_deref())?,
        };

        let matcher = build_matcher(&query, regex_mode, case_sensitive)?;
        let mut hits: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        let mut hit_files = 0usize;

        for file in store::walk_files(&root, 6) {
            if hits.len() >= limit {
                break;
            }
            scanned += 1;
            if scanned > 3000 {
                break;
            }
            let is_pdf = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            let text = if is_pdf {
                match read_document(&file).await {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            } else if store::is_texty(&file) {
                match store::read_text_capped(&file, 2 * 1024 * 1024) {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            } else {
                continue;
            };

            let label = ctx.label(&file);
            let mut file_hit = false;
            for (n, line) in text.lines().enumerate() {
                if hits.len() >= limit {
                    break;
                }
                if matcher(line) {
                    file_hit = true;
                    let line = line.trim();
                    let snippet: String = line.chars().take(220).collect();
                    hits.push(format!("{label}:{}: {snippet}", n + 1));
                }
            }
            if file_hit {
                hit_files += 1;
            }
        }

        if hits.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "没有找到「{query}」（扫描了 {scanned} 个文件）。"
            )));
        }
        Ok(ToolOutput::ok(format!(
            "在 {hit_files} 个文件里找到 {} 条命中（扫描 {scanned} 个文件）：\n\n{}",
            hits.len(),
            hits.join("\n")
        )))
    }
}

type Matcher = Box<dyn Fn(&str) -> bool + Send + Sync>;

fn build_matcher(query: &str, regex_mode: bool, case_sensitive: bool) -> AppResult<Matcher> {
    if regex_mode {
        let pattern = if case_sensitive { query.to_string() } else { format!("(?i){query}") };
        let re = regex::Regex::new(&pattern)
            .map_err(|e| AppError::invalid(format!("正则不合法：{e}")))?;
        Ok(Box::new(move |line: &str| re.is_match(line)))
    } else if case_sensitive {
        let q = query.to_string();
        Ok(Box::new(move |line: &str| line.contains(&q)))
    } else {
        let q = query.to_lowercase();
        Ok(Box::new(move |line: &str| line.to_lowercase().contains(&q)))
    }
}

// ---------------------------------------------------------------- material_import

pub struct MaterialImport;

#[async_trait]
impl Tool for MaterialImport {
    fn name(&self) -> &'static str {
        "material_import"
    }

    fn description(&self) -> &'static str {
        "把工作区之外的资料（讲义、课件、论文）复制进某个主题的 materials/ 目录，\
         之后就能正常检索和阅读了。源路径可以是绝对路径（会先向你申请授权）。\
         用户说「我的课件在 D:\\xxx」时用它；要放进别的主题（例如父主题）就传 topic。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "sources": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "源文件路径列表（绝对路径或相对主题目录）"
                },
                "subdir": str_prop("放进 materials/ 下的哪个子目录，默认直接放 materials/"),
                "topic": str_prop("放进哪个主题，默认当前主题"),
                "move": { "type": "boolean", "description": "是否用移动代替复制，默认 false" },
            }),
            &["sources"],
        )
    }

    fn risk(&self) -> Risk {
        Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        let n = input
            .get("sources")
            .and_then(|s| s.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        format!("导入 {n} 个资料到 materials/")
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let sources = crate::agent::registry::arg_str_array(&input, "sources");
        if sources.is_empty() {
            return Err(AppError::invalid("sources 不能为空"));
        }
        let move_file = crate::agent::registry::arg_bool(&input, "move").unwrap_or(false);
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;

        let target_dir = match arg_str(&input, "subdir").as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(sub) => {
                ensure_dir(&topic.materials_dir())?;
                crate::paths::resolve_in_root(&topic.materials_dir(), sub)?
            }
            None => topic.materials_dir(),
        };
        ensure_dir(&target_dir)?;

        let mut imported: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for src in &sources {
            let from = match ctx
                .resolve_path(src, None, Access::Read, "导入资料到主题")
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    skipped.push(format!("{src}（{e}）"));
                    continue;
                }
            };
            if !from.is_file() {
                skipped.push(format!("{src}（不是文件）"));
                continue;
            }
            let name = from
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "imported".into());
            let dest = crate::paths::unique_child(&target_dir, &name);
            let result = if move_file {
                std::fs::rename(&from, &dest).or_else(|_| std::fs::copy(&from, &dest).map(|_| ()))
            } else {
                std::fs::copy(&from, &dest).map(|_| ())
            };
            match result {
                Ok(()) => imported.push(ctx.label(&dest)),
                Err(e) => skipped.push(format!("{src}（{e}）")),
            }
        }

        // 统计刷新让侧栏立刻看到资料数变化
        if let Ok(t) = ctx.core.workspace().resolve(&topic.slug()) {
            ctx.core.emit_topics_updated(&t);
        }

        let mut out = String::new();
        if !imported.is_empty() {
            out.push_str(&format!(
                "已{} {} 个资料到 {}/\n{}",
                if move_file { "移动" } else { "复制" },
                imported.len(),
                ctx.label(&target_dir),
                imported
                    .iter()
                    .map(|p| format!("- {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
            out.push_str("\n\n现在可以用 fs_read 读它们（PDF 会自动提取文字）、用 viewer_open 打开给用户看。");
        }
        if !skipped.is_empty() {
            out.push_str(&format!("\n\n跳过 {} 个：\n{}", skipped.len(), skipped.join("\n")));
        }
        Ok(ToolOutput::ok(out))
    }
}

/// 供其它模块复用的「列出主题内所有文件相对路径」。
pub fn list_topic_files(dir: &Path, depth: usize) -> Vec<String> {
    store::walk_files(dir, depth)
        .into_iter()
        .map(|p| crate::paths::rel_in_root(dir, &p))
        .collect()
}
