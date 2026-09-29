//! 路径工具：目录名消毒、路径越界检查、相对/绝对路径转换。
//!
//! 安全模型：**一切文件访问都必须落在主题目录内**。
//! agent 拿到的路径一律是「相对主题目录的相对路径」，由 [`resolve_in_root`] 还原成绝对路径。

use crate::error::{AppError, AppResult};
use std::path::{Component, Path, PathBuf};

/// Windows 保留设备名，出现在目录名里会创建失败。
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 把用户输入的主题名变成合法（且尽量好看）的目录名。
pub fn sanitize_dir_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('_'),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    let out = out.trim().trim_end_matches('.').trim().to_string();
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out = if out.is_empty() { "未命名主题".to_string() } else { out };

    // 保留名加后缀，避免 Windows 直接拒绝创建
    let upper = out.to_ascii_uppercase();
    if RESERVED.iter().any(|r| upper == *r) {
        return format!("{out}_topic");
    }
    // 目录名过长会被文件系统截断，这里主动收敛
    if out.chars().count() > 64 {
        out.chars().take(64).collect()
    } else {
        out
    }
}

/// 词法归一化：消掉 `.`、`..`，不触碰磁盘（路径可能还不存在）。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 把「相对主题目录的路径」解析成绝对路径，并确保没有逃出 `root`。
///
/// - 允许 `notes/a.md`、`./a.md`
/// - 拒绝 `../x`、`C:\Windows`、空串
pub fn resolve_in_root(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let rel = rel.trim().replace('\\', "/");
    if rel.is_empty() {
        return Err(AppError::invalid("空路径"));
    }
    let raw = Path::new(&rel);
    if raw.is_absolute() {
        return Err(AppError::escape(rel));
    }

    let root_abs = normalize(&absolutize(root)?);
    let candidate = normalize(&root_abs.join(raw));
    if !candidate.starts_with(&root_abs) {
        return Err(AppError::escape(rel));
    }
    if candidate == root_abs {
        return Err(AppError::invalid("这是主题根目录，需要具体文件名"));
    }
    Ok(candidate)
}

/// 引用里的定位词：`materials/xx.pdf 第三章` 中的「第三章」不是文件名的一部分。
///
/// 出现的原因：模型引用资料时爱写「路径 + 章节」，而我们的引用格式只教了「第 N 页」。
/// 这条容错**只在原路径不存在时**才去掉尾巴，所以文件名真的以「第三讲」结尾也不受影响。
pub fn strip_locator_if_missing(root: &Path, rel: &str) -> String {
    let Ok(direct) = resolve_in_root(root, rel) else {
        return rel.trim().to_string();
    };
    if direct.exists() {
        return rel.trim().to_string();
    }
    match split_locator(rel) {
        Some((head, _)) => head.to_string(),
        None => rel.trim().to_string(),
    }
}

/// 把「materials/xx.pdf 第三章」拆成 `("materials/xx.pdf", "第三章")`；形状不对就返回 None。
fn split_locator(raw: &str) -> Option<(String, String)> {
    let t = raw.trim_end();
    let idx = t.rfind('第')?;
    let head = &t[..idx];
    let tail = &t[idx..];
    // 「第」前面要有空白：`第一讲.pdf` 这种是文件名，不能动
    if head.is_empty() || !head.ends_with(char::is_whitespace) {
        return None;
    }
    let unit = ["部分", "页", "章", "节", "讲", "篇", "段", "课"]
        .into_iter()
        .find(|u| tail.ends_with(u))?;
    let mid = &tail['第'.len_utf8()..tail.len() - unit.len()];
    let mid = mid.trim();
    // 中间只允许数字、中文数字、区间符号
    let ok = !mid.is_empty()
        && mid.chars().all(|c| {
            c.is_ascii_digit()
                || matches!(c, '-' | '~' | '～' | '、' | ',')
                || "零一二三四五六七八九十百两".contains(c)
        });
    if !ok {
        return None;
    }
    Some((head.trim_end().to_string(), tail.to_string()))
}

/// root 内部的相对路径（用于回传给 agent / 前端）。永不返回 `..`。
pub fn rel_in_root(root: &Path, path: &Path) -> String {
    let root_abs = normalize(&absolutize(root).unwrap_or_else(|_| root.to_path_buf()));
    let abs = normalize(&absolutize(path).unwrap_or_else(|_| path.to_path_buf()));
    match abs.strip_prefix(&root_abs) {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => abs.to_string_lossy().replace('\\', "/"),
    }
}

/// path 是否落在 root 之内（词法归一化后比较，不要求路径已存在）。
pub fn is_within(root: &Path, path: &Path) -> bool {
    let root_abs = normalize(&absolutize(root).unwrap_or_else(|_| root.to_path_buf()));
    let abs = normalize(&absolutize(path).unwrap_or_else(|_| path.to_path_buf()));
    abs.starts_with(&root_abs)
}

