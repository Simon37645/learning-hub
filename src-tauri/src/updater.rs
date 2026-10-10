//! 一键更新：检测 GitHub release → 下载安装包 → 分离启动安装器。
//!
//! **为什么不用 tauri-plugin-updater**：官方插件要求 minisign 签名 + 固定 endpoint
//! 的 latest.json，而且预发布（prerelease）根本不出现在 `/releases/latest` 别名里——
//! 与我们「β 满天飞、`gh release` 手动发」的流程对不上。自研这三步不需要签名基建：
//! 来源是自己的 GitHub 仓库，全程 HTTPS，安装包是 NSIS 静默安装（`/S` 装完 `/R`
//! 自动重启应用，「应用还在运行」由安装器自己处理）。
//!
//! 节流提醒：GitHub API 无认证限额 60 次/小时，所以**只响应用户点的按钮**，
//! 不做任何启动时或定时的自动检查。
//!
//! 结构：`check` / `download` 依赖网络与 AppHandle；「哪个版本更新、挑哪个安装包」
//! 是纯逻辑，拆在 [`parse_tag`] / [`parse_releases`] / [`choose_update`] /
//! [`installer_asset_name`]，可以脱离 Tauri 与网络单测。

use crate::error::{AppError, AppResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

/// 我们的 release 仓库（列表接口，含 prerelease；draft 也可能出现在最前面）
const RELEASES_URL: &str = "https://api.github.com/repos/Simon37645/learning-hub/releases?per_page=10";

/// 下载进度事件的发射步长：每收 ~512KB 发一次，别把事件通道刷爆
const EMIT_STEP: u64 = 512 * 1024;

/// `hub://update`：下载进度 `{kind:"progress", received, total}`。
/// total 可能为 null（重定向后拿不到 Content-Length）。
pub const EVENT_UPDATE: &str = "hub://update";

/// 一次检查的结果。`available: false` 表示「没有可装的更新」，
/// 包括当前版本解析失败这种边界——界面只需要显示「已是最新」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub available: bool,
    /// 新版本号（不带前导 v）
    pub version: Option<String>,
    /// release body，截断到 4000 字符
    pub notes: Option<String>,
    /// 安装包的 browser_download_url
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
    /// 发布时间（GitHub 原样的 ISO 串，给界面显示「发布于 …」）
    pub published_at: Option<String>,
}

// ------------------------------------------------------------ 纯逻辑（可单测）

/// `v0.3.2-beta.1` → `0.3.2-beta.1`。前导 v 大小写都容；解析不了就 None
/// （tag 写坏了的 release 不该挡住别的版本）。
fn parse_tag(tag: &str) -> Option<semver::Version> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    semver::Version::parse(t).ok()
}

/// 安装包在 release assets 里的文件名。必须与 scripts/package.mjs 产出的
/// `LearningHub-<版本>-x64-setup.exe` 一致——那里剥非 ASCII 就是为了这个。
fn installer_asset_name(version: &semver::Version) -> String {
    format!("LearningHub-{version}-x64-setup.exe")
}

/// GitHub release 的宽容解析中间形态：只留我们关心的字段。
#[derive(Debug, Clone)]
pub struct RawRelease {
    pub tag_name: String,
    pub draft: bool,
    pub notes: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<RawAsset>,
}

#[derive(Debug, Clone)]
pub struct RawAsset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
}

