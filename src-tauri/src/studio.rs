//! 工坊（Studio）：一个**独立于学习**的 agent 模式，用来造技能（SKILL.md）与 MCP 服务器。
//!
//! 为什么单独一个模式，而不是塞进学习模式：
//! - 它**没有主题**。学习模式的一切都挂在主题上（相对路径的根、提示词里的资料清单、
//!   卡片与计划），而造技能这件事跟任何一门课都无关。
//! - 它的产出物是**给应用自己用的能力**：写 `.hub/skills/`、把服务器登记进
//!   `config.json` 的 `agent.mcp_servers`。这和「学习内容」（笔记、卡片、资料）是两码事，
//!   混在一起的代价是学习模式的工具表与提示词被一堆用不上的东西撑大。
//! - 它需要一个可以随便写的**练习目录**：造到一半的服务器、跑不通的脚本、
//!   要试的依赖，都不该直接落进正式目录。
//!
//! 目录约定（相对工作区）：
//! ```text
//! .hub/workshop/            练习目录：本模式下相对路径的根
//!   skills/<id>/SKILL.md    技能草稿（发布后进 .hub/skills/<id>/）
//!   mcp/<id>/               服务器草稿（发布后进 .hub/mcp/<id>/）
//! .hub/skills/<id>/         正式技能目录（skills.rs 扫描的第一优先级工作区来源）
//! .hub/mcp/<id>/            正式 MCP 服务器代码
//! ```
//!
//! 规范文档（[`SPEC_SKILL`] / [`SPEC_MCP`]）用 `include_str!` 编进二进制：
//! 它们是**给 agent 的操作手册**，随应用版本走——放在磁盘上会产生「用户删了 / 版本旧了」
//! 这类和代码不一致的状态，而这几千字本来就不该由用户维护。

use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};

/// 练习目录（工作区相对路径）。
pub const DIR_REL: &str = ".hub/workshop";
/// 正式技能目录（工作区相对路径）。与 `skills.rs` 里扫描的「工作区」来源必须一致。
pub const SKILLS_REL: &str = ".hub/skills";
/// 正式 MCP 服务器目录（工作区相对路径）。
pub const MCP_REL: &str = ".hub/mcp";

pub const DRAFT_SKILLS: &str = "skills";
pub const DRAFT_MCP: &str = "mcp";

/// 技能规范（给 agent 的操作手册）。
pub const SPEC_SKILL: &str = include_str!("../docs/studio/skill.md");
/// MCP 规范。
pub const SPEC_MCP: &str = include_str!("../docs/studio/mcp.md");

/// 练习目录的说明文件：用户点开这个目录时得知道它是干什么的。
const BENCH_README: &str = "\
# 工坊

这是「工坊」模式的练习目录。工坊是一个独立于学习的 agent 模式，用来**造技能和 MCP 服务器**：

- `skills/<名字>/SKILL.md` —— 技能草稿。用 `skill_publish` 发布后进工作区的 `.hub/skills/`。
- `mcp/<名字>/` —— MCP 服务器草稿。用 `mcp_publish` 发布后进 `.hub/mcp/`，并自动登记到配置里。

这个目录里的东西可以随便改、随便删——没发布之前不会有任何东西影响应用本身。
发布过的产物在 `.hub/skills/` 与 `.hub/mcp/` 下，也可以在「技能」与「MCP 服务器」面板里开关。
";

/// 工坊的路径计算（不做任何 IO，纯粹是为了让「目录在哪儿」只有一个说法）。
#[derive(Debug, Clone)]
pub struct Studio {
    ws_root: PathBuf,
}

impl Studio {
    pub fn new(ws_root: impl Into<PathBuf>) -> Self {
        Self { ws_root: ws_root.into() }
    }

    /// 练习目录：本模式下相对路径的根。
    pub fn dir(&self) -> PathBuf {
        self.ws_root.join(".hub").join("workshop")
    }

    /// 正式技能目录（发布目标）。
    pub fn skills_dir(&self) -> PathBuf {
        self.ws_root.join(".hub").join("skills")
    }

