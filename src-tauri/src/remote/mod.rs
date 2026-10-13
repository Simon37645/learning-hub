//! 移动端远程控制：在手机 / 平板的浏览器里操作这台电脑上的 agent。
//!
//! **不是屏幕共享**。投屏只能看、点不动；这里给的是一套真正的操作界面——
//! 手机上发的消息真的由这台电脑上的 agent 执行（工具、审批、写笔记都在本机发生），
//! 事件也是从桌面端同一套 `AgentEvent` 转出去的。所以它不是「把画面推出去」，
//! 而是「把控制权交出去」。
//!
//! **手机上的界面就是桌面端的界面**：远程服务把编译进二进制的同一份前端产物
//! （`dist/`，见 `api::page`）直接送出去，再注入一个 `shim.js` 把
//! `window.__TAURI_INTERNALS__` 换成「HTTP + SSE」的版本。于是手机上跑的是
//! 同一套组件、同一份 CSS、同一批命令——不会随时间漂移成「另一个手机版」。
//!
//! ```text
//! 手机浏览器 ──HTTP/SSE──→ remote::http（迷你 HTTP/1.1）──→ remote::api
//!                                                          │  /api/invoke
//!                                                          ↓  remote::bridge（白名单）
//! 桌面端前端 ←── hub://remote-invoke ──┘                    │
//! 桌面端 AppCore ──emit_event/…──→ RemoteService::push_event ──→ SSE 扇出
//! ```
//!
//! 三个刻意的取舍：
//!
//! 1. **不引 HTTP 框架**：只需要「静态页面 + 几个 JSON 接口 + 一条推送流」，
//!    手写 400 行比拖进 hyper/tower 那一串依赖更好交代（和 MCP 客户端一样）。
//! 2. **用 SSE 而不是 WebSocket**：事件是单向的（服务器 → 手机），上行全是普通
//!    POST。省掉自己实现帧协议与握手摘要，断线重连也是 `EventSource` 自带的。
//! 3. **默认关闭**：这是一条能从别的设备操作本机的通道，不打开就不监听端口。
//!
//! 安全边界（都在这一层，别往别处放）：
//! - `/api/*` 一律要令牌（查询串 `?t=`、`Authorization: Bearer` 或 `X-Remote-Token`），
//!   比较是常量时间的；静态页面本身不需要令牌（页面里没有秘密，接口才有）。
//! - 远程能执行哪些命令由 [`bridge`] 的白名单说了算：能读、能学、能聊，
//!   **不能改应用配置、改工作区结构、删文件**（每一条都写了原因，会显示给用户）。
//! - 附件只能读**工作区内**的路径（`paths::resolve_in_root`），`../` 一律拒绝。

pub mod api;
pub mod bridge;
pub mod deploy;
pub mod http;
pub mod tunnel;

use crate::error::{AppError, AppResult};
use crate::state::AppCore;
use parking_lot::Mutex;
use serde::Serialize;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};

/// 页面的桥（把 IPC 接到 HTTP/SSE 上）。**静态资源，改完要重新编译**。
pub const SHIM_JS: &str = include_str!("web/shim.js");
/// 应用图标：PWA 清单与「添加到主屏幕」的图标都用它。
pub const WEB_ICON: &str = include_str!("web/icon.svg");

/// 事件扇出的一帧。
///
/// 手机端跑的是桌面端那份前端，它认的事件名和桌面端完全一样（`hub://agent`、
/// `hub://viewer`…），所以这里**不做二次翻译**：原样带上事件名与载荷，
/// shim 按名字分派给 `listen()` 注册的回调。少一层映射就少一处会漂的地方。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Event { name: String, payload: serde_json::Value },
    /// 连着几台设备变了（面板与手机上的计数都据此更新）
    Clients { count: usize },
}

struct Running {
    port: u16,
    /// 停服信号。用 `watch` 而不是 `Notify`：**停服信号不能丢**——
    /// `notify_waiters` 只叫醒「当下正在等」的那些 future，要是叫醒的那一刻
    /// accept 循环正好在两次等待之间，信号就永远丢了，监听套接字不释放，
    /// 紧接着的重启会撞上「端口被占用」。`watch` 的接收端读的是**当前值**，
    /// 错过通知也能从 `borrow()` 看出来。
    shutdown: watch::Sender<bool>,
}

