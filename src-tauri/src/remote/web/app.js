/* 学习中枢 · 移动端远程控制（前端）
 *
 * 这是一个**独立页面**，由桌面端进程里的迷你 HTTP 服务发出来，跑在手机浏览器里。
 * 它不共享桌面端的任何代码：手机上没有 Tauri、没有 IPC，只有 fetch + EventSource。
 *
 * 三条主线：
 * 1. **上行**：`POST /api/*`（发消息 / 审批 / 取消），全部带访问令牌
 * 2. **下行**：`GET /api/events` 一条 SSE 长连接，帧就是桌面端那套 AgentEvent
 * 3. **自己的状态**：当前主题 / 当前对话存在 localStorage（远程是独立的会话，
 *    不抢桌面端「当前正在看哪条」——两边各看各的，但写的是同一份 jsonl）
 *
 * 安全上只做一件事但必须做对：**所有模型/文件内容都先转义再进 DOM**。
 * 这个页面能被任何拿到令牌的浏览器打开，而模型输出里可能出现 HTML。
 */
(function () {
  "use strict";

  // ---------------------------------------------------------------- 小工具

  var $ = function (id) { return document.getElementById(id); };

  var ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return ESC[c]; });
  }

  function toast(message, level) {
    var wrap = $("toasts");
    var el = document.createElement("div");
    el.className = "toast" + (level ? " " + level : "");
    el.textContent = message;
    wrap.appendChild(el);
    setTimeout(function () { el.remove(); }, level === "error" ? 6000 : 3000);
  }

  // ---------------------------------------------------------------- Markdown

  /* 精简的 Markdown 渲染器：标题 / 列表（含缩进嵌套）/ 引用 / 围栏代码 /
     表格 / 分隔线 / 段落 + 行内（代码、粗斜体、删除线、链接、图片、$$公式）。
     没有 KaTeX 与 Mermaid：手机上先把「字能看清、代码能读」做到位，
     公式按等宽块原样显示（比乱码好，也比引一整套渲染器轻）。 */

  function fileUrl(path) {
    return "/api/file?path=" + encodeURIComponent(path) + "&t=" + encodeURIComponent(S.token);
  }

  function safeHref(href) {
    var h = String(href || "").replace(/&amp;/g, "&").trim();
    if (/^https?:\/\//i.test(h) || h.charAt(0) === "/") return h;
    return null;
  }

  function inline(raw) {
    var codes = [];
    // 行内代码先摘出来：里面的 * _ ` 不该再被当成格式
    var s = raw.replace(/`([^`]+)`/g, function (_, c) {
      codes.push(c);
      return "\u0000" + (codes.length - 1) + "\u0000";
    });
    s = s.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g, function (_, alt, src) {
      var h = safeHref(src);
      if (!h) return alt;
      return '<img src="' + esc(h) + '" alt="' + esc(alt) + '" loading="lazy">';
    });
    s = s.replace(/\[([^\]]*)\]\(([^)\s]+)\)/g, function (_, label, href) {
      var h = safeHref(href);
      if (!h) return label;
      return '<a href="' + esc(h) + '" target="_blank" rel="noopener noreferrer">' + label + "</a>";
    });
    s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/(^|[^*\w])\*([^*\n]+)\*/g, "$1<em>$2</em>");
    s = s.replace(/~~([^~]+)~~/g, "<del>$1</del>");
    s = s.replace(/\u0000(\d+)\u0000/g, function (_, n) {
      return "<code>" + codes[+n] + "</code>";
    });
    return s;
  }

  var RE_FENCE = /^\s*(`{3,}|~{3,})\s*([^\s`]*)\s*$/;
  var RE_HEAD = /^(#{1,6})\s+(.*)$/;
  var RE_HR = /^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/;
  var RE_QUOTE = /^\s*>\s?(.*)$/;
  var RE_ITEM = /^(\s*)(?:([-*+])|(\d+)[.)])\s+(.*)$/;
  var RE_TABLE_SEP = /^\s*\|?[\s:|-]+\|[\s:|-]*$/;

  function renderMarkdown(src) {
    var lines = String(src == null ? "" : src).replace(/\r\n?/g, "\n").split("\n");
    var out = [];
    var i = 0;

    function flushParagraph(buf) {
      if (!buf.length) return;
      out.push("<p>" + inline(esc(buf.join("\n")).replace(/\n/g, "<br>")) + "</p>");
      buf.length = 0;
    }

    var para = [];
    while (i < lines.length) {
      var line = lines[i];

      var fence = RE_FENCE.exec(line);
      if (fence) {
        flushParagraph(para);
        var mark = fence[1].charAt(0);
        var body = [];
        i++;
        while (i < lines.length) {
          var cur = lines[i];
          if (RE_FENCE.test(cur) && cur.trim().charAt(0) === mark) { i++; break; }
          body.push(cur);
          i++;
        }
        var lang = (fence[2] || "").toLowerCase();
        var raw = body.join("\n");
        var code = esc(raw);
        if (lang === "html" || lang === "htm" || lang === "svg") {
          // 演示卡片：和桌面端同一套约定——sandbox="" 的 iframe，脚本不执行
          out.push(demoCardHtml(lang, raw));
        } else if (lang === "mermaid") {
          // 图交给**桌面端**渲染（那边有唯一一份 mermaid 入口），
          // 这里先摆源码，画好了原地替换（见 hydrateRichContent）
          out.push('<div class="diagram" data-mermaid="1"><pre class="code"><code>' + code + "</code></pre></div>");
        } else if (lang === "math") {
          out.push('<div class="math">' + code + "</div>");
        } else {
          out.push('<pre class="code"><code>' + code + "</code></pre>");
        }
        continue;
      }

      if (!line.trim()) { flushParagraph(para); i++; continue; }

      var head = RE_HEAD.exec(line);
      if (head) {
        flushParagraph(para);
        var lvl = head[1].length;
        out.push("<h" + lvl + ">" + inline(esc(head[2])) + "</h" + lvl + ">");
        i++;
        continue;
      }

      if (RE_HR.test(line)) { flushParagraph(para); out.push("<hr>"); i++; continue; }

      // $$ 公式块（多行）：按等宽块显示，别把 LaTeX 当正文揉成一团
      if (/^\s*\$\$/.test(line)) {
        flushParagraph(para);
        var math = [];
        var first = line.replace(/^\s*\$\$\s*/, "");
        if (/\$\$\s*$/.test(first) && first.trim()) {
          math.push(first.replace(/\$\$\s*$/, ""));
          i++;
        } else {
          if (first.trim()) math.push(first);
          i++;
          while (i < lines.length && !/\$\$\s*$/.test(lines[i])) { math.push(lines[i]); i++; }
          if (i < lines.length) { math.push(lines[i].replace(/\$\$\s*$/, "")); i++; }
        }
        out.push('<div class="math">' + esc(math.join("\n")) + "</div>");
        continue;
      }

      if (RE_QUOTE.test(line)) {
        flushParagraph(para);
        var quote = [];
        while (i < lines.length && RE_QUOTE.test(lines[i])) {
          quote.push(RE_QUOTE.exec(lines[i])[1]);
          i++;
        }
        out.push("<blockquote>" + renderMarkdown(quote.join("\n")) + "</blockquote>");
        continue;
      }

      if (RE_ITEM.test(line)) {
        flushParagraph(para);
        var block = [];
        while (i < lines.length && (RE_ITEM.test(lines[i]) || (lines[i].trim() && /^\s{2,}/.test(lines[i])))) {
          block.push(lines[i]);
          i++;
        }
        out.push(renderList(block));
        continue;
      }

      // 表格：一行带 | 且下一行是分隔行
      if (line.indexOf("|") >= 0 && i + 1 < lines.length && RE_TABLE_SEP.test(lines[i + 1])) {
        flushParagraph(para);
        var head2 = splitRow(line);
        i += 2;
        var rows = [];
        while (i < lines.length && lines[i].indexOf("|") >= 0) { rows.push(splitRow(lines[i])); i++; }
        var t = "<table><thead><tr>";
        head2.forEach(function (c) { t += "<th>" + inline(esc(c)) + "</th>"; });
        t += "</tr></thead><tbody>";
        rows.forEach(function (r) {
          t += "<tr>";
          for (var k = 0; k < head2.length; k++) t += "<td>" + inline(esc(r[k] || "")) + "</td>";
          t += "</tr>";
        });
        out.push(t + "</tbody></table>");
        continue;
      }

      para.push(line);
      i++;
    }
    flushParagraph(para);
    return out.join("");
  }

  /* 演示卡片：模型在回复里放一个 ```html / ```svg 代码块就是一张演示。
     卡片是 sandbox="" 的 iframe——脚本不执行，只能靠 CSS 动画 / SVG SMIL 动。
     和桌面端同一套约定（那边会标「脚本已禁用」，这里也标）。 */
  function demoCardHtml(lang, source) {
    var doc =
      lang === "svg"
        ? '<!doctype html><html><head><meta charset="utf-8"><style>' +
          "html,body{margin:0;height:100%;display:grid;place-items:center;overflow:hidden}" +
          "svg{max-width:100%;max-height:100%}</style></head><body>" +
          source +
          "</body></html>"
        : source;
    var warn = /<script[\s>]/i.test(source) ? '<span class="demo-warn">脚本已禁用</span>' : "";
    return (
      '<div class="demo"><div class="demo-head"><span class="demo-label">演示 · ' +
      esc(lang.toUpperCase()) +
      "</span>" +
      warn +
      '<span class="grow"></span>' +
      '<button class="demo-zoom" data-demo-zoom="1">放大</button></div>' +
      '<iframe class="demo-frame" sandbox="" loading="lazy" srcdoc="' +
      esc(doc) +
      '"></iframe></div>'
    );
  }

  /* Mermaid 的 SVG 来自桌面端（那边 securityLevel: strict 已经把标签净化过），
     这里再挡一道，去掉能执行或能外联的东西。

     **不能删 foreignObject**：mermaid 的节点文字默认是 HTML 标签，就装在里面，
     删了图还在、字没了（实测踩过：一排彩色方块，没有文字）。 */
  function sanitizeSvg(svg) {
    return String(svg)
      .replace(/<script[\s\S]*?<\/script>/gi, "")
      .replace(/<(iframe|object|embed|link|meta|base|form|input|button)\b[^>]*>/gi, "")
      .replace(/\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "")
      .replace(/(href|xlink:href)\s*=\s*("|')?\s*(javascript|data:text\/html|vbscript):[^"'\s>]*/gi, "");
  }

  /* 渲染完 HTML 之后补一步：把还没画出来的 Mermaid 图请桌面端画一下。
     异步、原地替换——画不出来就留着源码并说明原因，绝不留白板。 */
  function hydrateRichContent(root) {
    if (!root) return;
    var slots = root.querySelectorAll('[data-mermaid="1"]:not([data-state])');
    Array.prototype.forEach.call(slots, function (slot) {
      slot.setAttribute("data-state", "loading");
      var source = slot.textContent || "";
      api("/api/render", { method: "POST", body: { lang: "mermaid", source: source } })
        .then(function (r) {
          if (!slot.isConnected) return;
          slot.setAttribute("data-state", "done");
          slot.innerHTML = sanitizeSvg(r.svg || "");
        })
        .catch(function (e) {
          if (!slot.isConnected) return;
          slot.setAttribute("data-state", "failed");
          slot.innerHTML =
            slot.innerHTML + '<div class="diagram-note">这张图没画出来（' + esc(e.message) + "），在电脑上能看到图。</div>";
        });
    });
  }

  function splitRow(line) {
    var t = line.trim().replace(/^\|/, "").replace(/\|$/, "");
    return t.split("|").map(function (c) { return c.trim(); });
  }

  /* 列表：按缩进（2 空格一级）分成嵌套的 ul/ol。 */
  function renderList(block) {
    var html = "";
    var stack = []; // {indent, ordered}
    block.forEach(function (raw) {
      var m = RE_ITEM.exec(raw);
      if (!m) return; // 续行：并入上一条（这里简化成单独一段）
      var indent = Math.floor(m[1].replace(/\t/g, "  ").length / 2);
      var ordered = !!m[3];
      var text = m[4];
      while (stack.length && indent < stack[stack.length - 1].indent) {
        html += stack.pop().ordered ? "</ol>" : "</ul>";
      }
      if (!stack.length || indent > stack[stack.length - 1].indent) {
        stack.push({ indent: indent, ordered: ordered });
        html += ordered ? "<ol>" : "<ul>";
      } else if (stack[stack.length - 1].ordered !== ordered) {
        html += stack.pop().ordered ? "</ol>" : "</ul>";
        stack.push({ indent: indent, ordered: ordered });
        html += ordered ? "<ol>" : "<ul>";
      }
      html += "<li>" + inline(esc(text)) + "</li>";
    });
    while (stack.length) html += stack.pop().ordered ? "</ol>" : "</ul>";
    return html;
  }

  // ---------------------------------------------------------------- 状态

  var S = {
    token: "",
    ready: false,
    topics: [],
    chats: {},        // slug → 对话清单
    openTopics: {},   // slug → 是否展开
    topic: "",        // "" = 首页的日常问答（没有主题）
    chat: "",
    messages: [],
    activities: {},   // callId → 实时工具状态
    streaming: null,  // {turnId, chatId, text, thinking}
    approval: null,   // {kind, call|request}
    running: [],
    images: [],       // 待发送的图片
    expanded: {},     // 展开过结果的工具卡片
  };

  function save() {
    try {
      localStorage.setItem("lh.topic", S.topic);
      localStorage.setItem("lh.chat", S.chat);
      localStorage.setItem("lh.openTopics", JSON.stringify(S.openTopics));
    } catch (e) { /* 隐私模式下写不进去，不影响使用 */ }
  }

  function load() {
    try {
      S.topic = localStorage.getItem("lh.topic") || "";
      S.chat = localStorage.getItem("lh.chat") || "";
      S.openTopics = JSON.parse(localStorage.getItem("lh.openTopics") || "{}") || {};
      var theme = localStorage.getItem("lh.theme");
      if (theme) document.documentElement.setAttribute("data-theme", theme);
    } catch (e) { /* 同上 */ }
  }

  // ---------------------------------------------------------------- 令牌

  function readToken() {
    var url = new URL(location.href);
    var t = url.searchParams.get("t");
    if (t) {
      S.token = t;
      try { localStorage.setItem("lh.token", t); } catch (e) { /* ignore */ }
      // 令牌从地址栏抹掉：免得它留在浏览器历史、被别人翻到
      url.searchParams.delete("t");
      var qs = url.searchParams.toString();
      history.replaceState(null, "", url.pathname + (qs ? "?" + qs : ""));
      return t;
    }
    try { return localStorage.getItem("lh.token") || ""; } catch (e) { return ""; }
  }

  function needToken() {
    $("messages").innerHTML =
      '<div class="hint">需要访问令牌。<br>请回到电脑上的「移动端远程控制」面板，重新扫码或复制链接打开。</div>';
  }

  // ---------------------------------------------------------------- 接口

  function api(path, options) {
    var opt = options || {};
    var headers = { "X-Remote-Token": S.token };
    if (opt.body) headers["Content-Type"] = "application/json";
    return fetch(path, {
      method: opt.method || "GET",
      headers: headers,
      body: opt.body ? JSON.stringify(opt.body) : undefined,
    }).then(function (res) {
      return res.text().then(function (text) {
        var data = null;
        if (text) { try { data = JSON.parse(text); } catch (e) { data = null; } }
        if (res.status === 401) {
          var err = new Error("令牌无效或已更换");
          err.auth = true;
          throw err;
        }
        if (!res.ok) throw new Error((data && data.error) || "请求失败（" + res.status + "）");
        return data;
      });
    });
  }

  // ---------------------------------------------------------------- 渲染：列表

  function renderRail() {
    var list = $("rail-list");
    var html = "";

    // 首页的日常问答（没有主题的那些对话）
    var home = S.chats[""] || null;
    html += '<div class="rail-group">';
    html += railItem("", "日常问答", home ? home.length : null, S.topic === "");
    if (S.topic === "") html += chatListHtml("", home);
    html += "</div>";

    if (!S.topics.length) {
      html += '<div class="rail-empty">还没有主题。先在电脑上建一个吧。</div>';
    }

    S.topics.forEach(function (t) {
      var open = !!S.openTopics[t.slug];
      var chats = S.chats[t.slug] || null;
      html += '<div class="rail-group">';
      html += railItem(t.slug, (t.meta.emoji ? t.meta.emoji + " " : "") + t.meta.name, t.stats && t.stats.chats, S.topic === t.slug, open);
      if (open || S.topic === t.slug) html += chatListHtml(t.slug, chats);
      html += "</div>";
    });
    list.innerHTML = html;

    var foot = $("rail-foot");
    var info = S.state && S.state.app ? S.state.app : null;
    foot.innerHTML = info
      ? esc(info.name) + " " + esc(info.version) + (S.state.profile ? " · " + esc(S.state.profile.model) : "")
      : "";
  }

  function railItem(slug, label, count, active, open) {
    return (
      '<button class="rail-item' + (active ? " on" : "") + '" data-slug="' + esc(slug) + '">' +
      (slug ? '<span style="color:var(--text-faint);font-size:11px">' + (open || active ? "▾" : "▸") + "</span>" : "") +
      '<span class="grow">' + esc(label) + "</span>" +
      (count ? '<span class="n">' + count + "</span>" : "") +
      "</button>"
    );
  }

  function chatListHtml(slug, chats) {
    if (!chats) return '<div class="rail-empty">加载中…</div>';
    var live = chats.filter(function (c) { return !c.archived; });
    if (!live.length) return '<div class="rail-empty">还没有对话</div>';
    return (
      '<div class="rail-sub">' +
      live
        .map(function (c) {
          return (
            '<button class="rail-item' + (S.topic === slug && S.chat === c.id ? " on" : "") +
            '" data-chat="' + esc(c.id) + '" data-chat-slug="' + esc(slug) + '">' +
            '<span class="grow">' + esc(c.title || "（空对话）") + "</span>" +
            (c.pinned ? '<span class="n">置顶</span>' : "") +
            "</button>"
          );
        })
        .join("") +
      "</div>"
    );
  }

  // ---------------------------------------------------------------- 渲染：消息

  function currentTopicName() {
    if (!S.topic) return "日常问答";
    var t = S.topics.filter(function (x) { return x.slug === S.topic; })[0];
    return t ? t.meta.name : S.topic;
  }

  function currentChatTitle() {
    var list = S.chats[S.topic] || [];
    var c = list.filter(function (x) { return x.id === S.chat; })[0];
    if (c) return c.title;
    // 刚发第一条消息的对话还不在清单里，用第一句用户消息顶上
    var first = S.messages.filter(function (m) { return m.role === "user"; })[0];
    if (first) return textOf(first).slice(0, 40) || "（图片）";
    return "新对话";
  }

  function textOf(msg) {
    return (msg.blocks || [])
      .filter(function (b) { return b.type === "text"; })
      .map(function (b) { return b.text; })
      .join("");
  }

  function imagesOf(msg) {
    return (msg.blocks || []).filter(function (b) { return b.type === "image"; });
  }

  function thinkingOf(msg) {
    return (msg.blocks || [])
      .filter(function (b) { return b.type === "thinking"; })
      .map(function (b) { return b.text; })
      .join("");
  }

  /** 工具调用的输入压成一行摘要：`path=notes/a.md 第 3 页` */
  function summarize(name, input) {
    if (!input || typeof input !== "object") return "";
    var parts = [];
    Object.keys(input).forEach(function (k) {
      if (parts.length >= 3) return;
      var v = input[k];
      if (v === null || v === undefined || v === "") return;
      var s = typeof v === "string" ? v : JSON.stringify(v);
      if (s.length > 40) s = s.slice(0, 40) + "…";
      parts.push(k + "=" + s);
    });
    return parts.join(" · ");
  }

  function toolCardHtml(card) {
    var live = S.activities[card.id];
    var state = "run";
    var stateText = "运行中";
    var body = "";
    if (card.result) {
      state = card.result.is_error ? "bad" : "ok";
      stateText = card.result.is_error ? "出错" : "完成";
    } else if (live) {
      if (live.done) {
        state = live.ok ? "ok" : "bad";
        stateText = live.denied ? "已拒绝" : live.ok ? "完成" : "出错";
      }
    } else if (!S.streaming || S.streaming.turnId !== card.turnId) {
      stateText = "未完成";
    }
    var resultText = card.result ? String(card.result.content || "") : live && live.preview ? live.preview : "";
    var images = (card.result && card.result.images) || [];
    var open = !!S.expanded[card.id];

    var html =
      '<div class="tool" data-tool="' + esc(card.id) + '">' +
      '<div class="tool-head">' +
      '<span class="name">' + esc(card.name) + "</span>" +
      '<span class="grow">' + esc(summarize(card.name, card.input)) + "</span>" +
      '<span class="state ' + state + '">' + esc(stateText) + "</span>" +
      "</div>";
    if (resultText || images.length) {
      html += '<button class="tool-toggle" data-toggle="' + esc(card.id) + '">' + (open ? "收起结果" : "看结果") + "</button>";
      if (open) {
        html += '<div class="tool-body">' + esc(resultText.slice(0, 4000));
        images.forEach(function (img) {
          html += '<img src="' + esc(fileUrl(img.path)) + '" alt="' + esc(img.name || "") + '" loading="lazy">';
        });
        html += "</div>";
      }
    }
    return html + "</div>";
  }

  function messageHtml(item) {
    if (item.type === "user") {
      var imgs = imagesOf(item.msg);
      return (
        '<div class="msg user">' +
        (imgs.length
          ? '<div class="imgs">' +
            imgs.map(function (im) { return '<img src="' + esc(fileUrl(im.path)) + '" alt="" loading="lazy">'; }).join("") +
            "</div>"
          : "") +
        esc(textOf(item.msg)) +
        "</div>"
      );
    }
    if (item.type === "assistant") {
      var html = '<div class="msg assistant">';
      if (item.thinking) {
        html +=
          '<details class="thinking"><summary>思考过程</summary><div class="md">' +
          renderMarkdown(item.thinking) +
          "</div></details>";
      }
      if (item.text) html += '<div class="md">' + renderMarkdown(item.text) + "</div>";
      item.tools.forEach(function (c) { html += toolCardHtml(c); });
      return html + "</div>";
    }
    // 落单的工具结果（找不到对应的调用）：直接当一行说明
    return '<div class="msg assistant"><div class="tool"><div class="tool-head"><span class="name">工具结果</span><span class="grow">' +
      esc(String(item.result.content || "").slice(0, 120)) +
      '</span><span class="state ok">完成</span></div></div></div>';
  }

  /** 把消息列表拍平成「能直接渲染的项」：assistant 的 tool_use 与 tool 结果按 id 配对。 */
  function buildItems(messages) {
    var items = [];
    var byCall = {};
    messages.forEach(function (m) {
      if (m.role === "user") {
        items.push({ type: "user", msg: m });
      } else if (m.role === "assistant") {
        var tools = [];
        (m.blocks || []).forEach(function (b) {
          if (b.type !== "tool_use") return;
          var card = { type: "tool", id: b.id, name: b.name, input: b.input, result: null, turnId: m.id };
          tools.push(card);
          byCall[b.id] = card;
        });
        items.push({ type: "assistant", text: textOf(m), thinking: thinkingOf(m), tools: tools, msg: m });
      } else if (m.role === "tool") {
        (m.blocks || []).forEach(function (b) {
          if (b.type !== "tool_result") return;
          var card = byCall[b.tool_use_id];
          if (card) card.result = b;
          else items.push({ type: "toolResult", result: b });
        });
      }
    });
    return items;
  }

  function renderMessages(keepScroll) {
    var box = $("messages");
    var nearBottom = keepScroll !== false && isNearBottom();
    var items = buildItems(S.messages);

    if (!items.length && !S.streaming) {
      box.innerHTML =
        '<div class="hint">' +
        esc(currentTopicName()) +
        "<br>在下面输入，消息会由电脑上的 agent 执行。<br>也可以贴一张图（拍照或相册）。</div>";
    } else {
      box.innerHTML = items.map(messageHtml).join("");
    }
    renderStreaming();
    hydrateRichContent(box);
    if (nearBottom) scrollToBottom();
  }

  function isNearBottom() {
    var box = $("messages");
    return box.scrollHeight - box.scrollTop - box.clientHeight < 120;
  }

  function scrollToBottom() {
    var box = $("messages");
    box.scrollTop = box.scrollHeight;
  }

  /** 正在流式输出的那一段（单独渲染，增量只动这里）。 */
  function renderStreaming() {
    var old = document.getElementById("streaming-bubble");
    if (old) old.remove();
    if (!S.streaming) return;
    var box = $("messages");
    var el = document.createElement("div");
    el.className = "msg assistant";
    el.id = "streaming-bubble";
    var html = "";
    if (S.streaming.thinking) {
      html += '<details class="thinking" open><summary>思考过程</summary><div class="md">' + renderMarkdown(S.streaming.thinking) + "</div></details>";
    }
    html += '<div class="md">' + (S.streaming.text ? renderMarkdown(S.streaming.text) : '<span style="color:var(--text-faint)">正在思考…</span>') + "</div>";
    el.innerHTML = html;
    box.appendChild(el);
    hydrateRichContent(el);
  }

  var streamTimer = null;
  function scheduleStreamRender() {
    if (streamTimer) return;
    streamTimer = requestAnimationFrame(function () {
      streamTimer = null;
      var near = isNearBottom();
      renderStreaming();
      if (near) scrollToBottom();
    });
  }

  // ---------------------------------------------------------------- 渲染：审批与底栏

  function renderDock() {
    var dock = $("dock");
    var html = "";
    if (S.approval && S.approval.kind === "tool") {
      var c = S.approval.call;
      html +=
        '<div class="approval"><div class="head">⚠ 需要你确认</div>' +
        "<div><b>" + esc(c.name) + "</b> · " + esc(c.riskLabel || "") + "</div>" +
        '<div class="why">' + esc(c.summary) + "</div>" +
        '<div class="acts">' +
        '<button class="btn primary" data-approve="1">允许</button>' +
        '<button class="btn danger" data-approve="0">拒绝</button>' +
        '<span class="spacer"></span>' +
        '<label class="always"><input type="checkbox" id="approve-always"> 本轮总是允许</label>' +
        "</div></div>";
    } else if (S.approval && S.approval.kind === "sandbox") {
      html +=
        '<div class="approval"><div class="head">⚠ 需要你确认</div>' +
        "<div>agent 想访问工作区之外的路径：</div>" +
        '<div class="why">' + esc(S.approval.request.path) + "</div>" +
        '<div class="acts"><span style="color:var(--text-dim);font-size:12.5px">' +
        "这是「放开一个目录」的决定，请回电脑上确认。" +
        "</span></div></div>";
    }
    dock.innerHTML = html;

    var running = !!S.streaming;
    $("stop-btn").hidden = !running;
    $("send-btn").hidden = running;
    $("send-btn").disabled = !running && !$("text").value.trim() && !S.images.length;
  }

  function renderChips() {
    var box = $("chips");
    box.innerHTML = S.images
      .map(function (im, i) {
        return (
          '<div class="chip"><img src="' + esc(im.preview) + '" alt="">' +
          '<span>' + esc(im.name) + "</span>" +
          '<button type="button" data-drop="' + i + '" aria-label="移除">×</button></div>'
        );
      })
      .join("");
  }

  function renderTitle() {
    $("chat-title").textContent = S.chat ? currentChatTitle() : "学习中枢";
    $("chat-sub").textContent = S.chat ? currentTopicName() : "选一条对话，或直接新建";
  }

  // ---------------------------------------------------------------- 数据加载

  function loadChats(slug) {
    return api("/api/chats?topic=" + encodeURIComponent(slug))
      .then(function (items) {
        S.chats[slug] = items || [];
        renderRail();
      })
      .catch(function (e) { toast(e.message, "error"); });
  }

  function openChat(slug, chatId) {
    S.topic = slug || "";
    S.chat = chatId || "";
    S.messages = [];
    S.streaming = null;
    S.activities = {};
    S.approval = null;
    save();
    renderRail();
    renderTitle();
    renderMessages();
    renderDock();
    closeDrawer();
    if (!S.chat) return Promise.resolve();
    return api("/api/transcript?topic=" + encodeURIComponent(S.topic) + "&chat=" + encodeURIComponent(S.chat))
      .then(function (messages) {
        S.messages = messages || [];
        renderMessages();
      })
      .catch(function (e) {
        $("messages").innerHTML = '<div class="hint">' + esc(e.message) + "</div>";
      });
  }

  function refreshState() {
    return api("/api/state").then(function (st) {
      S.state = st;
      S.topics = st.topics || [];
      S.running = st.running || [];
      // 运行中的轮次里若有当前对话，说明它正在生成（切出去再切回来也能看到）
      var mine = S.running[0];
      if (mine && !S.streaming) S.streaming = { turnId: mine.turnId, chatId: S.chat, text: "", thinking: "" };
      var jobs = [loadChats("")];
      if (S.topic) jobs.push(loadChats(S.topic));
      return Promise.all(jobs).then(function () {
        // 展开过的主题也把清单补上
        Object.keys(S.openTopics).forEach(function (slug) {
          if (S.openTopics[slug] && !S.chats[slug]) jobs.push(loadChats(slug));
        });
        renderRail();
        renderDock();
        // 没选对话时这里要显示空态提示（第一次打开页面、或对话被删掉之后）
        if (!S.chat) renderMessages();
      });
    });
  }

  // ---------------------------------------------------------------- SSE

  function connect() {
    if (!S.token) return;
    var es = new EventSource("/api/events?t=" + encodeURIComponent(S.token));
    es.onopen = function () {
      $("dot").className = "dot on";
      $("dot").title = "已连接";
      // 断线期间漏掉的事件靠重拉补齐
      void refreshState().then(function () { return S.chat ? reloadTranscript() : null; });
    };
    es.onerror = function () {
      $("dot").className = "dot off";
      $("dot").title = "连接断开，正在重连…";
      // EventSource 自己会重连，这里不用管
    };
    es.onmessage = function (e) {
      var frame;
      try { frame = JSON.parse(e.data); } catch (err) { return; }
      handleFrame(frame);
    };
    return es;
  }

  function handleFrame(frame) {
    if (frame.kind === "agent") return handleAgent(frame.event);
    if (frame.kind === "topics") return void refreshState();
    if (frame.kind === "toast") return void toast(frame.message, frame.level);
    if (frame.kind === "clients") return;
  }

  function handleAgent(ev) {
    switch (ev.kind) {
      case "turn_started":
        if (ev.chat_id !== S.chat) return;
        S.streaming = { turnId: ev.turn_id, chatId: ev.chat_id, text: "", thinking: "" };
        renderDock();
        scheduleStreamRender();
        break;
      case "delta":
        if (!S.streaming || S.streaming.turnId !== ev.turn_id) return;
        if (ev.thinking) S.streaming.thinking += ev.text;
        else S.streaming.text += ev.text;
        scheduleStreamRender();
        break;
      case "stream_reset":
        if (S.streaming && S.streaming.turnId === ev.turn_id) {
          S.streaming.text = "";
          S.streaming.thinking = "";
          scheduleStreamRender();
        }
        break;
      case "message":
        if (!S.streaming || S.streaming.turnId !== ev.turn_id) return;
        S.messages.push(ev.message);
        S.streaming.text = "";
        S.streaming.thinking = "";
        renderMessages();
        break;
      case "tool_started":
        S.activities[ev.call_id] = { name: ev.name, summary: ev.summary, done: false, preview: "" };
        renderMessages();
        break;
      case "tool_finished":
        S.activities[ev.outcome.callId] = {
          name: ev.outcome.name,
          summary: ev.outcome.summary,
          done: true,
          ok: ev.outcome.ok,
          denied: ev.outcome.denied,
          preview: ev.outcome.preview,
        };
        renderMessages();
        break;
      case "tool_approval":
        S.approval = { kind: "tool", call: ev.call };
        renderDock();
        scrollToBottom();
        break;
      case "sandbox_request":
        S.approval = { kind: "sandbox", request: { path: ev.path, reason: ev.reason } };
        renderDock();
        break;
      case "finished":
        if (S.streaming && S.streaming.turnId === ev.turn_id) {
          S.streaming = null;
          renderDock();
          void reloadTranscript();
        }
        break;
      case "failed":
        if (S.streaming && S.streaming.turnId === ev.turn_id) {
          S.streaming = null;
          renderDock();
        }
        toast(ev.message || "这一轮失败了", "error");
        break;
      default:
        break;
    }
  }

  function reloadTranscript() {
    if (!S.chat) return Promise.resolve();
    return api("/api/transcript?topic=" + encodeURIComponent(S.topic) + "&chat=" + encodeURIComponent(S.chat))
      .then(function (messages) {
        S.messages = messages || [];
        renderMessages();
        renderTitle();
      })
      .catch(function () { /* 对话可能刚被删掉，忽略 */ });
  }

  // ---------------------------------------------------------------- 发送 / 图片

  function send() {
    var text = $("text").value.trim();
    if (!text && !S.images.length) return;
    if (!S.chat) {
      // 还没选对话：就地开一条（id 由服务端生成）
      api("/api/new_chat", { method: "POST" }).then(function (r) {
        return openChat(S.topic, r.chatId).then(function () { doSend(text); });
      }).catch(function (e) { toast(e.message, "error"); });
      return;
    }
    doSend(text);
  }

  function doSend(text) {
    var images = S.images.map(function (im) {
      return { name: im.name, data: im.data, width: im.width, height: im.height };
    });
    S.images = [];
    renderChips();
    $("text").value = "";
    autoGrow();
    api("/api/send", { method: "POST", body: { topic: S.topic, chat: S.chat, text: text, images: images } })
      .then(function () {
        renderDock();
      })
      .catch(function (e) {
        toast(e.message, "error");
      });
  }

  function autoGrow() {
    var el = $("text");
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, window.innerHeight * 0.4) + "px";
  }

  function readAsDataURL(file) {
    return new Promise(function (resolve, reject) {
      var fr = new FileReader();
      fr.onload = function () { resolve(String(fr.result || "")); };
      fr.onerror = function () { reject(new Error("读不出这个文件")); };
      fr.readAsDataURL(file);
    });
  }

  function loadImage(url) {
    return new Promise(function (resolve, reject) {
      var img = new Image();
      img.onload = function () { resolve(img); };
      img.onerror = function () { reject(new Error("这张图打不开")); };
      img.src = url;
    });
  }

  /* 长边压到 1568px 再发（和桌面端同一条规矩：超过这个尺寸只会更贵，不会更清楚）。 */
  var MAX_EDGE = 1568;
  function prepareImage(file) {
    return readAsDataURL(file).then(function (dataUrl) {
      return loadImage(dataUrl).then(function (img) {
        var w = img.naturalWidth || 0;
        var h = img.naturalHeight || 0;
        var scale = Math.min(1, MAX_EDGE / Math.max(w, h || 1));
        var base64 = dataUrl.slice(dataUrl.indexOf("base64,") + 7);
        if (scale >= 1 && file.size <= 1500000) {
          return { name: file.name || "图片", data: base64, width: w, height: h, preview: dataUrl };
        }
        var cw = Math.max(1, Math.round(w * scale));
        var ch = Math.max(1, Math.round(h * scale));
        var canvas = document.createElement("canvas");
        canvas.width = cw;
        canvas.height = ch;
        var ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("画布不可用，没法压缩这张图");
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = "high";
        ctx.drawImage(img, 0, 0, cw, ch);
        var out = canvas.toDataURL(file.type === "image/png" ? "image/png" : "image/jpeg", 0.92);
        return {
          name: file.name || "图片",
          data: out.slice(out.indexOf("base64,") + 7),
          width: cw,
          height: ch,
          preview: out,
        };
      });
    });
  }

  function addFiles(files) {
    Array.prototype.slice.call(files).forEach(function (file) {
      if (!file.type || file.type.indexOf("image/") !== 0) return;
      prepareImage(file)
        .then(function (im) {
          S.images.push(im);
          renderChips();
          renderDock();
        })
        .catch(function (e) { toast(e.message, "error"); });
    });
  }

  // ---------------------------------------------------------------- 抽屉 / 主题

  function openDrawer() {
    $("rail").classList.add("open");
    $("backdrop").hidden = false;
  }
  function closeDrawer() {
    $("rail").classList.remove("open");
    $("backdrop").hidden = true;
  }

  function toggleTheme() {
    var cur = document.documentElement.getAttribute("data-theme") || "auto";
    var next = cur === "auto" ? "dark" : cur === "dark" ? "light" : "auto";
    document.documentElement.setAttribute("data-theme", next);
    try { localStorage.setItem("lh.theme", next); } catch (e) { /* ignore */ }
  }

  // ---------------------------------------------------------------- 事件绑定

  function bind() {
    $("menu-btn").addEventListener("click", openDrawer);
    $("backdrop").addEventListener("click", closeDrawer);
    $("theme-btn").addEventListener("click", toggleTheme);
    $("refresh-btn").addEventListener("click", function () {
      S.chats = {};
      void refreshState().then(function () { return reloadTranscript(); });
      toast("已刷新");
    });

    $("rail-list").addEventListener("click", function (e) {
      var chat = e.target.closest("[data-chat]");
      if (chat) {
        void openChat(chat.getAttribute("data-chat-slug"), chat.getAttribute("data-chat"));
        return;
      }
      var topic = e.target.closest("[data-slug]");
      if (!topic) return;
      var slug = topic.getAttribute("data-slug");
      // 点主题 = 展开/收起它下面的对话；点第二次就切到它（最近的一条）
      var wasOpen = !!S.openTopics[slug];
      S.openTopics[slug] = !wasOpen;
      save();
      if (!wasOpen) {
        if (!S.chats[slug]) void loadChats(slug);
        renderRail();
        return;
      }
      renderRail();
      if (S.topic !== slug) {
        var list = (S.chats[slug] || []).filter(function (c) { return !c.archived; });
        if (list.length) void openChat(slug, list[0].id);
        else void openChat(slug, "");
      }
    });

    $("dock").addEventListener("click", function (e) {
      var btn = e.target.closest("[data-approve]");
      if (!btn || !S.approval || !S.approval.call) return;
      var allow = btn.getAttribute("data-approve") === "1";
      var always = allow && $("approve-always") && $("approve-always").checked;
      var requestId = S.approval.call.requestId;
      S.approval = null;
      renderDock();
      api("/api/approve", { method: "POST", body: { requestId: requestId, allow: allow, always: always } })
        .then(function (r) {
          if (!r.handled) toast("这个确认已经在电脑上处理过了");
        })
        .catch(function (err) { toast(err.message, "error"); });
    });

    $("messages").addEventListener("click", function (e) {
      var toggle = e.target.closest("[data-toggle]");
      if (toggle) {
        var id = toggle.getAttribute("data-toggle");
        S.expanded[id] = !S.expanded[id];
        renderMessages();
        return;
      }
      var zoom = e.target.closest("[data-demo-zoom]");
      if (zoom) {
        var card = zoom.closest(".demo");
        var frame = card && card.querySelector(".demo-frame");
        if (frame) {
          $("demo-frame").setAttribute("srcdoc", frame.getAttribute("srcdoc") || "");
          $("demo-view").hidden = false;
        }
        return;
      }
      if (e.target.tagName === "IMG") {
        $("viewer-img").src = e.target.src;
        $("viewer").hidden = false;
      }
    });

    $("viewer").addEventListener("click", function () { $("viewer").hidden = true; });
    $("demo-view").addEventListener("click", function () {
      $("demo-view").hidden = true;
      $("demo-frame").removeAttribute("srcdoc");
    });

    $("composer").addEventListener("submit", function (e) {
      e.preventDefault();
      send();
    });
    $("stop-btn").addEventListener("click", function () {
      if (!S.streaming) return;
      api("/api/cancel", { method: "POST", body: { turnId: S.streaming.turnId } }).catch(function (err) {
        toast(err.message, "error");
      });
    });
    $("new-btn").addEventListener("click", function () {
      api("/api/new_chat", { method: "POST" })
        .then(function (r) { return openChat(S.topic, r.chatId); })
        .catch(function (e) { toast(e.message, "error"); });
    });
    $("attach-btn").addEventListener("click", function () { $("file-input").click(); });
    $("file-input").addEventListener("change", function (e) {
      addFiles(e.target.files);
      e.target.value = "";
    });
    $("chips").addEventListener("click", function (e) {
      var drop = e.target.closest("[data-drop]");
      if (!drop) return;
      S.images.splice(+drop.getAttribute("data-drop"), 1);
      renderChips();
      renderDock();
    });

    var text = $("text");
    text.addEventListener("input", function () {
      autoGrow();
      renderDock();
    });
    text.addEventListener("keydown", function (e) {
      // 手机上回车是换行（键盘上还有「发送」键专门用来发）；桌面上回车即发送
      if (e.key === "Enter" && !e.shiftKey && window.matchMedia("(min-width: 720px)").matches) {
        e.preventDefault();
        send();
      }
    });

    // 拖图进来（桌面浏览器打开时顺手支持一下）
    document.addEventListener("dragover", function (e) { e.preventDefault(); });
    document.addEventListener("drop", function (e) {
      if (e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files.length) {
        e.preventDefault();
        addFiles(e.dataTransfer.files);
      }
    });

    window.addEventListener("resize", function () {
      if (window.innerWidth >= 720) closeDrawer();
    });
  }

  // ---------------------------------------------------------------- 启动

  function boot() {
    load();
    S.token = readToken();
    bind();
    renderChips();
    renderDock();
    // 先把「空态」画出来：首屏第一眼该看到「选一条对话，或直接新建」，
    // 而不是等状态拉完才从白板变成内容（慢网络上那块白板会停留好几秒）。
    renderTitle();
    renderMessages();
    if (!S.token) {
      needToken();
      return;
    }
    refreshState()
      .then(function () {
        if (S.chat) return openChat(S.topic, S.chat);
        return null;
      })
      .catch(function (e) {
        if (e.auth) needToken();
        else $("messages").innerHTML = '<div class="hint">连不上电脑上的学习中枢：' + esc(e.message) + "</div>";
      });
    connect();
    // 手机上息屏回来时 SSE 可能已经断了（EventSource 会自己重连），
    // 顺手重拉一次状态，免得看到的是几十分钟前的清单
    document.addEventListener("visibilitychange", function () {
      if (document.visibilityState === "visible") void refreshState();
    });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
  else boot();
})();
