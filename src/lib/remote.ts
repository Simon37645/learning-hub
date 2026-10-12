// 远程模式：手机 / 平板浏览器里打开的那份界面。
//
// 判定只有一处：远程服务注入的 shim 会挂上 `window.__LH_REMOTE__`
// （见 `src-tauri/src/remote/web/shim.js`），桌面上永远没有它。
//
// 为什么要判定：**同一份前端产物在两种环境里跑**（这是刻意的，见 AGENTS.md），
// 少数能力只有桌面端有——窗口控制（自绘标题栏那三个按钮）、系统文件选择框、
// 原生子 WebView、拖放导入、把界面快照写回磁盘。这些在远程模式下要么收起来、
// 要么换成浏览器自己的做法（剪贴板、选图、打开链接），否则用户点到只是一个报错。
//
// 判定之外的差异一律不要往组件里塞：能靠 CSS（`html[data-remote]`）解决的
// 就用 CSS，能靠接口层（`lib/api.ts`）解决的就在那一层解决。

export interface RemoteBridge {
  /** 访问令牌（接口、SSE、文件地址都要它） */
  token: string;
  /** SSE 连上了吗（顶栏那个绿点） */
  connected?: boolean;
  /** 连着几台设备 */
  clients?: number;
  /** 工作区内的相对路径 → 能直接塞进 <img>/<iframe> 的地址 */
  fileUrl: (rel: string) => string;
  /** 绝对路径 → 工作区相对路径；不在工作区里返回 null */
  toRelative: (path: string) => string | null;
}

export function remoteBridge(): RemoteBridge | null {
  return (window as unknown as { __LH_REMOTE__?: RemoteBridge }).__LH_REMOTE__ ?? null;
}

/** 现在是「手机远程」还是「电脑上的应用」。 */
export function isRemote(): boolean {
  return remoteBridge() !== null;
}

/**
 * 给远程接口的地址补上令牌。
 *
 * 令牌只能走查询串：`EventSource` 不能自定义请求头，`<img>` / `<iframe>` 也不能。
 * 页面自身的地址里已经带了一个（扫码进来时），shim 也会把它存进 localStorage。
 */
export function withToken(path: string): string {
  const b = remoteBridge();
  if (!b) return path;
  return path + (path.includes("?") ? "&" : "?") + "t=" + encodeURIComponent(b.token);
}

/**
 * 浏览器里选文件（远程模式下替代系统的文件选择框）。
 *
 * 为什么必须换掉：桌面端那个选择框选的是**电脑上**的文件，而人在手机上——
 * 弹出来的是电脑屏幕上的窗口，没人看得见。手机上的图/文件要用浏览器自己的选择器，
 * 选完拿到的是 File（字节就在手里，不用再经后端读一遍）。
 *
 * 取消时不会触发 `change`，所以挂一个一次性的 focus 兜底：
 * 用户点完「取消」回到页面时把 promise 收掉（不然输入框旁边的转圈会一直转）。
 */
export function pickFilesInBrowser(accept: string, multiple = true): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.multiple = multiple;
    input.style.display = "none";
    document.body.appendChild(input);

    const finish = (files: File[]) => {
      input.remove();
      resolve(files);
    };
    input.addEventListener("change", () => finish(Array.from(input.files ?? [])));
    window.addEventListener(
      "focus",
      () => {
        window.setTimeout(() => {
          if (document.body.contains(input)) finish([]);
        }, 400);
      },
      { once: true },
    );
    input.click();
  });
}