pub struct RemoteService {
    /// 事件扇出。没有订阅者时 `send` 直接返回 Err（一个原子读），忽略即可
    hub: broadcast::Sender<Frame>,
    running: Mutex<Option<Running>>,
    /// 正在连着的浏览器数（面板上显示「已连接 N 台」）
    clients: Arc<AtomicUsize>,
    /// 上次启动/运行失败的原因。面板上要能看见，否则用户只看到「没反应」
    last_error: Mutex<Option<String>>,
    /// 隧道（自己有公网服务器时主动连出去的那条）
    tunnel: Mutex<Option<watch::Sender<bool>>>,
    /// 令牌试错计数（窗口内超过上限就限流），见 [`RemoteService::note_auth_failure`]
    failures: Mutex<(std::time::Instant, u32)>,
    /// IPC 桥的等待位（手机上每个 invoke 一个）
    pub bridge: bridge::InvokeBridge,
}

/// 令牌试错的上限与窗口。令牌是 32 位十六进制，暴力猜不现实；
/// 这一层挡的是「脚本拿着一个错令牌猛敲」，顺带让日志安静点。
const AUTH_FAIL_LIMIT: u32 = 20;
const AUTH_FAIL_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

impl RemoteService {
    pub fn new() -> Self {
        // 容量给足：一轮对话的工具调用会连发几十帧，慢手机追不上时
        // 宁可让 receiver 报 Lagged（页面收到就重拉一次状态），也不要卡住 agent。
        let (hub, _) = broadcast::channel(512);
        Self {
            hub,
            running: Mutex::new(None),
            clients: Arc::new(AtomicUsize::new(0)),
            last_error: Mutex::new(None),
            tunnel: Mutex::new(None),
            failures: Mutex::new((std::time::Instant::now(), 0)),
            bridge: bridge::InvokeBridge::new(),
        }
    }

    /// 记一次令牌错误。返回 true 表示「该限流了」。
    pub fn note_auth_failure(&self) -> bool {
        let mut guard = self.failures.lock();
        if guard.0.elapsed() > AUTH_FAIL_WINDOW {
            *guard = (std::time::Instant::now(), 0);
        }
        guard.1 += 1;
        guard.1 > AUTH_FAIL_LIMIT
    }

    /// 令牌对了就把窗口清掉：正常用户偶尔手滑几次不该被自己的服务限流。
    pub fn note_auth_success(&self) {
        let mut guard = self.failures.lock();
        if guard.1 != 0 {
            *guard = (std::time::Instant::now(), 0);
        }
    }

    // ------------------------------------------------------------ 事件扇出

    /// 把一个事件原样扇给连着的手机。
    ///
    /// 每个 push 都先看一眼「有没有人听」：没人听的时候连克隆都省掉——
    /// 桌面端自己在跑的时候不该为了一个没打开的远程多付一次事件拷贝。
    /// 调用点只有 `AppCore::emit_event` 一处（见那里的说明）。
    pub fn push_event<T: Serialize>(&self, name: &str, payload: &T) {
        if self.hub.receiver_count() == 0 {
            return;
        }
        let Ok(payload) = serde_json::to_value(payload) else {
            return;
        };
        let _ = self.hub.send(Frame::Event { name: name.to_string(), payload });
    }

