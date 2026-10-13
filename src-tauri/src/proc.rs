//! 起子进程时的公共约定。
//!
//! Windows 上，从 GUI 进程 spawn 一个**控制台程序**（powershell / cmd / npx / python…）时，
//! 系统会给它分配一个**新的控制台窗口**——用户看到的就是「闪一下的黑框」。
//! 加 `CREATE_NO_WINDOW` 才不弹；它只影响窗口，不影响 stdin/stdout（管道照旧）。
//!
//! 踩过的三处：MCP 服务器（大多是 `npx` / `uvx` 这类控制台程序）、远程面板枚举网卡的
//! PowerShell、用默认程序打开文件的 `cmd /C start`。**新加起进程的地方都要过这里**。
//!
//! 反例是更新器启动 NSIS 安装器（`updater::run_installer`）：那个要的是
//! `DETACHED_PROCESS`（脱离本应用的 job），自己设 flags，不走这里。

use std::process::Command;

/// Windows：让子进程不弹控制台窗口；别的平台上是空操作。
///
/// 注意 `creation_flags` 是**设置**而不是追加，所以别在同一个 Command 上再设一次别的 flags
/// （需要别的 flags 就自己拼，别调这里）。
pub fn no_console(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：加了这个标志之后命令照样跑得起来、还能拿到输出。
    /// （反过来的「有没有弹窗」在单测里看不见——那条只能靠人眼，所以这里守住「没把它弄坏」。）
    #[test]
    fn no_console_keeps_the_command_working() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo", "ok"]);
            c
        } else {
            let mut c = Command::new("echo");
            c.arg("ok");
            c
        };
        no_console(&mut cmd);
        let out = cmd.output().expect("子进程没跑起来");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
    }
}
