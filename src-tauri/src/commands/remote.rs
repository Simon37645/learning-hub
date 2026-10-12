//! 远程控制相关的命令（桌面端面板用）。
//!
//! 面板要的东西很少：现在是什么状态、开关、改端口/地址/对外地址、换令牌。
//! 所有**服务本身**的逻辑都在 `remote` 模块里，这里只做参数校验与状态回传。

use crate::error::{AppError, AppResult};
use crate::remote::RemoteStatus;
use crate::state::AppState;
use tauri::State;

/// 现在能不能用、地址是什么、连着几台设备。
#[tauri::command]
pub async fn remote_status(state: State<'_, AppState>) -> AppResult<RemoteStatus> {
    let core = state.0.clone();
    Ok(core.remote.status(&core))
}

/// 打开 / 关闭远程控制。
///
/// 打开时才监听端口（默认关闭是刻意的：这是一条能从别的设备操作本机的通道）。
/// 开关本身记进配置，下次启动自动恢复——用户既然开了，就是想让它在。
#[tauri::command]
pub async fn remote_set_enabled(state: State<'_, AppState>, enabled: bool) -> AppResult<RemoteStatus> {
    let core = state.0.clone();
    let core2 = core.clone();
    core.update_config(|c| {
        c.remote.enabled = enabled;
        c.remote.ensure_tokens();
    })?;
    if enabled {
        core.remote.start(core2)?;
    } else {
        core.remote.stop();
    }
    Ok(core.remote.status(&core))
}

/// 改端口 / 监听范围 / 对外地址 / 中继服务器。开着的话按新配置重启。
#[tauri::command]
pub async fn remote_config_patch(
    state: State<'_, AppState>,
    port: Option<u16>,
    bind: Option<String>,
    public_url: Option<String>,
    relay: Option<String>,
) -> AppResult<RemoteStatus> {
    let core = state.0.clone();
    if let Some(p) = port {
        // 0 会让系统随机挑端口，用户会看不懂；<1024 在 Windows 上要管理员权限
        if p < 1024 {
            return Err(AppError::invalid("端口请填 1024 以上的（1024 以下需要管理员权限）"));
        }
    }
    let bind_parsed = match bind.as_deref() {
        None => None,
        Some("lan") => Some(crate::config::RemoteBind::Lan),
        Some("local") => Some(crate::config::RemoteBind::Local),
        Some(other) => return Err(AppError::invalid(format!("认不出的监听范围：{other}"))),
    };
    let core2 = core.clone();
    let cfg = core.update_config(|c| {
        if let Some(p) = port {
            c.remote.port = p;
        }
        if let Some(b) = bind_parsed {
            c.remote.bind = b;
        }
        if let Some(u) = public_url.as_ref() {
            c.remote.public_url = u.trim().to_string();
        }
        if let Some(r) = relay.as_ref() {
            c.remote.relay = r.trim().to_string();
        }
    })?;
    if cfg.remote.enabled {
        // 配置变了要按新参数重开（端口/监听地址是绑定时就定下的）
        core.remote.restart(core2)?;
    }
    Ok(core.remote.status(&core))
}

/// 桌面端渲染完一张图之后回话（`hub://remote` 事件的对应动作）。
///
/// **失败也要回**：不回的话手机那边要等到 10 秒超时，用户盯着一个不动的占位。
#[tauri::command]
pub async fn remote_report_render(
    state: State<'_, AppState>,
    id: String,
    ok: bool,
    payload: String,
) -> AppResult<bool> {
    Ok(state.0.remote.render.finish(
        &id,
        crate::remote::render::RenderOutcome { ok, payload },
    ))
}

/// 换一个访问令牌。旧链接与旧二维码立刻失效（手机上那台会被踢下线）。
#[tauri::command]
pub async fn remote_token_reset(
    state: State<'_, AppState>,
    which: Option<String>,
) -> AppResult<RemoteStatus> {
    let core = state.0.clone();
    let core2 = core.clone();
    let target = which.unwrap_or_else(|| "access".to_string());
    let cfg = core.update_config(|c| {
        match target.as_str() {
            "relay" => c.remote.relay_token = crate::config::RemoteConfig::fresh_token(),
            _ => c.remote.token = crate::config::RemoteConfig::fresh_token(),
        }
    })?;
    if cfg.remote.enabled {
        core.remote.restart(core2)?;
    }
    Ok(core.remote.status(&core))
}