    fn push_clients(&self) {
        let _ = self.hub.send(Frame::Clients { count: self.clients() });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.hub.subscribe()
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.running.lock().is_some()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    fn note_error(&self, e: impl ToString) {
        *self.last_error.lock() = Some(e.to_string());
    }

    // ------------------------------------------------------------ 生命周期

    /// 起服务（幂等：已经在跑就先停掉再起）。端口被占用等错误**同步**报出去，
    /// 用户点「打开」时能立刻看到原因，而不是「打开了但连不上」。
    pub fn start(self: &Arc<Self>, core: Arc<AppCore>) -> AppResult<RemoteStatus> {
        self.stop();

        // 令牌缺了就补（首次打开、或用户手动清空过）
        if core.config_read().remote.token.trim().is_empty() {
            core.update_config(|c| {
                c.remote.ensure_tokens();
            })?;
        }
        let cfg = core.config_read().remote;

        let listener = bind_with_retry(cfg.bind.addr(), cfg.port).map_err(|e| {
            AppError::other(format!(
                "远程服务起不来：{}:{} 端口被占用或不允许监听（{e}）。换一个端口再试。",
                cfg.bind.addr(),
                cfg.port
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|e| AppError::other(format!("设置非阻塞失败：{e}")))?;

        let (shutdown, shutdown_rx) = watch::channel(false);
        *self.running.lock() = Some(Running { port: cfg.port, shutdown: shutdown.clone() });
        *self.last_error.lock() = None;

        let me = self.clone();
        let listen_core = core.clone();
        // 绑定用同步的 std 监听器（这样「端口被占用」能在用户点按钮时立刻报出来），
        // 但交给 tokio 必须**在运行时里**做——`from_std` 要注册到 reactor，
        // 在 setup 线程 / 命令线程里直接调会 panic（踩过一次：整个应用起不来）。
        tauri::async_runtime::spawn(async move {
            match TcpListener::from_std(listener) {
                Ok(listener) => serve_loop(listen_core, me.clone(), listener, shutdown_rx).await,
                Err(e) => {
                    eprintln!("[remote] 接管监听套接字失败：{e}");
                    me.note_error(format!("接管监听套接字失败：{e}"));
                    me.stop();
                }
            }
        });

        // 有公网服务器（中继）就顺手把隧道也接上——用户点一次「打开远程」，
        // 该通的路全通，不用再点第二个开关。
        if !cfg.relay.trim().is_empty() {
            let token = cfg.relay_token.clone();
            let relay = cfg.relay.trim().to_string();
            let port = cfg.port;
            let me2 = self.clone();
            let (shutdown, shutdown_rx) = watch::channel(false);
            *self.tunnel.lock() = Some(shutdown);
            tauri::async_runtime::spawn(async move {
                tunnel::run(me2, relay, token, port, shutdown_rx).await;
            });
        }

        eprintln!("[remote] 已启动：{}:{}", cfg.bind.label(), cfg.port);
        Ok(self.status(&core))
    }

    /// 停服务：叫醒 accept 循环与所有 SSE 流，并断掉隧道。
    ///
    /// 不阻塞等它退干净（那是异步的），所以紧接着的重启可能抢在旧监听器释放之前——
    /// [`bind_with_retry`] 就是为这个准备的。
    pub fn stop(&self) {
        if let Some(r) = self.running.lock().take() {
            let _ = r.shutdown.send(true);
        }
        if let Some(t) = self.tunnel.lock().take() {
            let _ = t.send(true);
        }
        if self.clients.swap(0, Ordering::Relaxed) > 0 {
            self.push_clients();
        }
    }

    /// 重启（改端口 / 改监听地址 / 换令牌之后）。
    pub fn restart(self: &Arc<Self>, core: Arc<AppCore>) -> AppResult<RemoteStatus> {
        self.start(core)
    }

    // ------------------------------------------------------------ 状态

    pub fn status(&self, core: &AppCore) -> RemoteStatus {
        let cfg = core.config_read().remote;
        let running = self.running.lock().as_ref().map(|r| r.port);
        let token = cfg.token.clone();

        // 顺序就是「从任何地方都能用」到「只有本机能用」：二维码用第一个。
        // 局域网地址可能不止一个（有线 + 无线 + VPN），全列出来——
        // 排第一的那个不一定是手机能到的那块网卡，用户要能换一个试。
        let mut urls: Vec<RemoteUrl> = Vec::new();
        // 公网入口优先用「对外地址」（有反代的填它），没填就用中继地址——
        // 中继那个端口既是隧道入口也是网页入口，所以装好中继之后二维码直接能扫，
        // 用户不必再理解「对外地址」是什么。
        let public_base = normalize_base(&cfg.public_url).or_else(|| crate::config::relay_base(&cfg.relay));
        if let Some(base) = public_base {
            urls.push(RemoteUrl {
                url: link(&base, &token),
                kind: "public".into(),
                label: if normalize_base(&cfg.public_url).is_some() {
                    "对外".into()
                } else {
                    "公网中继".into()
                },
            });
        }
        if cfg.bind == crate::config::RemoteBind::Lan {
            for (ip, alias) in local_ipv4_candidates() {
                urls.push(RemoteUrl {
                    url: link(&format!("http://{ip}:{}", cfg.port), &token),
                    kind: "lan".into(),
                    // 带上网卡名：多网卡时用户才知道该换哪一行试（WLAN / 以太网 / ZeroTier）
                    label: if alias.is_empty() {
                        format!("局域网 · {ip}")
                    } else {
                        format!("局域网 · {alias}")
                    },
                });
            }
        }
        urls.push(RemoteUrl {
            url: link(&format!("http://127.0.0.1:{}", cfg.port), &token),
            kind: "local".into(),
            label: "仅本机".into(),
        });

        RemoteStatus {
            enabled: cfg.enabled,
            running: running.is_some(),
            port: cfg.port,
            bind: cfg.bind,
            bind_label: cfg.bind.label().to_string(),
            token,
            relay: cfg.relay.clone(),
            relay_token: cfg.relay_token.clone(),
            ssh_port: cfg.ssh_port,
            ssh_user: cfg.ssh_user.clone(),
            host_key: cfg.host_key.clone(),
            public_url: normalize_base(&cfg.public_url).map(|b| link(&b, &cfg.token)),
            lan_urls: urls.iter().filter(|u| u.kind == "lan").map(|u| u.url.clone()).collect(),
            local_url: urls.iter().find(|u| u.kind == "local").map(|u| u.url.clone()).unwrap_or_default(),
            urls,
            clients: self.clients(),
            last_error: self.last_error(),
        }
    }
}

impl Default for RemoteService {
    fn default() -> Self {
        Self::new()
    }
}

/// 面板上的一行地址。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUrl {
    pub url: String,
    /// public / lan / local（界面据此分组，别靠字符串猜）
    pub kind: String,
    pub label: String,
}

/// 给桌面端面板看的远程状态（含令牌——那是本机界面，用户要靠它拼链接与二维码）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub bind: crate::config::RemoteBind,
    pub bind_label: String,
    pub token: String,
    pub relay: String,
    pub relay_token: String,
    /// SSH 端口 / 用户名（一键安装要重装时预填；密码不存）
    pub ssh_port: u16,
    pub ssh_user: String,
    pub host_key: String,
    pub public_url: Option<String>,
    pub lan_urls: Vec<String>,
    pub local_url: String,
    /// 从「任何地方都能用」到「只有本机能用」排序；二维码取第一个
    pub urls: Vec<RemoteUrl>,
    pub clients: usize,
    pub last_error: Option<String>,
}

/// `http://1.2.3.4:8787/` → `http://1.2.3.4:8787`（去掉尾斜杠，容忍用户填带路径的反代地址）
pub fn normalize_base(raw: &str) -> Option<String> {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() {
        return None;
    }
    Some(t.to_string())
}

/// 拼出给手机打开的链接。令牌放查询串：`EventSource` 不能自定义请求头，
/// 二维码里也只能是一整条 URL。
pub fn link(base: &str, token: &str) -> String {
    format!("{}/?t={token}", base.trim_end_matches('/'))
}

/// 本机在局域网里的 IPv4 地址。
///
/// 用「连一个公网 UDP 地址再看本地端口」的办法问操作系统要默认路由的那块网卡——
/// UDP 的 connect 不发包，所以断网也能用；引一个网卡枚举库只为这一件事不值。
///
/// **但这只是兜底**：装了 VPN / Tailscale / ZeroTier 的机器上，默认路由常常是那块
/// 虚拟网卡，问出来的地址手机根本连不上（实测过：二维码里出现 100.64.x.x）。
/// 所以主力是 [`local_ipv4_candidates`]——把网卡都列出来按「像不像家用局域网」排序。
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    for probe in ["223.5.5.5:80", "8.8.8.8:80"] {
        let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else { continue };
        if sock.connect(probe).is_err() {
            continue;
        }
        if let Ok(std::net::SocketAddr::V4(a)) = sock.local_addr() {
            if !a.ip().is_unspecified() && !a.ip().is_loopback() {
                return Some(*a.ip());
            }
        }
    }
    None
}

/// 一块网卡上的地址：IP + 网卡名（面板上要显示「局域网 · WLAN」这种，用户才知道换哪行试）。
pub type LanCandidate = (Ipv4Addr, String);

/// 这台机器上所有像样的 IPv4 地址，**按「手机最可能连得上」排序**。
///
/// 排序规则（纯函数，有单测）：
/// 1. **操作系统默认路由用的那个地址**排第一。这台机器上可能有 VPN / ZeroTier /
///    Tailscale 的虚拟网卡，光看网段猜不出哪块是「真网卡」——问操作系统的选择最准
///    （实测：ZeroTier 的 10.x 会被按网段规则排到真正的 WLAN 前面，那是个错的顺序）。
/// 2. 其余按网段：家用 / 办公局域网（192.168.x、10.x、172.16-31.x）优先，
///    其它（含 100.64/10 这种 VPN 用的 CGNAT 段）靠后。
///
/// 为什么要列出全部而不是只给一个：VPN 开着的时候「默认路由那块网卡」也未必是
/// 手机能到的那块，只给一个地址等于给了个连不上的二维码。
pub fn rank_addresses(mut addrs: Vec<LanCandidate>, preferred: Option<Ipv4Addr>) -> Vec<LanCandidate> {
    addrs.retain(|(a, _)| !a.is_loopback() && !a.is_unspecified() && !a.is_link_local() && !a.is_multicast());
    addrs.sort_by_key(|(a, _)| {
        let is_preferred = preferred.is_some_and(|p| p == *a);
        (if is_preferred { 0 } else { 1 }, tier(*a), u32::from(*a))
    });
    addrs.dedup_by_key(|(a, _)| *a);
    addrs
}

/// 越小越优先。
fn tier(a: Ipv4Addr) -> u8 {
    let [x, y, ..] = a.octets();
    if x == 192 && y == 168 {
        0
    } else if x == 10 {
        1
    } else if x == 172 && (16..=31).contains(&y) {
        2
    } else {
        // 100.64/10 是运营商级 NAT 段，VPN 常用（Tailscale 就是它）；
        // 手机不一定在那个虚拟网络里，所以排后面。
        3
    }
}

/// 枚举本机地址（带缓存）。
///
/// Windows 上问一次 PowerShell（`Get-NetIPAddress`）：语言无关、不引库；
/// 别的平台 / 命令失败时退回默认路由探测。缓存 60 秒——面板每 3 秒刷一次状态，
/// 每次都去起一个进程就太浪费了。
fn local_ipv4_candidates() -> Vec<LanCandidate> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<parking_lot::Mutex<Option<(std::time::Instant, Vec<LanCandidate>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| parking_lot::Mutex::new(None));
    {
        let guard = cache.lock();
        if let Some((at, addrs)) = guard.as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(60) {
                return addrs.clone();
            }
        }
    }
    let preferred = lan_ipv4();
    let mut found = enumerate_ipv4();
    if let Some(ip) = preferred {
        if !found.iter().any(|(a, _)| *a == ip) {
            found.push((ip, String::new()));
        }
    }
    let ranked = rank_addresses(found, preferred);
    *cache.lock() = Some((std::time::Instant::now(), ranked.clone()));
    ranked
}

