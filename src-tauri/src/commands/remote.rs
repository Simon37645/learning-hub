//! 远程控制相关的命令（桌面端面板用）。
//!
//! 面板要的东西很少：现在是什么状态、开关、改端口/地址/对外地址、换令牌。
//! 所有**服务本身**的逻辑都在 `remote` 模块里，这里只做参数校验与状态回传。

use crate::error::{AppError, AppResult};
use crate::remote::RemoteStatus;
use crate::state::AppState;
use tauri::{Emitter, State};

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

/// 一键把中继装到用户自己的服务器上（IP + 用户名 + 密码 → 开箱即用）。
///
/// 做四件事：SSH 连上去把中继装好并起常驻服务 → 把「中继服务器」填进配置 →
/// 重开隧道 → 返回全过程日志（面板上逐行显示）。
///
/// **密码只用这一次，不写进配置**：日常使用靠的是中继口令（已经在配置里），
/// 重新安装才需要再输一次密码。把 root 密码落盘换来的那点方便不值。
#[tauri::command]
pub async fn remote_deploy_relay(
    state: State<'_, AppState>,
    host: String,
    ssh_port: Option<u16>,
    user: String,
    password: String,
) -> AppResult<crate::remote::deploy::DeployReport> {
    let core = state.0.clone();
    let cfg = core.config_read().remote;
    let port = cfg.port;
    let token = cfg.relay_token.clone();
    if token.trim().is_empty() {
        return Err(AppError::invalid("还没有中继口令，先点一下「换」生成一个"));
    }
    let target = crate::remote::deploy::SshTarget {
        host: host.trim().to_string(),
        port: ssh_port.unwrap_or(22),
        user: user.trim().to_string(),
        password,
    };

    // 进度实时推给面板（一次安装可能几十秒，中间不能干等）
    let app = core.app.clone();
    let progress = move |line: &str| {
        let _ = app.emit(
            crate::agent::event::EVENT_REMOTE_DEPLOY,
            crate::agent::event::RemoteDeployEvent {
                stage: "running".into(),
                line: line.to_string(),
            },
        );
    };

    let expected = core.config_read().remote.host_key;
    let report = crate::remote::deploy::deploy(
        &target,
        port,
        &token,
        if expected.trim().is_empty() { None } else { Some(expected.as_str()) },
        progress,
    )
    .await;

    // 连都没连上（密码错、端口不通）：把这句话也推进面板，别让用户盯着空白
    let report = match report {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            let _ = core.app.emit(
                crate::agent::event::EVENT_REMOTE_DEPLOY,
                crate::agent::event::RemoteDeployEvent { stage: "failed".into(), line: msg.clone() },
            );
            return Err(e);
        }
    };

    // 记下服务器信息（不含密码）与公钥指纹，下次预填、也做 TOFU 校验
    let core2 = core.clone();
    let relay = format!("{}:{}", target.host, port);
    let host_key = report.host_key.clone();
    let ssh_user = target.user.clone();
    let ssh_port = target.port;
    core.update_config(|c| {
        c.remote.ssh_user = ssh_user.clone();
        c.remote.ssh_port = ssh_port;
        c.remote.host_key = host_key.clone();
        if report.ok {
            c.remote.relay = relay.clone();
            c.remote.relay_token = token.clone();
            c.remote.enabled = true;
        }
    })?;
    if report.ok && core.config_read().remote.enabled {
        // 配置变了 + 隧道要连新地址：重启一次（幂等，失败也只在面板上报错）
        core.remote.restart(core2)?;
    }

    let _ = core.app.emit(
        crate::agent::event::EVENT_REMOTE_DEPLOY,
        crate::agent::event::RemoteDeployEvent {
            stage: if report.ok { "done".into() } else { "failed".into() },
            line: if report.ok {
                "安装完成，隧道已接上".into()
            } else {
                report.error.clone().unwrap_or_else(|| "安装失败".into())
            },
        },
    );
    Ok(report)
}

/// 忘掉记住的服务器公钥指纹（换了机器 / 重装了系统时用）。
#[tauri::command]
pub async fn remote_forget_host_key(state: State<'_, AppState>) -> AppResult<()> {
    state.0.update_config(|c| c.remote.host_key.clear())?;
    Ok(())
}

/// 桌面端前端执行完一条远程命令之后回话（`hub://remote-invoke` 的对应动作）。
///
/// **失败也要回**：不回的话手机那边要等到 30 秒超时，用户盯着一个不动的界面。
/// `payload` 就是 `invoke` 的返回值（失败时是错误文案），原样转给手机。
#[tauri::command]
pub async fn remote_report_invoke(
    state: State<'_, AppState>,
    id: String,
    ok: bool,
    payload: serde_json::Value,
) -> AppResult<bool> {
    Ok(state
        .0
        .remote
        .bridge
        .finish(&id, crate::remote::bridge::Outcome { ok, payload }))
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
