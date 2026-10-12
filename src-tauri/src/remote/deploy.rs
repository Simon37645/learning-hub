//! 一键把中继装到用户自己的服务器上。
//!
//! 用户只想给三样东西——**IP、用户名、密码**——然后手机就能用。让用户自己去
//! `ssh` 上去写 unit、装 Node、敲 systemd 命令，等于把「远程控制」变成一件
//! 只有运维才会用的事。所以这一步由应用代劳：SSH 连上去，把
//! [`server/relay.mjs`](../../../server/relay.mjs)（编进二进制的那份）写过去、
//! 准备好 Node 运行时、起一个常驻服务，然后把配置填好、隧道接上。
//!
//! ## 为什么自己实现 SSH 而不调系统的 ssh
//!
//! Windows 上没有 `sshpass`，密码登录只能交互输入；用 `SSH_ASKPASS` 那套
//! 又要额外的可执行文件，还得把密码放进进程参数（同机器上谁都能看见）。
//! 用 russh（纯 Rust）就都在进程内，密码不进命令行、不落盘。
//!
//! ## 装的时候**不碰**用户机器上的既有服务
//!
//! - 不装系统包：优先用服务器上已有的 `node`；没有就下载一份官方 Node 运行时
//!   解到自己的目录里（只在 `apt-get install nodejs` 这一步会碰包管理器，
//!   而且只在确实没有 node 时才试）。
//! - 不写 nginx / 不改防火墙（默认**不反代**）：中继自己监听一个端口。
//! - 只在自己的目录（`/opt/learning-hub-relay` 或 `~/.learning-hub-relay`）
//!   与一个同名的 systemd 服务上动手脚。
//!
//! ## 主机公钥：TOFU
//!
//! 首次连接没有 known_hosts 可比，只能接受并**记住指纹**；之后再连发现指纹变了
//! 就拒绝并让用户确认（换机器了？还是有人在中间？）。这比 `StrictHostKeyChecking=no`
//! 一直盲信要诚实一些，也不至于让用户先去手动 ssh 一次。

use crate::error::{AppError, AppResult};
use russh::client::{self, Handle};
use russh::ChannelMsg;
use std::sync::Arc;
use std::time::Duration;

/// 中继脚本本体：跟着应用版本走，不从网上下（否则装上去的版本和面板说的不一致）。
pub const RELAY_SOURCE: &str = include_str!("../../../server/relay.mjs");

/// 免安装的 Node 运行时版本（服务器上既没有 node、包管理器也装不了时用）。
const NODE_VERSION: &str = "v22.11.0";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 整段安装的时间上限：下载 Node（约 25MB）在慢机器上要一会儿
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

/// SSH 登录信息。密码只在这个结构里活一次，不写进配置。
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployReport {
    pub ok: bool,
    /// 服务器上的输出（逐行推给面板，用户能看见装了什么）
    pub log: Vec<String>,
    /// Node 从哪儿来的：已有 / apt / 下载
    pub node: String,
    /// systemd 还是后台进程
    pub service: String,
    pub port: u16,
    /// 服务器公钥指纹（TOFU，存进配置）
    pub host_key: String,
    pub error: Option<String>,
}

/// 主机公钥检查。
struct ClientHandler {
    /// 配置里记着的指纹（空 = 第一次连）
    expected: Option<String>,
    /// 实际看到的指纹。`Arc` 是为了在 handler 被 move 进 connect 之后还能读出来
    seen: Arc<parking_lot::Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // 指纹算在公钥上（证书登录也算它里面那把公钥）
        let fingerprint = server_public_key
            .public_key()
            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
            .to_string();
        *self.seen.lock() = Some(fingerprint.clone());
        match &self.expected {
            Some(want) if !want.is_empty() && want != &fingerprint => {
                // 拒绝：指纹变了意味着对面不是上次那台机器（或有人在中间）。
                // 说清楚怎么办：面板上会提示用户去「重新信任这台服务器」。
                Err(russh::Error::UnknownKey)
            }
            _ => Ok(true),
        }
    }
}