#[cfg(windows)]
fn enumerate_ipv4() -> Vec<LanCandidate> {
    use std::process::Command;
    // -NoProfile：别让用户的 PowerShell 配置拖慢或干扰这一段
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { \"$($_.IPAddress)|$($_.InterfaceAlias)\" }",
    ]);
    // 不加这个就会闪一个黑框：powershell 是控制台程序，从 GUI 进程起它会拿到
    // 一个新控制台窗口（缓存 60 秒，所以是「每分钟闪一下」这种很迷惑的症状）
    crate::proc::no_console(&mut cmd);
    let out = cmd.output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (ip, alias) = line.trim().split_once('|')?;
            Some((ip.trim().parse::<Ipv4Addr>().ok()?, alias.trim().to_string()))
        })
        .collect()
}

#[cfg(not(windows))]
fn enumerate_ipv4() -> Vec<Ipv4Addr> {
    Vec::new()
}

/// 绑定端口，撞上「上一个监听器还没释放」时重试一会儿。
///
/// 为什么需要：改端口 / 改监听范围之后是 `stop()` + `start()`，而 stop 只是**发信号**，
/// 旧任务真正退出、把监听套接字丢掉是异步的（毫秒级）。不重试的话，
/// 用户会看到「端口被占用」——明明是自己刚停下来的那个端口（踩过）。
/// Windows 上没有 Unix 那种 SO_REUSEADDR 语义，只能重试。
fn bind_with_retry(addr: &str, port: u16) -> std::io::Result<std::net::TcpListener> {
    let mut last = None;
    for attempt in 0..20 {
        match std::net::TcpListener::bind((addr, port)) {
            Ok(l) => return Ok(l),
            Err(e) => {
                last = Some(e);
                if attempt < 19 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("绑定失败")))
}

/// accept 循环：一个请求一条连接，处理完就关。
async fn serve_loop(
    core: Arc<AppCore>,
    remote: Arc<RemoteService>,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }
        let accepted = tokio::select! {
            _ = shutdown.changed() => break,
            r = listener.accept() => r,
        };
        let (stream, peer) = match accepted {
            Ok(v) => v,
            Err(e) => {
                // 单个连接失败（手机切网、半开连接）不该把整个服务带走
                eprintln!("[remote] 接受连接失败：{e}");
                continue;
            }
        };
        let core = core.clone();
        let remote = remote.clone();
        let shutdown = shutdown.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = handle_conn(core, remote, stream, peer, shutdown).await {
                // 客户端主动断开是常态（手机息屏、切 WiFi），不值得刷日志
                let msg = e.to_string();
                if !msg.contains("断") && !msg.contains("reset") && !msg.contains("os error 10054") {
                    eprintln!("[remote] 连接处理失败（{peer}）：{e}");
                }
            }
        });
    }
    eprintln!("[remote] 已停止");
}

