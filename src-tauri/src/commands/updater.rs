//! 一键更新：检测 GitHub release / 下载安装包（带进度事件）/ 分离启动安装器。
//!
//! 逻辑都在 `crate::updater`，这里只做参数搬运。事件走 `hub://update`，
//! 与既有事件协议互不影响。

use crate::error::AppResult;
use crate::state::AppState;
use crate::updater;
use tauri::State;

/// 检查 GitHub 上有没有比当前版本新的 release。
#[tauri::command]
pub async fn update_check(state: State<'_, AppState>) -> AppResult<updater::UpdateInfo> {
    let core = state.0.clone();
    updater::check(&core.http).await
}

/// 下载安装包到临时目录，期间发 `hub://update` 进度事件，返回落盘路径。
/// `total` 是检测时拿到的安装包大小（可缺，进度条会退化为「已下载 X MB」）。
#[tauri::command]
pub async fn update_download(
    state: State<'_, AppState>,
    url: String,
    total: Option<u64>,
) -> AppResult<String> {
    let core = state.0.clone();
    let app = core.app.clone();
    let path = updater::download(&app, &core.http, url, total).await?;
    Ok(path.to_string_lossy().to_string())
}

/// 分离启动安装器（静默安装 + 装完自动重启）。启动成功即返回，不等安装结束。
#[tauri::command]
pub async fn update_run(path: String) -> AppResult<()> {
    updater::run_installer(&path)
}
