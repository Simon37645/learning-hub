#!/usr/bin/env node
// 学习中枢 · 远程中继（跑在**你自己的**公网服务器上）
//
// 为什么需要它：家里的电脑在路由器后面，手机从外面连不进去。这台公网服务器
// 做一根管子——电脑主动连出来（所以家里不用开任何端口），手机连到公网 IP，
// 中继把两边接上。
//
//   node relay.mjs --token <口令> [--port 54713] [--bind 0.0.0.0] [--pool 32]
//
// 协议（三句话，别加东西）：
//   电脑 → 中继：`LH1 <口令>\n`   握手，只此一句
//   中继 → 电脑：`PING\n`         空闲时每 30 秒一次（把两边 NAT 都焐热）
//   中继 → 电脑：`GO\n`           配对了，此后双向都是原始字节
//
// 配对之后中继**不解析任何 HTTP**：它只是根管子，看不懂内容也就没有额外的
// 攻击面。访问令牌由电脑那边的学习中枢自己校验，这里只管隧道口令。
//
// 零依赖（只用 node 内置模块）：服务器上 `node relay.mjs` 就能跑，
// 不用 npm install，也不会因为依赖升级哪天突然起不来。
//
// 建议用 systemd 常驻（仓库 docs/REMOTE.md 里有一份现成的 unit）。

import net from "node:net";
import { timingSafeEqual } from "node:crypto";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: {
    token: { type: "string" },
    port: { type: "string" },
    bind: { type: "string" },
    pool: { type: "string" },
    quiet: { type: "boolean" },
  },
});

const TOKEN = (values.token || process.env.LH_RELAY_TOKEN || "").trim();
if (!TOKEN) {
  console.error("缺少隧道口令。用法：node relay.mjs --token <口令> [--port 54713]");
  process.exit(1);
}
// 默认端口刻意不常见：8787 是 RStudio Server 的，5000/8000/8080 满地都是
const PORT = Number(values.port || process.env.LH_RELAY_PORT || 54713);
const BIND = values.bind || process.env.LH_RELAY_BIND || "0.0.0.0";
const MAX_POOL = Number(values.pool || 32);
const QUIET = !!values.quiet;
/** 空闲隧道多久没动静就当它死了（家里的 NAT 会悄悄断开空闲连接） */
const IDLE_MS = 6 * 60 * 1000;
/** 空闲时给电脑发心跳的间隔：比 IDLE_MS 小得多，保证连接一直是「热」的 */
const PING_MS = 30 * 1000;

function log(...args) {
  if (!QUIET) console.log(new Date().toISOString().slice(11, 19), ...args);
}

function tokenOk(got) {
  const a = Buffer.from(TOKEN);
  const b = Buffer.from(String(got || ""));
  if (a.length !== b.length) return false;
  return timingSafeEqual(a, b);
}

/** 空闲隧道池。中继不区分「哪条连接属于哪个请求」：配对后整条交给对方。 */
const idle = [];
let served = 0;

const server = net.createServer((sock) => {
  sock.setNoDelay(true);
  sock.on("error", () => {}); // 单个连接出错不该把进程带走
  classify(sock);
});

/** 看第一行是什么，决定这条连接是「电脑」还是「来访的浏览器」。 */
function classify(sock) {
  let buf = Buffer.alloc(0);
  const onData = (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    const nl = buf.indexOf(10);
    if (nl < 0) {
      // 握手行只有几十字节；超了说明这压根不是我们的客户端
      if (buf.length > 64) {
        sock.off("data", onData);
        handlePublic(sock, buf);
      }
      return;
    }
    const line = buf.subarray(0, nl).toString("utf8").trim();
    sock.off("data", onData);
    if (line.startsWith("LH1 ")) handleTunnel(sock, line.slice(4).trim());
    else handlePublic(sock, buf);
  };
  sock.on("data", onData);
  sock.on("close", () => sock.off("data", onData));
}

function handleTunnel(sock, got) {
  if (!tokenOk(got)) {
    // 不回任何内容：不给猜口令的人任何反馈
    sock.destroy();
    return;
  }
  if (idle.length >= MAX_POOL) {
    log(`隧道已满（${MAX_POOL} 条），拒绝一条新隧道`);
    sock.destroy();
    return;
  }
  const entry = { sock, alive: true, timer: null, since: Date.now() };
  entry.timer = setInterval(() => {
    if (!entry.alive) return;
    if (Date.now() - entry.since > IDLE_MS) {
      log("空闲隧道太久没动静，断开（让电脑重连一条新的）");
      sock.destroy();
      return;
    }
    sock.write("PING\n");
  }, PING_MS);
  sock.on("close", () => {
    entry.alive = false;
    clearInterval(entry.timer);
    const i = idle.indexOf(entry);
    if (i >= 0) idle.splice(i, 1);
  });
  idle.push(entry);
  log(`电脑接上了（空闲隧道 ${idle.length} 条）`);
}

function handlePublic(sock, initial) {
  const entry = idle.shift();
  if (!entry) {
    // 没人接：给一句人话，而不是让浏览器干等
    const body = Buffer.from("电脑上的学习中枢没有连上来。请确认它开着「远程控制」，且中继地址与口令一致。", "utf8");
    sock.end(
      "HTTP/1.1 503 Service Unavailable\r\n" +
        "Content-Type: text/plain; charset=utf-8\r\n" +
        `Content-Length: ${body.length}\r\n` +
        "Connection: close\r\n\r\n" +
        body.toString("utf8"),
    );
    log("有来访者，但池子里没有隧道（电脑没连上来？）");
    return;
  }
  entry.alive = false;
  clearInterval(entry.timer);
  const tunnel = entry.sock;

  // 顺序要紧：先 GO、再把已经读到的请求字节补上，然后才开始搬运。
  // 这样电脑那边「GO 之后收到的就是纯请求」，不会把握手行当成 HTTP 的一部分。
  tunnel.write("GO\n");
  if (initial && initial.length) tunnel.write(initial);

  tunnel.pipe(sock);
  sock.pipe(tunnel);

  const teardown = () => {
    sock.destroy();
    tunnel.destroy();
  };
  sock.on("close", teardown);
  tunnel.on("close", teardown);
  sock.on("error", teardown);
  tunnel.on("error", teardown);

  served++;
  log(`配对成功（第 ${served} 次），空闲隧道还剩 ${idle.length} 条`);
}

server.listen(PORT, BIND, () => {
  log(`学习中枢中继已启动：${BIND}:${PORT}（口令已设置，最长 ${MAX_POOL} 条隧道）`);
});

server.on("error", (e) => {
  console.error("中继起不来：", e.message);
  process.exit(1);
});

for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    log("收到退出信号，收摊");
    server.close(() => process.exit(0));
    for (const e of idle.splice(0)) e.sock.destroy();
    setTimeout(() => process.exit(0), 500).unref();
  });
}