    /// 正式 MCP 服务器目录（发布目标）。
    pub fn mcp_dir(&self) -> PathBuf {
        self.ws_root.join(".hub").join("mcp")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.ws_root.join(".hub").join("trash")
    }

    /// 建好练习目录骨架（含两个草稿子目录与说明）。第一次进工坊时调用。
    pub fn ensure(&self) -> AppResult<PathBuf> {
        let dir = self.dir();
        crate::paths::ensure_dir(&dir.join(DRAFT_SKILLS))?;
        crate::paths::ensure_dir(&dir.join(DRAFT_MCP))?;
        let readme = dir.join("README.md");
        if !readme.is_file() {
            crate::store::atomic_write(&readme, BENCH_README.as_bytes())?;
        }
        Ok(dir)
    }

    /// 按名字取规范文档。`list`（或空）返回一份「有哪几份、什么时候读」的清单。
    pub fn spec(&self, doc: &str) -> AppResult<String> {
        match doc.trim().to_ascii_lowercase().as_str() {
            "skill" | "skills" | "技能" => Ok(SPEC_SKILL.to_string()),
            "mcp" | "server" | "服务器" => Ok(SPEC_MCP.to_string()),
            "" | "list" | "index" | "目录" => Ok(format!(
                "工坊内置两份规范文档，用 `spec_read` 按名字读全文（读完照着做，不要凭印象写）：\n\
                 - skill：技能（SKILL.md）的格式、description 怎么写、目录约定、发布流程。造技能前必读。\n\
                 - mcp：MCP 协议在本应用里的最小实现（stdio + 换行分帧）、Python/Node 骨架、\n\
                 工具定义、注册参数与排查步骤。造 MCP 服务器前必读。\n\
                 另外：`skill_list` 能看到已有的技能（避免重复造），`mcp_status` 能看到已登记的服务器。"
            )),
            other => Err(AppError::invalid(format!(
                "没有叫 {other} 的规范文档，可选：skill / mcp / list"
            ))),
        }
    }
}

/// 一份发布成功的技能。
#[derive(Debug, Clone)]
pub struct PublishedSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub files: usize,
    pub dir: PathBuf,
}

impl PublishedSkill {
    /// 正式目录的显示路径（工具输出里给模型看的位置）。
    pub fn dir_text(&self) -> String {
        self.dir.to_string_lossy().to_string()
    }
}

/// 技能目录名即技能 id。这里只做最基本的安全校验——
/// id 会成为正式技能目录的名字，绝不能带路径分隔符或 `..`。
pub fn sanitize_id(raw: &str) -> AppResult<String> {
    let name = raw.trim().trim_end_matches(['/', '\\']);
    // `..` 直接拒绝，而不是取最后一段把它消掉：静默换成另一个名字比报错更糟
    // （用户以为改的是 A，结果写进了 B）。
    if name.contains("..") {
        return Err(AppError::invalid(format!("名字里不能有 `..`：{raw}")));
    }
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if last.is_empty() {
        return Err(AppError::invalid("名字不能为空"));
    }
    if last.starts_with('.') {
        return Err(AppError::invalid(format!("名字不合法：{last}")));
    }
    // 目录名里允许中文与空格（技能 id 就是人起的名字），只挡掉文件系统不认的字符
    if last.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(AppError::invalid(format!("名字里有文件名不允许的字符：{last}")));
    }
    Ok(last.to_string())
}

/// 把一个草稿目录搬进正式目录：同名旧版本先移进回收站，再把整棵目录复制过去。
///
/// 为什么先移回收站而不是直接覆盖：发布是覆盖式的（改完再发一次是常态），
/// 但用户手写的旧版本不该被无声抹掉——回收站是这个项目对「删除」的一贯态度。
/// 返回（目标目录, 复制的文件数）。
pub fn publish_dir(src: &Path, dest_root: &Path, trash_root: &Path) -> AppResult<(PathBuf, usize)> {
    if !src.is_dir() {
        // 别在消息里再写「找不到」——`AppError::NotFound` 的 Display 已经带了那个前缀
        return Err(AppError::NotFound(format!("目录不存在：{}", src.display())));
    }
    let id = sanitize_id(&src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())?;
    let target = dest_root.join(&id);
    if target.exists() {
        crate::store::move_to_trash(trash_root, &target)?;
    }
    let files = copy_tree(src, &target)?;
    Ok((target, files))
}

