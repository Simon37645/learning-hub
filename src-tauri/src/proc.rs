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

/// 把配置/工具里写的命令解析成「真能起起来的」命令 + 参数。
///
/// Windows 上 `npx` / `npm` / `uvx` 这类命令其实是 `npx.cmd` 这样的 **shim**，
/// 而 `CreateProcess` **不认 PATHEXT**（Rust 的 `Command::new("npx")` 只会去找
/// `npx.exe`）——于是「MCP 服务器起不来」，报错还未必说得清。这里自己按
/// PATH + PATHEXT 找一遍；找出来是 `.cmd` / `.bat` 就套一层 `cmd /C` 起它。
///
/// 找不到就原样返回（让 spawn 去报错——那条错误会带上命令原文，见调用方）。
pub fn resolve_command(command: &str, args: &[String]) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        // 带路径分隔符的（含绝对路径）交给系统按原样解析：那是调用方明确指定的位置
        if !command.contains(['/', '\\']) {
            if let Some(found) = find_on_path(command) {
                let lower = found.to_ascii_lowercase();
                if lower.ends_with(".cmd") || lower.ends_with(".bat") {
                    let mut all = vec!["/C".to_string(), found];
                    all.extend(args.iter().cloned());
                    return ("cmd.exe".to_string(), all);
                }
                return (found, args.to_vec());
            }
        }
    }
    (command.to_string(), args.to_vec())
}

/// 字节 → 文本，给「命令输出」用。
///
/// Windows 控制台程序（cmd 内建命令、PowerShell 5.1、Python…）往管道里写的是**本地代码页**
/// 的字节而不是 UTF-8：中文机器上是 GBK，按 UTF-8 硬解就成了一串替换字符
/// （症状：agent 跑 `dir` 或带中文输出的脚本，拿回来的内容是乱码）。
/// 先严格试 UTF-8，失败才按 GBK 兜底——GBK 兼容 ASCII，纯英文输出两种解出来一模一样，
/// 所以这条兜底不会把正常的英文输出弄坏。跟 `net::decode_html` 的兜底是同一个路子。
pub fn decode_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::GBK.decode(bytes);
            text.into_owned()
        }
    }
}

/// 按 PATH + PATHEXT 找一个可执行文件（Windows 专用；别的平台交给系统）。
#[cfg(windows)]
fn find_on_path(command: &str) -> Option<String> {
    use std::path::Path;
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(';').filter(|d| !d.trim().is_empty()) {
        let dir = Path::new(dir.trim());
        // 原样（可能本来就带了扩展名）
        let as_is = dir.join(command);
        if as_is.is_file() {
            return Some(as_is.to_string_lossy().to_string());
        }
        for ext in &exts {
            let cand = dir.join(format!("{command}{ext}"));
            if cand.is_file() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 一句话说明「这个命令我们解析到了哪里」——报错时带上它，用户/agent 一眼能看出
/// 是「没装」还是「装了但不在应用的 PATH 上」。
pub fn describe_command(command: &str) -> String {
    #[cfg(windows)]
    {
        if !command.contains(['/', '\\']) {
            return match find_on_path(command) {
                Some(p) => format!("{command} → {p}"),
                None => format!("{command}（在 PATH 里没找到）"),
            };
        }
    }
    command.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：加了这个标志之后命令照样跑得起来、还能拿到输出。
    /// （反过来的「有没有弹窗」在单测里看不见——那条只能靠人眼，所以这里守住「没把它弄坏」。）
    #[test]
    fn no_console_keeps_the_command_working() {        let mut cmd = if cfg!(windows) {
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

    /// 命令输出的解码：UTF-8 原样、GBK 兜底、纯英文两种都一样。
    #[test]
    fn decode_output_handles_utf8_and_local_codepage() {
        assert_eq!(decode_output(b"hello"), "hello");
        assert_eq!(decode_output("中文 utf8".as_bytes()), "中文 utf8");
        // cmd / PowerShell 5.1 在中文 Windows 上写出来的就是这四对字节
        let gbk = [0xd6, 0xd0, 0xce, 0xc4, b' ', b'o', b'k'];
        assert_eq!(decode_output(&gbk), "中文 ok");
    }

    /// 命令解析：带路径的原样放过；不带路径的要么解析到真实文件、要么原样返回
    /// （后者让 spawn 去报错，错误里会带命令原文）。
    #[test]
    fn resolve_command_keeps_paths_and_resolves_bare_names() {
        let (cmd, args) = resolve_command("C:/tools/thing.exe", &["a".into()]);
        assert_eq!(cmd, "C:/tools/thing.exe");
        assert_eq!(args, vec!["a".to_string()]);

        let (cmd, _) = resolve_command("definitely-not-a-real-command-xyz", &[]);
        assert_eq!(cmd, "definitely-not-a-real-command-xyz");

        #[cfg(windows)]
        {
            // cmd 一定在；解析出来要么是 cmd.exe、要么是套一层 cmd /C（shim 的情况）
            let (cmd, args) = resolve_command("cmd", &["/C".into(), "echo hi".into()]);
            let joined = format!("{cmd} {}", args.join(" ")).to_ascii_lowercase();
            assert!(joined.contains("cmd"), "实际：{joined}");
            assert!(describe_command("cmd").to_ascii_lowercase().contains("cmd"));
        }
    }
}
