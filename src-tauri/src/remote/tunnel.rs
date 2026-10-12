//! 反向隧道客户端：让手机从**公网**连到这台电脑，而不需要在路由器上做端口映射。
//!
//! 场景：用户自己有台公网服务器（VPS）。服务器上跑 `server/relay.mjs`
//! （仓库里那份，几十行、零依赖），本应用**主动连出去**建几条长连接，
//! 中继把来访的浏览器连接交给其中一条——于是「手机 → 公网 IP → 这台电脑」
//! 这条路就通了。家里的网络不用开任何端口。
//!
//! ```text
//! 手机 ──HTTP──→ 公网服务器 relay.mjs ──GO+原始字节──→ 本进程 ──→ 127.0.0.1:<port>
//!                        ▲
//!                        └── 本进程主动连出去（握手：LH1 <token>）
//! ```
//!
//! ## 协议（一句话就能讲完）
//!
//! ```text
//! 客户端 → 中继：LH1 <token>\n      握手，只此一句
//! 中继 → 客户端：PING\n            空闲时每 30 秒一次（把两边的 NAT 都焐热）
//! 中继 → 客户端：GO\n              配对了，此后双向都是原始字节
//! ```
//!
//! ## 为什么是「连接池」而不是多路复用
//!
//! 中继不区分「哪条连接属于哪个请求」，只是把一条来访连接整条交给一条隧道
//! （配对后就是纯粹的双向搬运）。所以本进程维持若干条**空闲**隧道，
//! 用掉一条补一条。多路复用能省连接，但要自己实现分帧、流控、半关闭，
//! 出问题时很难查——而这个功能要的只是「手机上能用」。
//!
//! 池子由**一个监管循环**统一补（`run`），worker 自己不再开新连接：
//! 唯一的分叉点是「配对成功」——那一刻这条连接会被占住很久（比如 SSE 长连接），
//! 所以立刻通知监管补一条，否则几次并发请求就把池子掏空了（会看到 503）。
//!
//! 方向上也刻意不对称：**空闲时只有中继往这边发 PING**，本进程一个字都不发。
//! 反过来的话（客户端发心跳）会有个竞态：心跳正好落在「中继刚配对、即将开始
//! 原始搬运」的瞬间，那几个字节会被当成 HTTP 请求的一部分灌给本机服务。
//! 只让中继说话就没有这个窗口。

use super::RemoteService;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{watch, Notify};

/// 维持几条空闲隧道。手机上一条 SSE 长连接 + 几个普通请求，6 条有余量
/// （手机浏览器还会开预连接）。
const POOL: usize = 6;
/// 隧道连接总数上限（空闲 + 正在服务）：挡住「一直开新连接」把本机端口耗光
const MAX_TOTAL: usize = 32;
/// 等中继消息的上限（**每次读**都重新计时）。中继每 30 秒 PING 一次，
/// 所以这个超时只在「中继真的没声了」时才触发——连接断了、或者对面不是我们的中继。
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// 连不上时的重试间隔
const RETRY: Duration = Duration::from_secs(3);

/// 起隧道。返回时隧道已停止（停服时循环退出）。
pub async fn run(
    remote: Arc<RemoteService>,
    relay: String,
    token: String,
    local_port: u16,
    mut shutdown: watch::Receiver<bool>,
) {
    if relay.trim().is_empty() || token.trim().is_empty() {
        return;
    }
    eprintln!("[remote] 隧道：连 {relay}（本地 {local_port}）");

    // 还没配对的 worker 数（它们持有的连接正躺在中继的空闲池里）
    let live = Arc::new(AtomicUsize::new(0));
    // 正在搬运字节的 worker 数（配对之后）
    let busy = Arc::new(AtomicUsize::new(0));
    // 「刚配对 / 刚退出，快补一条」的信号
    let topup = Arc::new(Notify::new());

    loop {
        if *shutdown.borrow() {
            break;
        }
        // 先尽量补齐，补到 POOL 条（或撞上总数上限）
        let l = live.load(Ordering::Relaxed);
        if l < POOL && l + busy.load(Ordering::Relaxed) < MAX_TOTAL {
            live.fetch_add(1, Ordering::Relaxed);
            let remote = remote.clone();
            let relay = relay.clone();
            let token = token.clone();
            let shutdown = shutdown.clone();
            let (live, busy, topup) = (live.clone(), busy.clone(), topup.clone());
            tauri::async_runtime::spawn(async move {
                worker(remote, relay, token, local_port, shutdown, live, busy, topup).await;
            });
            continue;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
            // 配对成功 / worker 退出会立刻叫醒这里；剩下的时间靠定时兜底
            _ = topup.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    eprintln!("[remote] 隧道已停止");
}

/// 一个 worker：连上中继 → 等配对 → 服务一次 → 退出（由监管补新的）。
#[allow(clippy::too_many_arguments)]
async fn worker(
    remote: Arc<RemoteService>,
    relay: String,
    token: String,
    local_port: u16,
    mut shutdown: watch::Receiver<bool>,
    live: Arc<AtomicUsize>,
    busy: Arc<AtomicUsize>,
    topup: Arc<Notify>,
) {
    // 连续失败只在第 1 次和每 10 次打一行日志：中继没开的时候别把日志刷满
    let mut failures: u32 = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let outcome = tokio::select! {
            _ = shutdown.changed() => break,
            r = serve_one(&relay, &token, local_port, &live, &busy) => r,
        };
        match outcome {
            Ok(()) => break, // 服务过一次就收工，监管会补一条新的
            Err(e) => {
                failures += 1;
                if failures == 1 || failures % 10 == 0 {
                    eprintln!("[remote] 隧道连不上 {relay}：{e}（第 {failures} 次）");
                    if failures == 1 {
                        remote.note_error(format!("隧道连不上 {relay}：{e}"));
                    }
                }
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tokio::time::sleep(RETRY) => {}
                }
            }
        }
    }
    // 退出前把「我还占着一条空闲隧道」还回去（配对过的那些已经还过了）
    topup.notify_waiters();
}