/// 把一份技能草稿发布到正式技能目录：`src` 必须是一个含 `SKILL.md` 的目录。
///
/// 发布完成后从**正式目录**回读 name/description：这样「frontmatter 写坏了」
/// 会在发布的同一句话里暴露出来，而不是等到用户下次对话时发现技能描述是空的。
pub fn publish_skill(src: &Path, skills_root: &Path, trash_root: &Path) -> AppResult<PublishedSkill> {
    if !src.is_dir() {
        return Err(AppError::NotFound(format!(
            "目录不存在：{}（技能草稿应当是一个含 SKILL.md 的目录）",
            src.display()
        )));
    }
    if !src.join(crate::skills::SKILL_FILE).is_file() {
        return Err(AppError::invalid(format!(
            "{} 里没有 {}——技能的主文件必须叫这个名字",
            src.display(),
            crate::skills::SKILL_FILE
        )));
    }
    let (target, files) = publish_dir(src, skills_root, trash_root)?;
    let id = sanitize_id(&target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())?;

    let text = crate::store::read_text(&target.join(crate::skills::SKILL_FILE))?;
    let (name, description, _) = crate::skills::parse_front_matter(&text);
    Ok(PublishedSkill {
        id: id.clone(),
        name: name.unwrap_or(id),
        description: description.unwrap_or_default(),
        files,
        dir: target,
    })
}

/// 递归复制目录，跳过明显的垃圾（依赖目录、缓存、版本库）。
///
/// 为什么不用 `store::walk_files`：那个是给「列清单」用的（跳过点开头的目录），
/// 而技能目录里的 `.gitignore`、`scripts/` 这类东西是**要一起带走**的。
fn copy_tree(src: &Path, dst: &Path) -> AppResult<usize> {
    crate::paths::ensure_dir(dst)?;
    let mut count = 0usize;
    let entries = std::fs::read_dir(src).map_err(|e| crate::store::io_err(src, e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "node_modules" | "__pycache__" | ".git" | "target" | ".venv") {
            continue;
        }
        let to = dst.join(&name);
        if path.is_dir() {
            count += copy_tree(&path, &to)?;
        } else {
            let bytes = std::fs::read(&path).map_err(|e| crate::store::io_err(&path, e))?;
            crate::store::atomic_write(&to, &bytes)?;
            count += 1;
        }
    }
    Ok(count)
}

