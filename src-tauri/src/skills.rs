//! 技能（Skill）：把「怎么做某类事」的经验写成 Markdown，让 agent 按需加载。
//!
//! 格式与 ZCode / Claude 的 Agent Skills 兼容——`SKILL.md` 里一段 YAML frontmatter
//! （`name` / `description`）+ 正文。这样做的好处是**已有的技能目录可以直接用**：
//! 用户机器上 `~/.agents/skills/` 里的东西搬过来就能跑，不用改格式。
//!
//! 加载策略是「渐进式披露」：
//! 1. 启动时只把 name + description 列进系统提示词（几百字，几乎不占上下文）
//! 2. 模型判断需要某个技能时，用 `skill_read` 把正文（和它引用的子文件列表）读进来
//! 3. 技能正文里如果提到 `references/xxx.md` 之类的文件，模型可以再按需读

use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 主文件固定叫这个（与其它 agent 生态一致）
pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// 技能标识（目录名，除非 frontmatter 里显式给了 name）
    pub id: String,
    pub name: String,
    /// 一句话说明「什么时候该用它」——模型靠这句决定要不要加载
    pub description: String,
    /// 技能目录
    pub dir: String,
    /// 正文（不含 frontmatter）
    pub body: String,
    /// 目录里的附加文件，相对技能目录
    pub files: Vec<String>,
    /// 来自哪个搜索路径（界面上区分「内置」与「自己的」）
    pub source: String,
}

/// 轻量 frontmatter 解析：只认 `key: value`，不引 YAML 依赖。
/// 技能文件里的 frontmatter 就是这两个字段，够用且行为可预测。
fn parse_front_matter(text: &str) -> (Option<String>, Option<String>, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.trim_start().starts_with("---") {
        return (None, None, text.to_string());
    }
    let start = text.find("---").map(|i| i + 3).unwrap_or(0);
    let rest = &text[start..];
    let Some(end) = rest.find("\n---") else {
        return (None, None, text.to_string());
    };
    let head = &rest[..end];
    let mut name = None;
    let mut description = None;
    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match k.trim().to_ascii_lowercase().as_str() {
            "name" => name = Some(v),
            "description" => description = Some(v),
            _ => {}
        }
    }
    let body = rest[end + 4..].trim_start_matches(['\n', '\r']).to_string();
    (name, description, body)
}

/// 扫描一个目录下的技能（每个子目录一个技能）。
fn scan_dir(root: &Path, source: &str, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            // 也允许 root 下直接放一个 SKILL.md（单技能目录）
            if dir.file_name().is_some_and(|n| n == SKILL_FILE) {
                if let Some(skill) = load_skill(root, source) {
                    push_unique(out, skill);
                }
            }
            continue;
        }
        if let Some(skill) = load_skill(&dir, source) {
            push_unique(out, skill);
        }
    }
}

fn push_unique(out: &mut Vec<Skill>, skill: Skill) {
    // 同名技能：先扫到的（优先级更高的目录）保留
    if !out.iter().any(|s| s.id == skill.id) {
        out.push(skill);
    }
}

fn load_skill(dir: &Path, source: &str) -> Option<Skill> {
    let file = dir.join(SKILL_FILE);
    let text = std::fs::read_to_string(&file).ok()?;
    let (name, description, body) = parse_front_matter(&text);
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".into());
    let name = name.unwrap_or_else(|| id.clone());
    let description = description.unwrap_or_else(|| {
        // 没写 description 时退而求其次：取正文第一行非空内容
        body.lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("（这个技能没有写说明）")
            .to_string()
    });

    let mut files = Vec::new();
    for f in crate::store::walk_files(dir, 3) {
        if let Ok(rel) = f.strip_prefix(dir) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel != SKILL_FILE {
                files.push(rel);
            }
        }
    }
    files.sort();

    Some(Skill {
        id,
        name,
        description,
        dir: dir.to_string_lossy().to_string(),
        body,
        files,
        source: source.to_string(),
    })
}

/// 搜索所有技能目录。顺序即优先级（先扫到的同名技能胜出）。
///
/// - 工作区的 `.hub/skills/`：跟着这个工作区走，可以放进版本库
/// - 用户目录的 `.agents/skills/`：与 ZCode / Claude 的技能目录共用
/// - 用户目录的 `.learning-hub/skills/`：本应用自己的
/// - 配置里额外指定的目录
pub fn discover(
    workspace_root: Option<&Path>,
    topic_dir: Option<&Path>,
    extra_dirs: &[String],
) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();

    // 主题自己的技能放最前面 → 同名时它胜出
    if let Some(topic) = topic_dir {
        scan_dir(&topic.join(crate::domain::topic::DIR_INTERNAL).join("skills"), "本主题", &mut out);
        scan_dir(&topic.join("skills"), "本主题", &mut out);
    }

    if let Some(root) = workspace_root {
        let dir = root
            .join(crate::domain::topic::DIR_INTERNAL)
            .join("skills");
        scan_dir(&dir, "工作区", &mut out);
    }

    if let Some(home) = home_dir() {
        scan_dir(&home.join(".agents").join("skills"), "用户技能", &mut out);
        scan_dir(
            &home.join(".learning-hub").join("skills"),
            "学习中枢",
            &mut out,
        );
    }

    for d in extra_dirs {
        let p = PathBuf::from(crate::paths::expand_home(d));
        scan_dir(&p, "自定义目录", &mut out);
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// 按名字/目录名/包含匹配找一个技能。
pub fn find<'a>(skills: &'a [Skill], needle: &str) -> Option<&'a Skill> {
    let n = needle.trim().to_lowercase();
    skills
        .iter()
        .find(|s| s.id.to_lowercase() == n || s.name.to_lowercase() == n)
        .or_else(|| {
            skills
                .iter()
                .find(|s| s.name.to_lowercase().contains(&n) || s.id.to_lowercase().contains(&n))
        })
}

/// 给系统提示词用的清单：只有「名字 + 什么时候用」。
pub fn catalog(skills: &[Skill], max_desc_chars: usize) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for s in skills {
        let desc: String = s.description.chars().take(max_desc_chars).collect();
        out.push_str(&format!("- {}（{}）：{}\n", s.name, s.id, desc));
    }
    out
}

/// 读技能正文（`skill_read` 工具用）。
pub fn read_body(skill: &Skill) -> AppResult<String> {
    let mut out = format!(
        "# 技能：{}\n（来自 {}）\n\n{}",
        skill.name, skill.dir, skill.body
    );
    if !skill.files.is_empty() {
        out.push_str("\n\n## 这个技能附带的文件（需要时用 fs_read 读，路径相对技能目录）\n");
        for f in &skill.files {
            out.push_str(&format!("- {f}\n"));
        }
        out.push_str("\n注意：这些文件不在主题目录里，读取时把完整路径拼上技能目录。\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let text = "---\nname: 文献检索\ndescription: 找论文、查 PMID 的时候用\n---\n\n## 怎么做\n1. 先拆关键词\n";
        let (name, desc, body) = parse_front_matter(text);
        assert_eq!(name.as_deref(), Some("文献检索"));
        assert_eq!(desc.as_deref(), Some("找论文、查 PMID 的时候用"));
        assert!(body.starts_with("## 怎么做"));
    }

    #[test]
    fn tolerates_missing_frontmatter() {
        let (name, desc, body) = parse_front_matter("# 标题\n正文");
        assert!(name.is_none() && desc.is_none());
        assert_eq!(body, "# 标题\n正文");
    }
}