/// 请求阶段的超时：连上却不发请求的连接（端口扫描、浏览器预连接）不该占着任务。
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// SSE 心跳间隔。手机侧 NAT 超时通常 5 分钟起，25 秒足够，也不至于费电。
const SSE_PING: std::time::Duration = std::time::Duration::from_secs(25);

async fn handle_conn(
    core: Arc<AppCore>,
    remote: Arc<RemoteService>,
    mut stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let req = {
        let mut reader = tokio::io::BufReader::new(&mut stream);
        match tokio::time::timeout(REQUEST_TIMEOUT, http::read_request(&mut reader)).await {
            Err(_) => {
                http::write_response(&mut stream, &http::Response::error(400, "请求超时")).await?;
                return Ok(());
            }
            Ok(Err(e)) => {
                http::write_response(&mut stream, &http::Response::error(e.status, e.message)).await?;
                return Ok(());
            }
            Ok(Ok(req)) => req,
        }
    };

    match api::route(&core, &remote, req).await {
        api::Route::Reply(res) => http::write_response(&mut stream, &res).await?,
        api::Route::Sse => {
            http::start_sse(&mut stream).await?;
            run_sse(&core, &remote, &mut stream, shutdown).await?;
        }
    }
    let _ = stream.shutdown().await;
    let _ = peer;
    Ok(())
}

