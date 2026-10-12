/* 学习中枢 · 远程页面桥
 *
 * 手机浏览器里跑的是**桌面端同一份前端产物**（`dist/` 直接由远程服务送出去），
 * 所以这一小段脚本的唯一任务就是：让那份产物以为自己在 Tauri 里。
 *
 * 替换的东西只有一样：`window.__TAURI_INTERNALS__`（Tauri 的 IPC 出口）。
 * - `invoke` → POST /api/invoke（Rust 侧查白名单，再交给电脑上的前端执行）
 * - 事件 → SSE（`/api/events`），按事件名分派给 `listen()` 注册的回调
 * - `convertFileSrc` → /api/file（工作区内的文件，带令牌）
 * - 少数插件命令（剪贴板 / 打开链接 / 系统信息）直接用浏览器能力顶上
 *
 * 为什么用「注入 shim」而不是给前端加一套传输层抽象：见 AGENTS.md 那条
 * 「远程的 UI 就是桌面端的 UI」——一处适配，所有组件、所有 invoke 调用点
 * 都不用改，也不会随时间漂移。
 *
 * 这个文件是 `include_str!` 编进二进制的，改完要重新编译（不是热更新）。
 */
(function () {
  "use strict";

  var TOKEN_KEY = "hub.remote.token";
  var token = "";
  try {
    var fromQuery = new URLSearchParams(location.search).get("t");
    if (fromQuery) {
      token = fromQuery;
      localStorage.setItem(TOKEN_KEY, fromQuery);
    } else {
      token = localStorage.getItem(TOKEN_KEY) || "";
    }
  } catch (e) {
    /* 隐私模式下 localStorage 会抛，令牌就只用查询串里的那个 */
  }

  // 工作区根目录：从 bootstrap / config_get 的返回里顺手记下来，
  // 用来把前端算出来的**绝对路径**（消息里的图片走 assetUrl 那条路）
  // 折回工作区相对路径——/api/file 只认相对路径（越界一律拒）。
  var workspaceRoot = "";

  function fileUrl(rel) {
    return "/api/file?path=" + encodeURIComponent(rel) + "&t=" + encodeURIComponent(token);
  }

  /** 绝对路径 → 工作区相对路径；不在工作区里就返回 null。 */
  function toRelative(path) {
    if (!path) return null;
    var norm = String(path).replace(/\\/g, "/");
    if (!workspaceRoot) return norm.charAt(0) === "/" || /^[a-zA-Z]:/.test(norm) ? null : norm;
    var root = workspaceRoot.replace(/\\/g, "/").replace(/\/+$/, "");
    // Windows 上盘符大小写不敏感，比较前统一
    if (norm.toLowerCase().indexOf(root.toLowerCase()) !== 0) return null;
    return norm.slice(root.length).replace(/^\/+/, "");
  }

  var remote = {
    token: token,
    fileUrl: fileUrl,
    toRelative: toRelative,
    /** 页面自己也要用它拼资源地址（见 Chat.tsx 的 assetUrl） */
    ready: true,
  };
  window.__LH_REMOTE__ = remote;
  document.documentElement.setAttribute("data-remote", "1");

  if (!token) {
    fatal("这个页面需要访问令牌：请在电脑上打开「远程控制」，重新扫一次二维码。");
    return;
  }

  // ------------------------------------------------------------ 致命错误提示
  // 页面还没起来就没法用应用自己的 Toast：直接铺一层最简单的提示。
  function fatal(message) {
    function paint() {
      var box = document.createElement("div");
      box.setAttribute("data-lh-fatal", "1");
      box.style.cssText =
        "position:fixed;inset:0;z-index:9999;display:grid;place-items:center;padding:24px;" +
        "background:#12131a;color:#e6e6e6;font:14px/1.7 system-ui,-apple-system,'Segoe UI',sans-serif;text-align:center";
      box.textContent = message;
      document.body.appendChild(box);
    }
    if (document.body) paint();
    else document.addEventListener("DOMContentLoaded", paint);
  }

  // ------------------------------------------------------------ 回调登记表
  // Tauri 的 JS 侧用 transformCallback 把函数登记成一个数字 id，
  // 再把 id 交给 Rust；事件回来时按 id 找函数。这里照搬同一套约定。
  var callbackSeq = 0;
  var callbacks = new Map();

  function transformCallback(callback, once) {
    var id = ++callbackSeq;
    function wrapper() {
      if (once) callbacks.delete(id);
      if (typeof callback === "function") callback.apply(null, arguments);
    }
    callbacks.set(id, wrapper);
    return id;
  }

  function unregisterCallback(id) {
    callbacks.delete(id);
  }

  function runCallback(id, data) {
    var fn = callbacks.get(id);
    if (fn) fn(data);
  }

  // ------------------------------------------------------------ 事件
  var listeners = new Map(); // 事件名 → [{id, handlerId}]
  var eventSeq = 0;
  var eventIndex = new Map(); // 事件 id → {event, handlerId}

  function listenLocal(name, handlerId) {
    var id = ++eventSeq;
    var list = listeners.get(name) || [];
    list.push({ id: id, handlerId: handlerId });
    listeners.set(name, list);
    eventIndex.set(id, { event: name, handlerId: handlerId });
    return id;
  }

  function unlistenLocal(eventId) {
    var entry = eventIndex.get(eventId);
    if (!entry) return;
    eventIndex.delete(eventId);
    var list = listeners.get(entry.event) || [];
    listeners.set(
      entry.event,
      list.filter(function (l) {
        return l.id !== eventId;
      }),
    );
    unregisterCallback(entry.handlerId);
  }

  /** 把一个事件派发给页面里所有监听它的回调。 */
  function dispatch(name, payload) {
    var list = listeners.get(name);
    if (!list || list.length === 0) return;
    // 复制一份再遍历：回调里可能自己 unlisten（once 就是这么实现的）
    list.slice().forEach(function (l) {
      runCallback(l.handlerId, { event: name, id: l.id, payload: payload });
    });
  }

  var es = null;
  var connected = false;

  function connect() {
    es = new EventSource("/api/events?t=" + encodeURIComponent(token));
    es.onopen = function () {
      // 重连之后可能漏掉了事件（手机息屏、切网）：让页面重拉一次当前状态。
      // 首次连接不用（页面自己正在 bootstrap）。
      if (connected) dispatch("hub://reconnect", { at: Date.now() });
      connected = true;
      remote.connected = true;
    };
    es.onerror = function () {
      remote.connected = false;
      // EventSource 自己会重连，这里不用管；页面顶栏的绿点会变灰
    };
    es.onmessage = function (msg) {
      var frame = null;
      try {
        frame = JSON.parse(msg.data);
      } catch (e) {
        return;
      }
      if (!frame) return;
      if (frame.kind === "event") dispatch(frame.name, frame.payload);
      else if (frame.kind === "clients") remote.clients = frame.count;
    };
  }

  // ------------------------------------------------------------ IPC
  function post(path, body) {
    return fetch(path + (path.indexOf("?") >= 0 ? "&" : "?") + "t=" + encodeURIComponent(token), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }).then(function (res) {
      return res.json().then(
        function (data) {
          return { status: res.status, data: data };
        },
        function () {
          return { status: res.status, data: null };
        },
      );
    });
  }

  /** 少数插件命令用浏览器自己的能力顶上；其余插件命令一律明确拒绝。 */
  function pluginLocal(cmd, args) {
    args = args || {};
    if (cmd === "plugin:clipboard-manager|write_text") return writeClipboard(String(args.text || ""));
    if (cmd === "plugin:clipboard-manager|read_text") {
      if (navigator.clipboard && navigator.clipboard.readText) return navigator.clipboard.readText();
      return Promise.reject("这个浏览器不给网页读剪贴板（不是 https 打开的时候通常会这样）");
    }
    if (cmd === "plugin:opener|open_url") {
      window.open(String(args.url || ""), "_blank", "noopener,noreferrer");
      return Promise.resolve(null);
    }
    return Promise.reject(
      "远程模式下不能用这个功能：" + cmd + "（它只能在电脑上操作）",
    );
  }

  /** 剪贴板：http 打开的页面没有 navigator.clipboard，退回老办法。 */
  function writeClipboard(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      try {
        var ta = document.createElement("textarea");
        ta.value = text;
        ta.style.cssText = "position:fixed;opacity:0";
        document.body.appendChild(ta);
        ta.select();
        var ok = document.execCommand("copy");
        document.body.removeChild(ta);
        ok ? resolve(null) : reject("复制失败");
      } catch (e) {
        reject("复制失败：" + e);
      }
    });
  }

  function invoke(cmd, args) {
    args = args || {};
    if (cmd === "plugin:event|listen") {
      return Promise.resolve(listenLocal(String(args.event || ""), args.handler));
    }
    if (cmd === "plugin:event|unlisten") {
      unlistenLocal(args.eventId);
      return Promise.resolve(null);
    }
    // 页面自己 emit 的事件没有别的接收者（远程只有这一份前端），
    // 直接当成功——但别静默丢掉，调试时有用。
    if (cmd === "plugin:event|emit" || cmd === "plugin:event|emit_to") {
      console.debug("[remote] 忽略前端 emit：", args.event);
      return Promise.resolve(null);
    }
    if (cmd.indexOf("plugin:") === 0) return pluginLocal(cmd, args);

    return post("/api/invoke", { cmd: cmd, args: args }).then(function (res) {
      var data = res.data;
      if (!data) throw "电脑那边没有回话（" + res.status + "）";
      if (!data.ok) throw data.error || "执行失败";
      // 顺手记住工作区根目录：图片地址要用它把绝对路径折成相对路径
      if (cmd === "app_bootstrap" || cmd === "config_get") {
        var cfg = cmd === "app_bootstrap" ? data.value && data.value.config : data.value;
        if (cfg && cfg.workspaceRoot) workspaceRoot = String(cfg.workspaceRoot);
      }
      return data.value;
    });
  }

  window.__TAURI_INTERNALS__ = {
    invoke: invoke,
    transformCallback: transformCallback,
    unregisterCallback: unregisterCallback,
    runCallback: runCallback,
    callbacks: callbacks,
    convertFileSrc: function (path) {
      var rel = toRelative(path);
      if (rel === null) {
        // 工作区之外的文件（比如系统字体）在远程没有出口，给一个 1x1 的空图，
        // 别让 <img> 直接报错（onerror 会弹提示，看着像应用坏了）
        return "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
      }
      return fileUrl(rel);
    },
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
    // 插件自己的出口（@tauri-apps/plugin-* 读这几个全局）
    plugins: {},
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: function (_event, id) {
      unlistenLocal(id);
    },
  };
  // 编辑器的平台判断（`inknote/lib/platform.ts`）：按**浏览器的**平台来，
  // 决定显示 Ctrl 还是 ⌘——手机上没有键盘，这条只影响提示文案。
  var mac = /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
  window.__TAURI_OS_PLUGIN_INTERNALS__ = {
    platform: mac ? "macos" : "windows",
    family: mac ? "unix" : "windows",
    os_type: mac ? "macos" : "windows",
    version: "",
    arch: "",
    eol: mac ? "\n" : "\r\n",
    exe_extension: mac ? "" : "exe",
  };

  // ------------------------------------------------------------ 起连接
  // 页面加载完再连：SSE 一开就会推帧，太早连上没人接。
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", connect);
  } else {
    connect();
  }

  // 往页面上拖文件在浏览器里意味着「导航到那个文件」——界面会被整个换掉。
  // 远程页面不接这条路（导入资料本来也只在电脑上做），拦掉默认行为。
  ["dragover", "drop"].forEach(function (type) {
    window.addEventListener(type, function (e) {
      e.preventDefault();
    });
  });
})();