/// 连上并装好中继。`progress` 收到的每一行都会实时推给面板。
///
/// 返回 `Ok(report)` 表示**连接与执行**这一步有结果（成功或失败都在 report 里）；
/// `Err` 只用于「连都连不上」这类连日志都没得看的情况。
pub async fn deploy(
    target: &SshTarget,
    port: u16,
    token: &str,
    expected_host_key: Option<&str>,
    progress: impl Fn(&str) + Send + Sync + 'static,
) -> AppResult<DeployReport> {
    if target.host.trim().is_empty() || target.user.trim().is_empty() {
        return Err(AppError::invalid("服务器地址和用户名都要填"));
    }
    if target.password.is_empty() {
        return Err(AppError::invalid("需要 SSH 密码才能替你装（密码只用于这一次，不保存）"));
    }

    let seen = Arc::new(parking_lot::Mutex::new(None));
    let handler = ClientHandler {
        expected: expected_host_key.map(|s| s.to_string()),
        seen: seen.clone(),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(120)),
        ..Default::default()
    });

    let addr = (target.host.trim(), target.port);
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, addr, handler)).await;
    let mut session: Handle<ClientHandler> = match connect {
        Err(_) => {
            return Err(AppError::other(format!(
                "连 {}:{} 超时（服务器不通？SSH 端口不是 {}？）",
                target.host, target.port, target.port
            )))
        }
        Ok(Err(russh::Error::UnknownKey)) => {
            return Err(AppError::other(
                "服务器公钥和上次记住的不一样——可能换了机器，也可能有人在中间。\
                 确认无误后可以在面板上点「重新信任这台服务器」。"
                    .to_string(),
            ))
        }
        Ok(Err(e)) => return Err(AppError::other(format!("SSH 连接失败：{e}"))),
        Ok(Ok(s)) => s,
    };

    // 密码认证；有些服务器（走 PAM 的）只提供 keyboard-interactive，用同一个密码再试一次
    let auth = session
        .authenticate_password(target.user.trim(), &target.password)
        .await
        .map_err(|e| AppError::other(format!("SSH 认证出错：{e}")))?;
    if !auth.success() {
        let ok = keyboard_interactive(&mut session, target).await.unwrap_or(false);
        if !ok {
            return Err(AppError::other(
                "用户名或密码不对（也试过 keyboard-interactive 了）。root 登录被禁用的话，\
                 请用一个有 sudo 权限的账号。",
            ));
        }
    }

    let host_key = seen.lock().clone().unwrap_or_default();
    progress(&format!("已登录 {}（{}）", target.host.trim(), target.user.trim()));

    // 脚本走 stdin（`bash -s`），不进命令行——否则服务器上 `ps` 能看到脚本里的口令
    let script = build_script(port, token, &target.password);
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| AppError::other(format!("开 SSH 会话失败：{e}")))?;
    channel
        .exec(true, "bash -s")
        .await
        .map_err(|e| AppError::other(format!("执行安装脚本失败：{e}")))?;
    // 0.64 的 Channel 不直接实现 AsyncWrite，用 data_bytes 发（脚本 10KB 上下，
    // 一次发完就行；分片由库自己按窗口切）
    channel.data_bytes(script.into_bytes()).await.ok();
    channel.eof().await.ok();

    let mut report = DeployReport {
        port,
        host_key,
        ..Default::default()
    };
    let mut exit: Option<u32> = None;

    let drain = async {
        let mut buf = String::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    buf.push_str(&String::from_utf8_lossy(&data));
                    // 逐行推给面板（最后一行可能不完整，留着下一轮再拼）
                    while let Some(at) = buf.find('\n') {
                        let line = buf[..at].trim_end_matches('\r').to_string();
                        buf.drain(..=at);
                        if !line.trim().is_empty() {
                            report.log.push(line.clone());
                            progress(&line);
                        }
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
                _ => {}
            }
        }
        if !buf.trim().is_empty() {
            report.log.push(buf.trim().to_string());
            progress(buf.trim());
        }
    };

    if tokio::time::timeout(INSTALL_TIMEOUT, drain).await.is_err() {
        return Err(AppError::other(
            "安装超时（5 分钟）。服务器可能太慢或卡在下载 Node——重新点一次即可，\
             已经装好的部分不会重做。",
        ));
    }
    session
        .disconnect(russh::Disconnect::ByApplication, "done", "")
        .await
        .ok();

    report.ok = exit == Some(0) && report.log.iter().any(|l| l.contains("LH_DEPLOY_DONE"));
    report.node = pick(&report.log, "LH_NODE=").unwrap_or_else(|| "未知".into());
    report.service = pick(&report.log, "LH_SERVICE=").unwrap_or_else(|| "未知".into());
    if !report.ok {
        report.error = Some(
            pick(&report.log, "LH_ERROR=").unwrap_or_else(|| {
                format!("安装没有成功（退出码 {}），上面的输出里有原因", exit.map(|c| c.to_string()).unwrap_or_else(|| "未知".into()))
            }),
        );
    }
    Ok(report)
}

