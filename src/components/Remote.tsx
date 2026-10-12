// 「移动端远程控制」面板：扫码在手机 / 平板上操作这台电脑上的 agent。
//
// 面板要做的事只有三件：
// 1. **把链接给出去**（二维码 + 可复制的地址）——这是用户唯一必须做的事
// 2. **让人看清通不通**（服务在不在跑、连着几台设备、上次为什么失败）
// 3. **配置可达性**（端口 / 监听范围 / 对外地址 / 中继服务器）
//
// 二维码在前端画（qrcode-generator，纯 JS 零依赖）：链接里带令牌，
// 不该让它在别的地方过一道；画成 data URL 的 <img>，不用 dangerouslySetInnerHTML。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import qrcode from "qrcode-generator";
import { api, errText } from "../lib/api";
import type { RemoteDeployEvent, RemoteStatus } from "../lib/types";
import { useApp } from "../store/app";
import { Field, Icon, Modal, Segmented, Spinner, Switch } from "./ui";

/** 链接 → 二维码的 data URL。模块数随内容变，所以 cellSize 按尺寸回推。 */
function qrDataUrl(text: string): string | null {
  if (!text) return null;
  try {
    const qr = qrcode(0, "M");
    qr.addData(text);
    qr.make();
    const count = qr.getModuleCount();
    // 目标约 300px：手机镜头对着屏幕扫，太小会扫不出
    const cell = Math.max(3, Math.floor(300 / (count + 4)));
    return qr.createDataURL(cell, 2);
  } catch {
    // 链接太长（二维码容量有上限）时别把面板整个搞崩，退回「复制链接」
    return null;
  }
}

