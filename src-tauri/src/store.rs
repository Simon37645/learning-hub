//! 存储层：原子写、JSON / JSONL 读写、目录扫描。
//!
//! 设计原则：**磁盘上的文件就是唯一真相**。没有数据库、没有隐藏索引，
//! 用户随时可以用编辑器直接改学习资料，应用下次扫描就能看到。

use crate::error::{AppError, AppResult, IoContext};
use serde::{de::DeserializeOwned, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 原子写：先写同目录临时文件再 rename，避免半截文件。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(parent.display())?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default()
    ));
    {
        let mut f = std::fs::File::create(&tmp).ctx(tmp.display())?;
        f.write_all(bytes).ctx(tmp.display())?;
        f.sync_all().ctx(tmp.display())?;
    }
    // Windows 上 rename 覆盖已存在文件是允许的（std 内部用 MoveFileEx）
    std::fs::rename(&tmp, path).ctx(path.display())?;
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let text = serde_json::to_string_pretty(value)?;
    atomic_write(path, text.as_bytes())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    let text = std::fs::read_to_string(path).ctx(path.display())?;
    Ok(serde_json::from_str(&text)?)
}

/// 文件不存在时返回 `None`，其余错误照常抛出。
pub fn read_json_opt<T: DeserializeOwned>(path: &Path) -> AppResult<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

pub fn read_text(path: &Path) -> AppResult<String> {
    std::fs::read_to_string(path).ctx(path.display())
}

pub fn read_text_opt(path: &Path) -> AppResult<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    read_text(path).map(Some)
}

/// JSONL：一行一条记录，天然支持追加，方便人肉查看与 diff。
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> AppResult<Vec<T>> {
    let Some(text) = read_text_opt(path)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(v) => out.push(v),
            // 单行损坏不应该毁掉整个文件，跳过并继续
            Err(e) => eprintln!("[store] 跳过损坏行 {}:{} — {e}", path.display(), i + 1),
        }
    }
    Ok(out)
}

pub fn write_jsonl<T: Serialize>(path: &Path, items: &[T]) -> AppResult<()> {
    let mut buf = String::new();
    for it in items {
        buf.push_str(&serde_json::to_string(it)?);
        buf.push('\n');
    }
    atomic_write(path, buf.as_bytes())
}

pub fn append_jsonl<T: Serialize>(path: &Path, item: &T) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(parent.display())?;
    }
    let mut line = serde_json::to_string(item)?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ctx(path.display())?;
    f.write_all(line.as_bytes()).ctx(path.display())?;
    Ok(())
}

/// 文件是否是可读文本（供查看器/检索判断，不让二进制撑爆上下文）。
pub fn is_texty(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("md" | "markdown" | "txt" | "json" | "jsonl" | "csv" | "tsv" | "yaml" | "yml" | "toml"
            | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "c" | "h" | "cpp" | "hpp" | "java" | "go"
            | "sh" | "html" | "htm" | "css" | "tex" | "bib" | "srt" | "log")
    )
}

/// 递归列目录，返回相对 `root` 的路径；跳过点开头目录（.hub 等内部状态）。
pub fn walk_files(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && name != "node_modules"
        });
    for entry in walker.flatten() {
        if entry.file_type().is_file() {
            out.push(entry.path().to_path_buf());
        }
    }
    out
}

/// 文件修改时间（RFC3339）；拿不到就用 epoch。
pub fn modified_at(path: &Path) -> chrono::DateTime<chrono::Utc> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or_else(|_| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
}

pub fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// 读文件时做一次大小护栏，防止把几百 MB 的 log 塞进模型上下文。
pub fn read_text_capped(path: &Path, max_bytes: u64) -> AppResult<String> {
    let meta = std::fs::metadata(path).ctx(path.display())?;
    if meta.len() > max_bytes {
        let text = std::fs::read_to_string(path).ctx(path.display())?;
        let mut cut = String::new();
        for ch in text.chars() {
            if cut.len() + ch.len_utf8() > max_bytes as usize {
                break;
            }
            cut.push(ch);
        }
        cut.push_str("\n\n…（文件过长已截断）");
        return Ok(cut);
    }
    read_text(path)
}

/// 供 UI 展示的错误包装：把底层 io 错误转成「路径 + 原因」。
pub fn io_err(path: &Path, e: std::io::Error) -> AppError {
    AppError::Io(std::io::Error::new(e.kind(), format!("{} — {e}", path.display())))
}

/// 不直接删除，而是移进回收目录。
///
/// 学习资料是用户长期积累的东西，误删的代价远大于多留一份。
/// 返回移动后的新路径。
pub fn move_to_trash(trash_root: &Path, path: &Path) -> AppResult<PathBuf> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    ensure_dir(trash_root)?;
    let dest = trash_root.join(format!("{name}-{stamp}"));
    let dest = if dest.exists() {
        trash_root.join(format!("{name}-{stamp}-{}", uuid::Uuid::new_v4().simple()))
    } else {
        dest
    };
    std::fs::rename(path, &dest).map_err(|e| {
        AppError::Io(std::io::Error::new(
            e.kind(),
            format!("无法移动到回收站 {} → {}：{e}", path.display(), dest.display()),
        ))
    })?;
    Ok(dest)
}

pub fn ensure_dir(path: &Path) -> AppResult<()> {
    std::fs::create_dir_all(path).ctx(path.display())
}