/// 建一条隧道并服务一次配对。返回 `Ok(())` 表示真的服务了一次。
async fn serve_one(
    relay: &str,
    token: &str,
    local_port: u16,
    live: &Arc<AtomicUsize>,
    busy: &Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let stream = TcpStream::connect(relay).await?;
    let _ = stream.set_nodelay(true);
    let (rh, mut wh) = stream.into_split();
    let mut reader = BufReader::new(rh);
    wh.write_all(format!("LH1 {token}\n").as_bytes()).await?;
    wh.flush().await?;

    // 等中继说 GO（期间会收到若干 PING）。
    //
    // 超时是**每次读**的，不是整体：中继每 30 秒会 PING 一下，所以空闲的隧道
    // 一直「有动静」，不会因为待久了就被自己掐掉重连（之前写成整体超时，
    // 结果池子里每条隧道每 5 分钟就轮换一次，日志里全是假警报）。
    loop {
        let mut line = String::new();
        let read = tokio::time::timeout(IDLE_TIMEOUT, reader.read_line(&mut line))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "中继一直没有消息（PING 也停了）"))??;
        if read == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "中继关闭了连接"));
        }
        match line.trim() {
            "GO" => break,
            "PING" => continue,
            // 中继只会说这两句；别的话说明对面不是我们的中继
            //（最常见的原因：端口填成了别的服务）
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("中继回了一句看不懂的话：{other}"),
                ))
            }
        }
    }

    // 配对成功：这条连接被占住了，先记账（监管据此补一条新的空闲隧道）
    live.fetch_sub(1, Ordering::Relaxed);
    busy.fetch_add(1, Ordering::Relaxed);

    // 连本机服务，双向搬运。注意用的是同一个 `reader`（它可能已经把来访请求的
    // 头几个字节读进缓冲了），换成裸 socket 会把那几个字节丢掉。
    let local = TcpStream::connect(("127.0.0.1", local_port)).await?;
    let _ = local.set_nodelay(true);
    let (lr, mut lw) = local.into_split();

    let to_local = async move {
        let mut r = reader;
        let res = tokio::io::copy(&mut r, &mut lw).await;
        // 来访者关了（手机息屏 / 请求结束）→ 让本机服务看到 EOF
        let _ = lw.shutdown().await;
        res
    };
    let to_remote = async move {
        let mut r = lr;
        let res = tokio::io::copy(&mut r, &mut wh).await;
        let _ = wh.shutdown().await;
        res
    };
    let _ = tokio::join!(to_local, to_remote);
    busy.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// 中继侧的最小仿真：按协议握手 → PING/GO → 双向搬运到目标端口。
    /// 这同时是「协议能不能跑通」的回归网——协议只有三句话，
    /// 但方向写反、或者少了一次半关闭，症状都是「手机上一直转圈」。
    #[tokio::test]
    async fn tunnel_round_trip_through_fake_relay() {
        // 目标服务（相当于本机的远程 HTTP 服务）：读一次，回一句
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = target.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let _ = sock
                        .write_all(format!("echo:{}", String::from_utf8_lossy(&buf[..n])).as_bytes())
                        .await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        // 假中继
        let relay = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_port = relay.local_addr().unwrap().port();
        let relay_task = tokio::spawn(async move {
            let (tunnel, _) = relay.accept().await.unwrap();
            let (rh, wh) = tunnel.into_split();
            let mut reader = BufReader::new(rh);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("LH1 "), "握手行：{line}");

            // 来访的浏览器（真实中继是先收到它才开始配对的）
            let (visitor, _) = relay.accept().await.unwrap();
            let (vr, mut vw) = visitor.into_split();

            let mut wh = wh;
            // 空闲时先说一句 PING：客户端应当忽略它，只认 GO
            wh.write_all(b"PING\n").await.unwrap();
            wh.write_all(b"GO\n").await.unwrap();

            let (mut r1, mut r2) = (reader, vr);
            let mut up = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut r2, &mut wh).await;
                let _ = wh.shutdown().await;
            });
            // 一个方向结束就收摊（真实中继同此语义），否则浏览器等不到 EOF
            let down = async {
                let _ = tokio::io::copy(&mut r1, &mut vw).await;
                let _ = vw.shutdown().await;
            };
            tokio::pin!(down);
            tokio::select! {
                _ = &mut down => { up.abort(); }
                _ = &mut up => { let _ = down.await; }
            }
        });

        // 浏览器端：连中继、发请求、把回包读干净
        let browser = tokio::spawn(async move {
            // 等一下让中继先接受隧道那条连接（真实场景里手机是扫码后才连的）
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", relay_port)).await.unwrap();
            sock.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
            let mut buf = Vec::new();
            let _ = sock.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        });

        let live = Arc::new(AtomicUsize::new(1));
        let busy = Arc::new(AtomicUsize::new(0));
        serve_one(&format!("127.0.0.1:{relay_port}"), "tok", target_port, &live, &busy)
            .await
            .unwrap();
        let response = browser.await.unwrap();
        assert!(response.contains("echo:GET / HTTP/1.1"), "实际：{response}");
        // 配对之后这条连接就不算「空闲」了：监管据此补新的一条
        assert_eq!(live.load(Ordering::Relaxed), 0);
        assert_eq!(busy.load(Ordering::Relaxed), 0);
        let _ = relay_task.await;
    }
}