export function RemoteDialog({ onClose }: { onClose: () => void }) {
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [port, setPort] = useState("");
  const [publicUrl, setPublicUrl] = useState("");
  const [relay, setRelay] = useState("");
  const [showSetup, setShowSetup] = useState(false);
  // 一键安装：服务器地址 / SSH 端口 / 用户名 / 密码（密码只在这次用，不落盘）
  const [ssh, setSsh] = useState({ host: "", port: "22", user: "root", password: "" });
  const [deploying, setDeploying] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const logBox = useRef<HTMLDivElement>(null);
  const timer = useRef<number | null>(null);

  const apply = useCallback((st: RemoteStatus) => {
    setStatus(st);
    setPort(String(st.port));
    setPublicUrl(st.publicUrl ?? "");
    setRelay(st.relay);
    // 服务器地址/用户名回填（安装过一次就不用再敲）
    setSsh((prev) => ({
      host: prev.host || (st.relay ? st.relay.split(":")[0] : ""),
      port: String(st.sshPort || 22),
      user: prev.user === "root" && st.sshUser ? st.sshUser : prev.user || "root",
      password: "",
    }));
  }, []);

  // 安装过程逐行推过来（一次可能几十秒，不能干等）
  useEffect(() => {
    const un = listen<RemoteDeployEvent>("hub://remote-deploy", (e) => {
      setLog((prev) => [...prev.slice(-400), e.payload.line]);
      const box = logBox.current;
      if (box) requestAnimationFrame(() => (box.scrollTop = box.scrollHeight));
      if (e.payload.stage === "done" || e.payload.stage === "failed") setDeploying(false);
    });
    return () => void un.then((f) => f());
  }, []);

  const refresh = useCallback(async () => {
    try {
      apply(await api.remoteStatus());
    } catch (e) {
      toast("error", errText(e));
    }
  }, [apply, toast]);

  useEffect(() => {
    void refresh();
    // 面板开着的时候每 3 秒看一眼：连着几台设备、上次为什么失败都要能看见。
    // 只在面板打开期间轮询，关掉就停（别让它在后台一直打扰后端）。
    timer.current = window.setInterval(() => void refresh(), 3000);
    return () => {
      if (timer.current) window.clearInterval(timer.current);
    };
  }, [refresh]);

  const patch = useCallback(
    async (p: { port?: number; bind?: "lan" | "local"; publicUrl?: string; relay?: string }) => {
      setBusy(true);
      try {
        apply(await api.remoteConfigPatch(p));
      } catch (e) {
        toast("error", errText(e));
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [apply, refresh, toast],
  );

  const toggle = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        apply(await api.remoteSetEnabled(enabled));
        toast("success", enabled ? "远程控制已打开" : "远程控制已关闭");
      } catch (e) {
        toast("error", errText(e));
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [apply, refresh, toast],
  );

  const primaryUrl = status?.urls[0]?.url ?? "";
  const qr = useMemo(() => qrDataUrl(primaryUrl), [primaryUrl]);

  const copy = useCallback(
    async (text: string) => {
      try {
        await navigator.clipboard.writeText(text);
        toast("success", "链接已复制");
      } catch {
        toast("error", "复制失败，请手动选中链接");
      }
    },
    [toast],
  );

  return (
    <Modal
      title="移动端远程控制"
      icon="phone"
      wide
      onClose={onClose}
      footer={
        <>
          <span className="left muted" style={{ fontSize: 11.5 }}>
            消息由这台电脑上的 agent 执行；手机上只能读、发消息与审批，改不了设置。
          </span>
          <button className="btn" onClick={() => void refresh()}>
            <Icon name="refresh" size={13} /> 刷新
          </button>
        </>
      }
    >
      <div className="remote-grid">
        {/* ---------------------------------------------------- 左：扫码连接 */}
        <div className="card-box">
          <div className="remote-card-head">
            <Icon name="phone" size={14} />
            <span className="grow">手机扫码连接</span>
            {status?.running ? (
              <span className="pill ok">
                <span className="dot-live" /> 运行中
              </span>
            ) : (
              <span className="pill">已停止</span>
            )}
          </div>

          <p className="remote-note">
            {status?.running
              ? status.clients > 0
                ? `已连接 ${status.clients} 台设备。`
                : "等待手机连接。用手机扫码，或在手机上打开下面的链接。"
              : "打开后手机才能连上。这是一条能从别的设备操作本机的通道，所以默认关着。"}
          </p>

          {status && !status.running && (
            <button className="btn primary" disabled={busy} onClick={() => void toggle(true)}>
              <Icon name="play" size={13} /> 打开远程控制
            </button>
          )}

          {status?.running && (
            <>
              {qr ? (
                <div className="qr-box">
                  <img src={qr} alt="扫码连接" width={232} height={232} />
                </div>
              ) : (
                <p className="remote-note">链接太长，二维码画不出来（换个短一点的对外地址，或直接复制链接）。</p>
              )}

              <div className="remote-urls">
                {status.urls.map((u, i) => (
                  <div className="remote-url" key={u.url}>
                    <span className="tag">
                      {u.kind === "public" ? "对外" : u.kind === "local" ? "仅本机" : "局域网"}
                      {i === 0 && <span className="muted"> · 二维码</span>}
                    </span>
                    <span className="mono grow ellipsis" title={u.url}>
                      {u.url}
                    </span>
                    <button className="icon-btn" title="复制" onClick={() => void copy(u.url)}>
                      <Icon name="copy" size={13} />
                    </button>
                  </div>
                ))}
              </div>

              {status.bind === "lan" && status.lanUrls.length === 0 && (
                <p className="remote-note warn">没探测到局域网地址——连上 WiFi 后点「刷新」再看。</p>
              )}
              {status.lanUrls.length > 1 && (
                <p className="remote-note">
                  这台机器有多块网卡（有线 / 无线 / VPN），二维码用的是排第一的那个；
                  手机扫了连不上就换一行复制。
                </p>
              )}

              <div className="row" style={{ marginTop: 8 }}>
                <button className="btn" onClick={() => void copy(primaryUrl)}>
                  <Icon name="copy" size={13} /> 复制链接
                </button>
                <button
                  className="btn"
                  title="换一个令牌：旧链接与旧二维码立刻失效，已连上的设备会被断开"
                  onClick={async () => {
                    if (!window.confirm("换一个访问令牌？旧的二维码和链接会立刻失效。")) return;
                    setBusy(true);
                    try {
                      apply(await api.remoteTokenReset("access"));
                      toast("success", "已换新令牌");
                    } catch (e) {
                      toast("error", errText(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  <Icon name="refresh" size={13} /> 换令牌
                </button>
                <div className="spacer" />
                <button className="btn danger" disabled={busy} onClick={() => void toggle(false)}>
                  停止
                </button>
              </div>
            </>
          )}

          {status?.lastError && <p className="remote-note error">上次出错：{status.lastError}</p>}
        </div>

        {/* ---------------------------------------------------- 右：可达性与服务器 */}
        <div className="card-box">
          <div className="remote-card-head">
            <Icon name="plug" size={14} />
            <span className="grow">地址与服务器</span>
            {busy && <Spinner />}
          </div>

          <Field label="监听范围" hint="局域网 = 同一个 WiFi 下的手机直接能连；仅本机 = 只给反代 / 中继用">
            <Segmented
              value={status?.bind ?? "lan"}
              onChange={(v) => void patch({ bind: v })}
              options={[
                { id: "lan", label: "局域网" },
                { id: "local", label: "仅本机" },
              ]}
            />
          </Field>

          <Field label="端口" hint="1024 以上；被占用时换一个（比如 54714）">
            <input
              className="input mono"
              value={port}
              inputMode="numeric"
              onChange={(e) => setPort(e.target.value.replace(/[^\d]/g, ""))}
              onBlur={() => {
                const n = Number(port);
                if (n && n !== status?.port) void patch({ port: n });
              }}
            />
          </Field>

          <Field
            label="对外地址"
            hint="有反代或隧道（nginx / cloudflared / frp）时填它，二维码与链接就用这个域名"
          >
            <input
              className="input mono"
              value={publicUrl}
              placeholder="https://hub.example.com"
              onChange={(e) => setPublicUrl(e.target.value)}
              onBlur={() => {
                if (publicUrl.trim() !== (status?.publicUrl ?? "")) void patch({ publicUrl });
              }}
            />
          </Field>

          <div className="deploy-box">
            <div className="remote-card-head">
              <Icon name="download" size={13} />
              <span className="grow">一键装到我的服务器</span>
              {deploying && <Spinner />}
            </div>
            <p className="remote-note">
              填服务器的 IP、用户名和密码，应用自己 SSH 上去把中继装好、起成常驻服务，
              再把地址填好接上隧道——不用你登服务器敲命令。<b>不需要反代</b>：中继自己听一个端口。
            </p>
            <div className="deploy-grid">
              <input
                className="input mono"
                placeholder="服务器 IP 或域名"
                value={ssh.host}
                onChange={(e) => setSsh({ ...ssh, host: e.target.value })}
              />
              <input
                className="input mono"
                placeholder="22"
                inputMode="numeric"
                value={ssh.port}
                onChange={(e) => setSsh({ ...ssh, port: e.target.value.replace(/[^\d]/g, "") })}
              />
            </div>
            <div className="deploy-grid">
              <input
                className="input mono"
                placeholder="用户名（root 或带 sudo 的账号）"
                value={ssh.user}
                onChange={(e) => setSsh({ ...ssh, user: e.target.value })}
              />
              <input
                className="input"
                type="password"
                placeholder="SSH 密码"
                value={ssh.password}
                onChange={(e) => setSsh({ ...ssh, password: e.target.value })}
              />
            </div>
            <div className="row" style={{ gap: 6 }}>
              <button
                className="btn primary"
                disabled={deploying || !ssh.host.trim() || !ssh.user.trim() || !ssh.password}
                onClick={async () => {
                  setLog([]);
                  setDeploying(true);
                  setLog([`连接 ${ssh.host}（${ssh.user}）…`]);
                  try {
                    const r = await api.remoteDeployRelay(
                      ssh.host.trim(),
                      ssh.user.trim(),
                      ssh.password,
                      Number(ssh.port) || 22,
                    );
                    setSsh((prev) => ({ ...prev, password: "" }));
                    if (r.ok) {
                      toast("success", `中继已装好（${r.node} · ${r.service}）`);
                      await refresh();
                    } else {
                      toast("error", r.error ?? "安装失败");
                    }
                  } catch (e) {
                    toast("error", errText(e));
                  } finally {
                    setDeploying(false);
                  }
                }}
              >
                <Icon name="download" size={13} /> {deploying ? "正在安装…" : "安装并连接"}
              </button>
              {status?.hostKey && (
                <button
                  className="btn"
                  title="换机器 / 重装了系统之后，指纹会变——确认无误后点这里重新信任"
                  onClick={async () => {
                    if (!window.confirm("忘掉记住的服务器指纹？下次连接会重新信任对方。")) return;
                    await api.remoteForgetHostKey().catch(() => {});
                    toast("success", "已忘掉旧指纹");
                    await refresh();
                  }}
                >
                  重新信任服务器
                </button>
              )}
            </div>
            {log.length > 0 && (
              <div className="deploy-log" ref={logBox}>
                {log.map((line, i) => {
                  // 脚本里那几行 LH_* 是给程序看的标记，转成人话再显示；
                  // 成功标记本身不用露脸（成功另有提示）
                  if (line.startsWith("LH_DEPLOY_DONE")) return null;
                  if (line.startsWith("LH_ERROR=")) {
                    return (
                      <div key={i} className="err">
                        ✗ {line.slice("LH_ERROR=".length)}
                      </div>
                    );
                  }
                  if (line.startsWith("LH_NODE=")) return <div key={i}>Node：{line.slice(8)}</div>;
                  if (line.startsWith("LH_SERVICE=")) return <div key={i}>服务：{line.slice(11)}</div>;
                  return <div key={i}>{line}</div>;
                })}
              </div>
            )}
          </div>

          <Field
            label="中继服务器"
            hint="自己填也行：应用主动连出去建隧道，家里不用开端口"
          >
            <input
              className="input mono"
              value={relay}
              placeholder="1.2.3.4:8787"
              onChange={(e) => setRelay(e.target.value)}
              onBlur={() => {
                if (relay.trim() !== (status?.relay ?? "")) void patch({ relay });
              }}
            />
          </Field>

          <div className="row" style={{ gap: 6 }}>
            <span className="muted" style={{ fontSize: 11.5 }}>
              中继口令
            </span>
            <span className="mono grow ellipsis" style={{ fontSize: 11.5 }} title={status?.relayToken}>
              {status?.relayToken ? `${status.relayToken.slice(0, 8)}…` : "—"}
            </span>
            <button className="btn" onClick={() => void copy(status?.relayToken ?? "")}>
              <Icon name="copy" size={13} />
            </button>
            <button
              className="btn"
              onClick={async () => {
                if (!window.confirm("换一个中继口令？服务器上 relay.mjs 的 --token 要跟着改。")) return;
                setBusy(true);
                try {
                  apply(await api.remoteTokenReset("relay"));
                  toast("success", "已换中继口令");
                } catch (e) {
                  toast("error", errText(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              换
            </button>
          </div>

          <button className="btn ghost" style={{ marginTop: 10 }} onClick={() => setShowSetup((v) => !v)}>
            <Icon name={showSetup ? "collapse" : "expand"} size={13} /> 手动装（不想给密码的话）
          </button>

          {showSetup && (
            <div className="remote-setup">
              <p>
                仓库里的 <code className="mono">server/relay.mjs</code> 就是中继（零依赖，Node 直接跑）。
                在服务器上：
              </p>
              <pre className="code-block">
{`# 传到服务器后
node relay.mjs --token ${status?.relayToken ?? "<口令>"} --port 8787
# 想常驻就交给 systemd（docs/REMOTE.md 里有现成的 unit）`}
              </pre>
              <p>
                跑起来之后，把上面的「中继服务器」填成 <code className="mono">服务器IP:8787</code>，
                保存即会重连。中继只做字节搬运，看不懂也改不了你的内容；访问令牌仍由这台电脑校验。
              </p>
              <p className="warn">
                公网直连是明文 HTTP（令牌即口令）。只在自己信任的网络用，或者把中继放到 HTTPS 反代后面，
                再把「对外地址」填成那个 https 域名。
              </p>
            </div>
          )}

          <div className="row" style={{ marginTop: 10 }}>
            <Switch checked={!!status?.enabled} onChange={(v) => void toggle(v)} label="启用远程控制" />
          </div>
          <p className="remote-note">
            打开后随应用启动自动恢复；关掉即停止监听。手机端能做的是：看主题与对话、发消息（含拍照贴图）、
            审批工具调用、中止当前轮。
          </p>
        </div>
      </div>
    </Modal>
  );
}
