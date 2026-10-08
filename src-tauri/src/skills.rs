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
///
/// 公开是给工坊用的：发布技能后要从**正式目录**回读一遍，
/// 让「description 没写对」在发布那一步就暴露出来，而不是等下次对话才发现描述是空的。
pub fn parse_front_matter(text: &str) -> (Option<String>, Option<String>, String) {
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
/// - 子主题自己的 `skills/`：优先级最高
/// - 父主题的 `skills/`：同一门课共用的技能（子主题看得见）
/// - 工作区的 `.hub/skills/`：跟着这个工作区走，可以放进版本库
/// - 用户目录的 `.agents/skills/`：与 ZCode / Claude 的技能目录共用
/// - 用户目录的 `.learning-hub/skills/`：本应用自己的
/// - 配置里额外指定的目录
pub fn discover(
    workspace_root: Option<&Path>,
    topic_dirs: &[PathBuf],
    extra_dirs: &[String],
) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();

    // 主题自己的技能放最前面 → 同名时它胜出；父主题的排在其后
    for dir in topic_dirs {
        scan_dir(&dir.join(crate::domain::topic::DIR_INTERNAL).join("skills"), "本主题", &mut out);
        scan_dir(&dir.join("skills"), "本主题", &mut out);
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

/// 技能生效规则（纯函数，好测也好读）：**只给开着的技能**。
///
/// 判断不区分技能来自哪个目录——工作区、用户目录、自定义目录一视同仁。
/// 用户以后往哪儿加技能，规则都成立（没有按名字或目录写死的特例）。
/// 调用方负责把「全局禁用 + 本主题禁用 + 父主题禁用」并成 `disabled`。
pub fn effective(all: Vec<Skill>, skills_enabled: bool, disabled: &[String]) -> Vec<Skill> {
    if !skills_enabled {
        return Vec::new();
    }
    all.into_iter()
        .filter(|s| !disabled.iter().any(|d| d == &s.id))
        .collect()
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

    /// 只给开着的技能：被禁用的不给（不管它来自哪个目录），总开关关了一个都不给。
    #[test]
    fn effective_respects_switches() {
        let mk = |id: &str| Skill {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            dir: String::new(),
            body: String::new(),
            files: Vec::new(),
            source: "测试".into(),
        };
        let all = vec![mk("skill-a"), mk("skill-b")];
        let ids = |v: Vec<Skill>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();

        // 没关就都给
        assert_eq!(ids(effective(all.clone(), true, &[])), vec!["skill-a", "skill-b"]);
        // 关掉哪个就不给哪个
        assert_eq!(
            ids(effective(all.clone(), true, &["skill-a".to_string()])),
            vec!["skill-b"]
        );
        // 两个都关就都不给
        assert!(effective(
            all.clone(),
            true,
            &["skill-a".to_string(), "skill-b".to_string()]
        )
        .is_empty());
        // 总开关关掉：一个都不给
        assert!(effective(all, false, &[]).is_empty());
    }
    /// 子主题能看见父主题私有目录里的技能；同名时子主题自己的那份胜出。
    #[test]
    fn discovers_parent_topic_skills() {
        let tmp = std::env::temp_dir().join(format!("lh-skills-{}", uuid::Uuid::new_v4()));
        let parent = tmp.join("线性代数");
        let child = tmp.join("第三章 特征值");
        for (dir, body) in [
            (parent.join("skills/文献检索"), "父主题版本"),
            (child.join("skills/文献检索"), "子主题版本"),
            (parent.join("skills/画图"), "父主题独有"),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(SKILL_FILE), format!("---\nname: 测试\n---\n\n{body}\n")).unwrap();
        }

        // 只断言这两个测试技能：discover 还会扫用户目录，结果里会有本机装的其它技能
        let found: Vec<Skill> = discover(None, &[child.clone(), parent.clone()], &[])
            .into_iter()
            .filter(|s| s.id == "文献检索" || s.id == "画图")
            .collect();
        assert_eq!(found.len(), 2, "同名技能应当只出现一次");
        let shared = found.iter().find(|s| s.id == "文献检索").unwrap();
        assert!(shared.body.contains("子主题版本"), "同名时子主题自己的技能优先");

        std::fs::remove_dir_all(&tmp).ok();
    }

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