/// 判断字符串是不是绝对路径（含 Windows 盘符与 UNC）。
pub fn looks_absolute(raw: &str) -> bool {
    let s = raw.trim();
    if s.starts_with("\\\\") || s.starts_with("//") {
        return true;
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return true;
    }
    Path::new(s).is_absolute()
}

/// 展开开头的 `~` 为用户主目录。
pub fn expand_home(raw: &str) -> String {
    let s = raw.trim();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        if !home.is_empty() {
            return format!("{home}{}", &s[1..]);
        }
    }
    s.to_string()
}

/// 取一个路径的「可长期授权的根」：文件取父目录，目录取自身。
///
/// 批准粒度是目录而不是单个文件——不然读十份讲义就要点十次确认，
/// 用户会直接关掉沙箱，反而更危险。
pub fn grant_root(path: &Path) -> PathBuf {
    let abs = normalize(&absolutize(path).unwrap_or_else(|_| path.to_path_buf()));
    if abs.is_dir() {
        return abs;
    }
    match abs.parent() {
        Some(p) if p.parent().is_some() => p.to_path_buf(),
        _ => abs,
    }
}

/// 把相对路径变绝对；已经绝对的原样返回（再归一化）。
pub fn absolutize(path: &Path) -> AppResult<PathBuf> {
    if path.is_absolute() {
        return Ok(normalize(path));
    }
    let cwd = std::env::current_dir()?;
    Ok(normalize(&cwd.join(path)))
}

pub fn ensure_dir(path: &Path) -> AppResult<()> {
    std::fs::create_dir_all(path).map_err(|e| AppError::Io(std::io::Error::new(e.kind(), format!("{} — {e}", path.display()))))
}

/// 同目录唯一化：`a.md` 已存在时返回 `a-2.md`。
pub fn unique_child(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file_name.to_string(), String::new()),
    };
    for n in 2..10_000 {
        let c = dir.join(format!("{stem}-{n}{ext}"));
        if !c.exists() {
            return c;
        }
    }
    dir.join(format!("{stem}-{}{ext}", uuid::Uuid::new_v4()))
}

/// 人类可读的文件大小
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 引用尾巴上的「第 X 章/页/讲」要在文件不存在时才被去掉，
    /// 而文件名本来就叫「第一讲.pdf」的不能被误伤。
    #[test]
    fn strips_citation_locator_only_when_missing() {
        let tmp = std::env::temp_dir().join(format!("lh-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("materials")).unwrap();
        std::fs::write(tmp.join("materials/lecture.pdf"), b"x").unwrap();
        std::fs::write(tmp.join("materials/第一讲.pdf"), b"x").unwrap();

        // 存在 → 原样返回（哪怕尾巴长得像定位词）
        assert_eq!(
            strip_locator_if_missing(&tmp, "materials/第一讲.pdf"),
            "materials/第一讲.pdf"
        );
        // 不存在 + 章节定位 → 去掉定位词
        assert_eq!(
            strip_locator_if_missing(&tmp, "materials/lecture.pdf 第三章"),
            "materials/lecture.pdf"
        );
        assert_eq!(
            strip_locator_if_missing(&tmp, "materials/lecture.pdf 第 12-13 页"),
            "materials/lecture.pdf"
        );
        // 形状不像定位词（或路径本来就对）→ 不动
        assert_eq!(
            strip_locator_if_missing(&tmp, "materials/nope.pdf 讲义"),
            "materials/nope.pdf 讲义"
        );
        assert_eq!(strip_locator_if_missing(&tmp, "materials/ok.pdf"), "materials/ok.pdf");

        assert!(split_locator("notes/a.md 第三章").is_some());
        assert!(split_locator("notes/第一讲.md").is_none(), "文件名里的「第」不能在没空格时被切");
        assert!(split_locator("notes/a.md 第一章 PDF").is_none());

        std::fs::remove_dir_all(&tmp).ok();
    }

    fn root() -> PathBuf {
        PathBuf::from("C:/ws")
    }

    #[test]
    fn sanitize_keeps_chinese() {
        assert_eq!(sanitize_dir_name("线性代数 3:1"), "线性代数 3_1");
        assert_eq!(sanitize_dir_name("   "), "未命名主题");
        assert_eq!(sanitize_dir_name("con"), "con_topic");
    }

    #[test]
    fn blocks_escape() {
        assert!(resolve_in_root(&root(), "../x.md").is_err());
        assert!(resolve_in_root(&root(), "C:/Windows/x").is_err());
        assert!(resolve_in_root(&root(), "a/../../x").is_err());
    }

    #[test]
    fn allows_inside() {
        let p = resolve_in_root(&root(), "notes/a.md").unwrap();
        assert!(p.ends_with("notes/a.md") || p.ends_with("notes\\a.md"));
    }
}