/// 从 GitHub 的 JSON 里抢救 release 列表。**解析必须宽容**：缺字段、类型不对的
/// 条目直接跳过——API 偶尔改个结构，不该让「检查更新」整个报错。
fn parse_releases(value: &serde_json::Value) -> AppResult<Vec<RawRelease>> {
    let items = value
        .as_array()
        .ok_or_else(|| AppError::other("检查更新失败（GitHub 返回的不是版本列表）"))?;
    let mut out = Vec::new();
    for item in items {
        let Some(tag) = item.get("tag_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let draft = item.get("draft").and_then(|v| v.as_bool()).unwrap_or(false);
        let notes = item
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(4000).collect::<String>());
        let assets = item
            .get("assets")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        Some(RawAsset {
                            name: a.get("name")?.as_str()?.to_string(),
                            url: a.get("browser_download_url")?.as_str()?.to_string(),
                            size: a.get("size").and_then(|s| s.as_u64()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.push(RawRelease {
            tag_name: tag.to_string(),
            draft,
            notes,
            published_at: item
                .get("published_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            assets,
        });
    }
    Ok(out)
}

/// 在乱序的 release 列表里挑「比 current 新、且有安装包」的最大版本。
/// 没有安装包的版本跳过（一个 release 可能只有源码包），但不挡住更旧的可用版本。
fn choose_update(current: &semver::Version, releases: &[RawRelease]) -> Option<(semver::Version, RawRelease)> {
    let mut candidates: Vec<(semver::Version, &RawRelease)> = releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_tag(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| v > current)
        .collect();
    // 从最大往回找第一个带安装包的：β 链上偶尔混进一个没传附件的 release
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates
        .iter()
        .rev()
        .find(|(v, r)| r.assets.iter().any(|a| asset_is_installer(a, v)))
        .map(|(v, r)| (v.clone(), (*r).clone()))
}

fn asset_is_installer(asset: &RawAsset, version: &semver::Version) -> bool {
    asset.name.eq_ignore_ascii_case(&installer_asset_name(version))
}

// ------------------------------------------------------------ 网络三步

/// 第一步：问 GitHub 有没有比当前版本新的 release（含 β 预发布，跳过 draft）。
pub async fn check(http: &reqwest::Client) -> AppResult<UpdateInfo> {
    let current_str = env!("CARGO_PKG_VERSION").to_string();
    let current = semver::Version::parse(&current_str).ok();

    // GitHub API 必须带 UA；这里覆盖共享客户端的浏览器 UA，报自己的名字
    let resp = http
        .get(RELEASES_URL)
        .header(reqwest::header::USER_AGENT, "learning-hub-updater")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let hint = if status.as_u16() == 403 || status.as_u16() == 429 {
            "可能是接口限流（无认证每小时 60 次），稍后再试"
        } else if status.as_u16() == 404 {
            "仓库不存在或已改名"
        } else {
            "网络不通或 GitHub 暂时不可用"
        };
        return Err(AppError::other(format!("检查更新失败（GitHub 返回 {status}，{hint}）")));
    }
    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::other(format!("检查更新失败（返回内容无法解析：{e}）")))?;
    let releases = parse_releases(&value)?;

    let no_update = UpdateInfo {
        current: current_str.clone(),
        available: false,
        version: None,
        notes: None,
        asset_url: None,
        asset_size: None,
        published_at: None,
    };
    let Some(current) = current else {
        // 本地版本号本身解析不了（开发期改坏的版本串之类），没法比较，保守报「无更新」
        return Ok(no_update);
    };
    let Some((version, release)) = choose_update(&current, &releases) else {
        return Ok(no_update);
    };
    let asset = release
        .assets
        .iter()
        .find(|a| asset_is_installer(a, &version))
        .cloned();
    Ok(UpdateInfo {
        current: current_str,
        available: true,
        version: Some(version.to_string()),
        notes: release.notes,
        asset_url: asset.as_ref().map(|a| a.url.clone()),
        asset_size: asset.as_ref().and_then(|a| a.size),
        published_at: release.published_at,
    })
}

/// 第二步：流式下载安装包到 `%TEMP%\learning-hub-update-<文件名>`，
/// 每收 ~512KB 发一次进度事件。GitHub 的 browser_download_url 会有
/// Content-Length，但被重定向/代理剥头之后就没了——未知就发 `total: null`。
pub async fn download(
    app: &AppHandle,
    http: &reqwest::Client,
    url: String,
    total_hint: Option<u64>,
) -> AppResult<std::path::PathBuf> {
    let filename = url_filename(&url)
        .ok_or_else(|| AppError::invalid(format!("下载地址里认不出文件名：{url}")))?;
    let target = std::env::temp_dir().join(format!("learning-hub-update-{filename}"));
    // 上一次装到一半的残留没有复用价值，直接删掉重来
    let _ = std::fs::remove_file(&target);

    let resp = http.get(&url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::other(format!("下载更新失败（服务器返回 {status}）")));
    }
    let total = total_hint.or(resp.content_length());

    let mut file = tokio::fs::File::create(&target).await?;
    let mut stream = resp;
    let mut received: u64 = 0;
    let mut last_emitted: u64 = 0;
    while let Some(chunk) = stream.chunk().await? {
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        if received - last_emitted >= EMIT_STEP {
            last_emitted = received;
            emit_progress(app, received, total);
        }
    }
    file.flush().await?;
    drop(file);

    // 长度对不上就不敢交给安装器：半截 exe 跑起来的报错没人看得懂
    if let Some(t) = total {
        let actual = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        if actual != t {
            let _ = std::fs::remove_file(&target);
            return Err(AppError::other(format!(
                "下载不完整（应有 {t} 字节，实际只有 {actual} 字节），已删除残留文件，请稍后再试"
            )));
        }
    }
    emit_progress(app, received, total);
    Ok(target)
}

fn emit_progress(app: &AppHandle, received: u64, total: Option<u64>) {
    let _ = app.emit(EVENT_UPDATE, serde_json::json!({
        "kind": "progress",
        "received": received,
        "total": total,
    }));
}

/// 从 URL 尾段取文件名（剥掉查询串）。
fn url_filename(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .last()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// 第三步：分离启动 NSIS 安装器。`/S` 静默安装、`/R` 装完自动重启应用；
/// spawn 后不等待，DETACHED 下子进程独立于本应用存活。
#[cfg(windows)]
pub fn run_installer(path: impl AsRef<std::path::Path>) -> AppResult<()> {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let path = path.as_ref();
    if !path.is_file() {
        return Err(AppError::NotFound(format!(
            "安装包不存在：{}",
            path.display()
        )));
    }

    // breakaway 让安装器不受本应用退出（job 对象回收）影响；
    // 有的宿主环境不让脱离 job，那种失败就去掉这一位重试——
    // 代价只是「安装器可能跟着应用一起被回收」，但应用马上就要退了，通常无感。
    let try_spawn = |flags: u32| -> std::io::Result<()> {
        let mut cmd = std::process::Command::new(path);
        cmd.args(["/S", "/R"]).creation_flags(flags);
        cmd.spawn().map(|_| ())
    };
    try_spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| try_spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP))
        .map_err(|e| AppError::other(format!("启动安装程序失败：{e}")))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn run_installer(_path: impl AsRef<std::path::Path>) -> AppResult<()> {
    Err(AppError::other("一键更新目前只在 Windows 上可用"))
}

// ------------------------------------------------------------ 单测

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, draft: bool, assets: &[&str]) -> RawRelease {
        RawRelease {
            tag_name: tag.to_string(),
            draft,
            notes: Some(format!("{tag} 的更新说明")),
            published_at: Some("2026-01-01T00:00:00Z".into()),
            assets: assets
                .iter()
                .map(|name| RawAsset {
                    name: name.to_string(),
                    url: format!("https://example.com/{name}"),
                    size: Some(1024),
                })
                .collect(),
        }
    }

    #[test]
    fn tag_parsing() {
        assert_eq!(
            parse_tag("v0.3.2-beta.1").map(|v| v.to_string()),
            Some("0.3.2-beta.1".into())
        );
        assert_eq!(parse_tag("0.4.0").map(|v| v.to_string()), Some("0.4.0".into()));
        // 不是 semver 的 tag 一律不要
        assert_eq!(parse_tag("v1.0"), None);
        assert_eq!(parse_tag("nightly-2026"), None);
        assert_eq!(parse_tag(""), None);
    }

    #[test]
    fn prerelease_orders_below_release() {
        // semver 原生语义：同版本号下 prerelease < 正式版
        let beta = parse_tag("0.3.2-beta.1").unwrap();
        let stable = parse_tag("0.3.2").unwrap();
        assert!(beta < stable);
    }

    #[test]
    fn picks_max_from_unordered_releases() {
        let current = semver::Version::parse("0.3.1").unwrap();
        let releases = vec![
            rel("v0.3.2", false, &["LearningHub-0.3.2-x64-setup.exe"]),
            rel("v0.4.0-beta.1", false, &["LearningHub-0.4.0-beta.1-x64-setup.exe"]),
            rel("v0.3.1", false, &["LearningHub-0.3.1-x64-setup.exe"]),
        ];
        let (v, _) = choose_update(&current, &releases).unwrap();
        assert_eq!(v.to_string(), "0.4.0-beta.1");
    }

    #[test]
    fn skips_drafts() {
        let current = semver::Version::parse("0.3.1").unwrap();
        let releases = vec![
            rel("v9.9.9", true, &["LearningHub-9.9.9-x64-setup.exe"]),
            rel("v0.3.2", false, &["LearningHub-0.3.2-x64-setup.exe"]),
        ];
        let (v, _) = choose_update(&current, &releases).unwrap();
        assert_eq!(v.to_string(), "0.3.2");
    }

    #[test]
    fn skips_release_without_installer_asset() {
        let current = semver::Version::parse("0.3.1").unwrap();
        let releases = vec![
            // 最新版只传了源码包，没有安装包：退而取次新的可用版本
            rel("v0.4.0", false, &["source.zip"]),
            rel("v0.3.2", false, &["LearningHub-0.3.2-x64-setup.exe"]),
        ];
        let (v, r) = choose_update(&current, &releases).unwrap();
        assert_eq!(v.to_string(), "0.3.2");
        assert!(r.assets.iter().any(|a| asset_is_installer(a, &v)));
    }

    #[test]
    fn nothing_newer_means_none() {
        let current = semver::Version::parse("0.3.2").unwrap();
        // 0.3.2-beta.1 比 0.3.2 旧，不该被当成更新
        let releases = vec![rel("v0.3.2-beta.1", false, &["LearningHub-0.3.2-beta.1-x64-setup.exe"])];
        assert!(choose_update(&current, &releases).is_none());
    }

    #[test]
    fn tolerant_json_parsing() {
        let value: serde_json::Value = serde_json::from_str(
            r#"[
                {"tag_name": "v0.4.0", "draft": false,
                 "assets": [{"name": "LearningHub-0.4.0-x64-setup.exe",
                             "browser_download_url": "https://x/1.exe", "size": 5}]},
                {"draft": false},
                {"tag_name": "v0.5.0", "assets": [{"name": "缺 url"}]},
                "not-an-object"
            ]"#,
        )
        .unwrap();
        let releases = parse_releases(&value).unwrap();
        // 有 tag_name 的条目都收进来；缺 tag 的跳过，坏 asset 只丢自己
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag_name, "v0.4.0");
        assert_eq!(releases[0].assets.len(), 1);
        assert_eq!(releases[0].assets[0].size, Some(5));
        assert!(releases[1].assets.is_empty());
    }

    #[test]
    fn filename_from_url_tail() {
        assert_eq!(
            url_filename("https://github.com/x/releases/download/v0.4.0/LearningHub-0.4.0-x64-setup.exe"),
            Some("LearningHub-0.4.0-x64-setup.exe".into())
        );
        assert_eq!(url_filename("https://x/a/b?query=1"), Some("b".into()));
        assert_eq!(url_filename("not a url"), None);
    }
}