/// `LH_NODE=已有 node v20` 这种标记行的取值。
fn pick(log: &[String], key: &str) -> Option<String> {
    log.iter().find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_string()))
}

/// keyboard-interactive 认证（把密码当答案填进去）。很多服务器两种都开，
/// 但走 PAM 的机器有时只留这一种。
async fn keyboard_interactive(
    session: &mut Handle<ClientHandler>,
    target: &SshTarget,
) -> Result<bool, russh::Error> {
    let mut response = session
        .authenticate_keyboard_interactive_start(target.user.trim(), None::<String>)
        .await?;
    loop {
        match response {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(true),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| target.password.clone()).collect::<Vec<_>>();
                response = session
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
}

/// 单引号包裹一段 shell 字符串（里面出现单引号就断开再接上）。
///
/// 密码会进这段脚本（sudo 要用），所以必须**正确地**转义——写错了轻则安装失败，
/// 重则让密码里的某个字符变成一条命令。有单测盯着。
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 生成安装脚本。端口、口令、密码都嵌在里面（脚本经 SSH 的 stdin 送达，
/// 不出现在服务器的进程列表里）。
pub fn build_script(port: u16, token: &str, password: &str) -> String {
    format!(
        r#"set -u
PW={pw}
PORT={port}
TOKEN={token}

echo "LH_NODE=探测中"
echo "[1/5] 环境：$(uname -srm)　用户：$(id -un)（uid=$(id -u)）"

# ---------------------------------------------------------------- 权限与目录
DIR=""
SERVICE=""
if [ "$(id -u)" = "0" ]; then
  DIR=/opt/learning-hub-relay
elif command -v sudo >/dev/null 2>&1 && printf '%s\n' "$PW" | sudo -S -p '' true >/dev/null 2>&1; then
  DIR=/opt/learning-hub-relay
else
  DIR="$HOME/.learning-hub-relay"
fi
echo "  安装目录：$DIR"

as_root() {{
  if [ "$(id -u)" = "0" ]; then "$@"; else printf '%s\n' "$PW" | sudo -S -p '' "$@"; fi
}}
can_root() {{ [ "$(id -u)" = "0" ] || command -v sudo >/dev/null 2>&1; }}

# ---------------------------------------------------------------- Node 运行时
echo "[2/5] 准备 Node 运行时"
NODE_BIN=""
NODE_FROM=""
if command -v node >/dev/null 2>&1; then
  NODE_BIN=$(command -v node); NODE_FROM="已有 $(node -v 2>/dev/null)"
elif command -v apt-get >/dev/null 2>&1 && can_root; then
  echo "  服务器上没有 node，试一下包管理器（不改动任何正在跑的服务）"
  as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nodejs >/dev/null 2>&1 || true
  if command -v node >/dev/null 2>&1; then NODE_BIN=$(command -v node); NODE_FROM="apt 装好 $(node -v 2>/dev/null)"; fi
fi
if [ -z "$NODE_BIN" ]; then
  case "$(uname -m)" in
    x86_64|amd64) ARCH=x64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *) echo "LH_ERROR=这个架构（$(uname -m)）没有现成的 Node 包，请先手动装 Node 18+"; exit 11 ;;
  esac
  BASE="https://nodejs.org/dist/{node_version}/node-{node_version}-linux-$ARCH"
  echo "  下载官方 Node 运行时（约 25MB，只此一次）"
  as_root mkdir -p "$DIR" 2>/dev/null || mkdir -p "$DIR"
  if curl -fsSL "$BASE.tar.xz" -o "$DIR/node.tar.xz" 2>/dev/null && tar -xJf "$DIR/node.tar.xz" -C "$DIR" 2>/dev/null; then
    :
  elif curl -fsSL "$BASE.tar.gz" -o "$DIR/node.tar.gz" 2>/dev/null && tar -xzf "$DIR/node.tar.gz" -C "$DIR" 2>/dev/null; then
    :
  else
    echo "LH_ERROR=下载 Node 失败——服务器能出网吗？也可以先在服务器上装好 node 再点一次"
    exit 12
  fi
  rm -f "$DIR"/node.tar.*
  rm -rf "$DIR/node-runtime"
  mv "$DIR/node-{node_version}-linux-$ARCH" "$DIR/node-runtime"
  NODE_BIN="$DIR/node-runtime/bin/node"
  NODE_FROM="下载的官方运行时"
fi
echo "LH_NODE=$NODE_FROM"
echo "  $NODE_BIN"

# ---------------------------------------------------------------- 写中继脚本
echo "[3/5] 写入中继脚本"
as_root mkdir -p "$DIR" 2>/dev/null || mkdir -p "$DIR"
cat > "$DIR/relay.mjs" <<'LH_RELAY_EOF'
{relay}
LH_RELAY_EOF
chmod 644 "$DIR/relay.mjs"
echo "  $DIR/relay.mjs（$(wc -c < "$DIR/relay.mjs") 字节）"

# ---------------------------------------------------------------- 起服务
echo "[4/5] 启动常驻服务"
START_CMD="\"$NODE_BIN\" \"$DIR/relay.mjs\" --token \"$TOKEN\" --port \"$PORT\" --bind 0.0.0.0"
if command -v systemctl >/dev/null 2>&1 && [ "$DIR" = "/opt/learning-hub-relay" ] && can_root; then
  UNIT=/etc/systemd/system/learning-hub-relay.service
  printf '%s\n' \
    "[Unit]" \
    "Description=Learning Hub remote relay (学习中枢 远程中继)" \
    "After=network-online.target" \
    "Wants=network-online.target" \
    "" \
    "[Service]" \
    "Type=simple" \
    "ExecStart=$NODE_BIN $DIR/relay.mjs --token $TOKEN --port $PORT --bind 0.0.0.0" \
    "Restart=always" \
    "RestartSec=3" \
    "NoNewPrivileges=true" \
    "" \
    "[Install]" \
    "WantedBy=multi-user.target" | as_root tee "$UNIT" >/dev/null
  as_root systemctl daemon-reload
  as_root systemctl enable learning-hub-relay >/dev/null 2>&1
  as_root systemctl restart learning-hub-relay
  SERVICE="systemd"
else
  # 没有 systemd（或没有 root）：用后台进程 + 一条 @reboot 定时任务兜住重启
  pkill -f "learning-hub-relay/relay.mjs" >/dev/null 2>&1 || true
  setsid nohup $START_CMD > "$DIR/relay.log" 2>&1 < /dev/null &
  sleep 1
  if command -v crontab >/dev/null 2>&1; then
    (crontab -l 2>/dev/null | grep -v "learning-hub-relay/relay.mjs"; \
     echo "@reboot setsid nohup $START_CMD > \"$DIR/relay.log\" 2>&1 < /dev/null &") | crontab - 2>/dev/null || true
  fi
  SERVICE="后台进程（+ 开机自启）"
fi
echo "LH_SERVICE=$SERVICE"

# ---------------------------------------------------------------- 验证
echo "[5/5] 验证"
OK=0
for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  if command -v ss >/dev/null 2>&1; then
    ss -tln 2>/dev/null | grep -q ":$PORT " && {{ OK=1; break; }}
  elif command -v netstat >/dev/null 2>&1; then
    netstat -tln 2>/dev/null | grep -q ":$PORT " && {{ OK=1; break; }}
  else
    OK=1; break
  fi
done
if [ "$OK" = "1" ]; then
  echo "  端口 $PORT 已在监听"
else
  echo "  端口 $PORT 没起来，最近几行日志："
  if [ "$SERVICE" = "systemd" ]; then
    as_root journalctl -u learning-hub-relay -n 8 --no-pager 2>/dev/null | tail -8
  else
    tail -8 "$DIR/relay.log" 2>/dev/null
  fi
  echo "LH_ERROR=服务没起来（端口被占用？看上面的日志）"
  exit 13
fi

echo "LH_DEPLOY_DONE"
echo "  完成：中继在 0.0.0.0:$PORT 上等电脑连进来。"
echo "  如果手机连不上，检查云服务商的安全组是否放行了 $PORT（这台机器自己的防火墙不用改）。"
"#,
        pw = sh_quote(password),
        port = port,
        token = sh_quote(token),
        node_version = NODE_VERSION,
        relay = RELAY_SOURCE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_survives_nasty_passwords() {
        assert_eq!(sh_quote("abc"), "'abc'");
        assert_eq!(sh_quote("a b"), "'a b'");
        // 单引号：断开、转义、接上——写错的话密码里的引号会变成一条命令
        assert_eq!(sh_quote("it's"), r#"'it'\''s'"#);
        assert_eq!(sh_quote("'; rm -rf /"), r#"''\''; rm -rf /'"#);
        assert_eq!(sh_quote("$HOME`id`"), "'$HOME`id`'");
    }

    #[test]
    fn script_carries_port_token_and_relay_source() {
        let s = build_script(54713, "tok123", "p@ss");
        assert!(s.contains("PORT=54713"));
        assert!(s.contains("TOKEN='tok123'"));
        assert!(s.contains("PW='p@ss'"));
        // 中继脚本本体要嵌在 heredoc 里，且用带引号的 EOF（不展开变量）
        assert!(s.contains("<<'LH_RELAY_EOF'"));
        assert!(s.contains("学习中枢中继已启动") || s.contains("relay.mjs"));
        // 面板靠这几行标记判断成败
        assert!(s.contains("LH_DEPLOY_DONE"));
        assert!(s.contains("LH_SERVICE="));
        // 不碰反代/防火墙：脚本里不该出现 nginx 或 iptables/ufw 的写操作
        assert!(!s.contains("nginx"));
        assert!(!s.contains("ufw "));
        assert!(!s.contains("iptables"));
    }

    #[test]
    fn relay_source_is_the_repo_file() {
        // 编进二进制的那份必须还是那个能跑的中继（别哪天被换成空文件）
        assert!(RELAY_SOURCE.contains("LH1 "), "握手协议要在");
        assert!(RELAY_SOURCE.contains("listen"), "要真的监听端口");
        assert!(RELAY_SOURCE.len() > 3000, "中继脚本不该这么短：{}", RELAY_SOURCE.len());
    }
}