/// 发布 MCP 服务器前，把参数里指向练习目录的路径改写成发布后的位置。
///
/// 为什么要这一步：agent 在练习目录里写脚本时，很自然会用绝对路径（`python D:\…\workshop\mcp\x\server.py`）
/// 或者相对路径 + cwd。目录一搬，绝对路径就指到空气里去了，而那种失败表现为
/// 「连不上，错误信息是文件不存在」——很难看出是搬家导致的。
pub fn rewrite_paths_after_move(value: &str, from: &Path, to: &Path) -> String {
    let from_s = from.to_string_lossy().replace('\\', "/");
    let to_s = to.to_string_lossy().replace('\\', "/");
    let v = value.replace('\\', "/");
    if v.starts_with(&from_s) {
        return format!("{}{}", to_s, &v[from_s.len()..]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_docs_are_embedded_and_addressable() {
        let s = Studio::new(".");
        assert!(s.spec("skill").unwrap().contains("SKILL.md"));
        assert!(s.spec("mcp").unwrap().contains("initialize"));
        assert!(s.spec("").unwrap().contains("spec_read"));
        assert!(s.spec("nope").is_err());
        // 两份文档都要真材实料（被 include_str 掉成空文件是很容易犯的错）
        assert!(SPEC_SKILL.len() > 2000, "技能规范太短：{}", SPEC_SKILL.len());
        assert!(SPEC_MCP.len() > 4000, "MCP 规范太短：{}", SPEC_MCP.len());
    }

    #[test]
    fn ids_are_sanitized() {
        assert_eq!(sanitize_id("skills/lecture-to-cards").unwrap(), "lecture-to-cards");
        assert_eq!(sanitize_id("  文献检索  ").unwrap(), "文献检索");
        assert_eq!(sanitize_id("skills\\win-style\\").unwrap(), "win-style");
        assert!(sanitize_id("").is_err());
        assert!(sanitize_id("..").is_err());
        assert!(sanitize_id("skills/../etc").is_err());
        assert!(sanitize_id("a/b:c").is_err());
    }

    /// 发布：复制整棵目录、回读出 name/description、同名时旧的进回收站。
    #[test]
    fn publish_copies_and_reads_back_frontmatter() {
        let tmp = std::env::temp_dir().join(format!("lh-pub-{}", uuid::Uuid::new_v4()));
        let src = tmp.join("workshop/skills/lecture-to-cards");
        std::fs::create_dir_all(src.join("references")).unwrap();
        std::fs::write(
            src.join("SKILL.md"),
            "---\nname: 讲义转卡片\ndescription: 把讲义整理成问答卡片时用\n---\n\n## 步骤\n1. 读讲义\n",
        )
        .unwrap();
        std::fs::write(src.join("references/rules.md"), "细则").unwrap();
        // 垃圾目录必须被跳过
        std::fs::create_dir_all(src.join("__pycache__")).unwrap();
        std::fs::write(src.join("__pycache__/x.pyc"), "junk").unwrap();

        let skills_root = tmp.join(".hub/skills");
        let trash = tmp.join(".hub/trash");
        let published =
            publish_skill(&src, &skills_root, &trash).expect("应当发布成功");
        assert_eq!(published.id, "lecture-to-cards");
        assert_eq!(published.name, "讲义转卡片");
        assert_eq!(published.description, "把讲义整理成问答卡片时用");
        assert_eq!(published.files, 2, "SKILL.md + references/rules.md");
        assert!(skills_root.join("lecture-to-cards/references/rules.md").is_file());
        assert!(!skills_root.join("lecture-to-cards/__pycache__").exists());

        // 再发一次（改过内容）：目标被移进回收站，新内容生效
        std::fs::write(src.join("SKILL.md"), "---\nname: 讲义转卡片 v2\n---\n\n改过了\n").unwrap();
        let again = publish_skill(&src, &skills_root, &trash).unwrap();
        assert_eq!(again.name, "讲义转卡片 v2");
        assert!(trash.is_dir(), "旧版本应当进回收站而不是被删掉");
        assert_eq!(again.files, 2);

        // 没有 SKILL.md 的目录不许发布
        std::fs::create_dir_all(tmp.join("workshop/skills/empty")).unwrap();
        assert!(publish_skill(&tmp.join("workshop/skills/empty"), &skills_root, &trash).is_err());
        assert!(publish_skill(&tmp.join("workshop/skills/nope"), &skills_root, &trash).is_err());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn paths_are_rewritten_after_the_move() {
        let from = Path::new("D:/ws/.hub/workshop/mcp/wordbook");
        let to = Path::new("D:/ws/.hub/mcp/wordbook");
        assert_eq!(
            rewrite_paths_after_move("D:/ws/.hub/workshop/mcp/wordbook/server.py", from, to),
            "D:/ws/.hub/mcp/wordbook/server.py"
        );
        assert_eq!(
            rewrite_paths_after_move(r"D:\ws\.hub\workshop\mcp\wordbook\server.py", from, to),
            "D:/ws/.hub/mcp/wordbook/server.py"
        );
        // 不相干的路径原样返回（含反斜杠形式也要能识别前缀）
        assert_eq!(rewrite_paths_after_move("server.py", from, to), "server.py");
        assert_eq!(
            rewrite_paths_after_move("C:/other/x.py", from, to),
            "C:/other/x.py"
        );
    }
}