/// 把事件推给一台手机，直到它断开或服务停掉。
async fn run_sse(
    core: &Arc<AppCore>,
    remote: &Arc<RemoteService>,
    stream: &mut tokio::net::TcpStream,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let mut rx = remote.subscribe();
    let count = remote.clients.fetch_add(1, Ordering::Relaxed) + 1;
    remote.push_clients();
    if count == 1 {
        core.toast("info", "手机已连接（远程控制）");
    }

    let mut ping = tokio::time::interval(SSE_PING);
    ping.tick().await; // 第一次 tick 立刻返回，跳过

    let result = loop {
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            _ = ping.tick() => {
                if let Err(e) = http::write_sse_ping(stream).await { break Err(e); }
            }
            frame = rx.recv() => match frame {
                Ok(frame) => {
                    let payload = match serde_json::to_string(&frame) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if let Err(e) = http::write_sse_event(stream, &payload).await { break Err(e); }
                }
                // 追不上就跳帧：页面收到 hello 之外的 Lagged 会自己重拉状态，
                // 卡在这里等反而是把慢手机放大成「整个服务不动」
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break Ok(()),
            },
        }
    };

    let left = remote.clients.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
    remote.push_clients();
    let _ = left;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_and_normalize() {
        assert_eq!(normalize_base("https://a.example.com/"), Some("https://a.example.com".into()));
        assert_eq!(normalize_base("   "), None);
        assert_eq!(normalize_base(""), None);
        assert_eq!(link("http://1.2.3.4:8787/", "tok"), "http://1.2.3.4:8787/?t=tok");
        assert_eq!(link("https://hub.example.com", "tok"), "https://hub.example.com/?t=tok");
    }

    /// 装了 VPN 的机器上，默认路由那块网卡往往不是手机能到的——排序必须把
    /// 家用网段放前面（踩过：二维码里出现 100.64.x.x 的 Tailscale 地址）。
    #[test]
    fn addresses_are_ranked_lan_first() {
        use std::net::Ipv4Addr;
        let c = |o: [u8; 4]| (Ipv4Addr::new(o[0], o[1], o[2], o[3]), String::new());
        let ranked = rank_addresses(
            vec![
                c([100, 64, 12, 3]),  // Tailscale / CGNAT
                c([127, 0, 0, 1]),    // 回环：不要
                c([169, 254, 1, 1]),  // 链路本地：不要
                c([10, 8, 0, 4]),     // 办公网
                c([192, 168, 1, 5]),  // 家用：最优先
                c([172, 20, 3, 9]),   // 私有段
                c([203, 0, 113, 7]),  // 公网地址：最后
            ],
            None,
        );
        let ips: Vec<Ipv4Addr> = ranked.iter().map(|(a, _)| *a).collect();
        assert_eq!(
            ips,
            vec![
                Ipv4Addr::new(192, 168, 1, 5),
                Ipv4Addr::new(10, 8, 0, 4),
                Ipv4Addr::new(172, 20, 3, 9),
                Ipv4Addr::new(100, 64, 12, 3),
                Ipv4Addr::new(203, 0, 113, 7),
            ]
        );
        assert!(rank_addresses(vec![], None).is_empty());
    }

    /// 操作系统默认路由选的那块网卡排第一——哪怕它的网段「看起来更不像局域网」。
    /// 实测踩过：ZeroTier 的 10.x 被按网段规则排到了真正的 WLAN（100.66.x）前面，
    /// 二维码就指向了一个手机连不上的地址。
    #[test]
    fn default_route_address_wins_over_segment_rules() {
        use std::net::Ipv4Addr;
        let zt = (Ipv4Addr::new(10, 45, 70, 92), "ZeroTier One".to_string());
        let wlan = (Ipv4Addr::new(100, 66, 47, 247), "WLAN".to_string());
        let ranked = rank_addresses(vec![zt.clone(), wlan.clone()], Some(wlan.0));
        assert_eq!(ranked[0], wlan);
        assert_eq!(ranked[1], zt);
        // 网卡名要带出来（面板上显示「局域网 · WLAN」）
        assert_eq!(ranked[0].1, "WLAN");
    }

    /// 改端口 / 改监听范围之后是「先停再起」：旧监听器是**异步**释放的，
    /// 不重试就会撞上「端口被占用」——而且占用的还是自己刚停下来的那个端口。
    #[test]
    fn bind_retry_waits_for_the_old_listener() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held); // 模拟旧 accept 循环退出、丢掉监听套接字
        });
        let got = bind_with_retry("127.0.0.1", port).expect("旧监听器释放后应该能绑上");
        assert_eq!(got.local_addr().unwrap().port(), port);
    }

    /// 端口真的被别人占着时，重试要有**上限**（约 1 秒）就报错，
    /// 不能把用户点按钮的那个命令卡在那儿。
    #[test]
    fn bind_retry_gives_up_on_a_taken_port() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let started = std::time::Instant::now();
        assert!(bind_with_retry("127.0.0.1", port).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "重试太久：{:?}", started.elapsed());
    }

    #[test]
    fn frames_serialize_with_kind() {
        let f = Frame::Event {
            name: "hub://toast".into(),
            payload: serde_json::json!({ "level": "info", "message": "hi" }),
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains("\"kind\":\"event\""), "实际：{s}");
        assert!(s.contains("\"name\":\"hub://toast\""), "实际：{s}");
        assert!(s.contains("\"message\":\"hi\""), "实际：{s}");
    }
}
