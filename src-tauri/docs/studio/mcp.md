# 工坊规范：MCP 服务器

> 这份文档是工坊模式的内置操作手册（`spec_read` 返回的就是它），写给你（agent）照着做，不是用户输入。用户问起时可以概括，不要整篇复述。

## 1. MCP 是什么，本应用支持到什么程度

MCP（Model Context Protocol）= 用 JSON-RPC 2.0 跟一个**子进程**对话，让它提供工具；本应用把这些工具转成自己的工具交给模型调用。

- **只支持 stdio 传输**：应用 `spawn` 一个本地子进程，从它的 stdin 写、从 stdout 读。**不支持 HTTP / SSE**，配置里也没有 URL 这类字段（`McpServerConfig` 只有 `name` / `command` / `args` / `env` / `cwd` / `enabled`）。
- 客户端**只发送四个方法**：`initialize`（握手）、`notifications/initialized`（通知）、`tools/list`、`tools/call`。`resources/*`、`prompts/*`、`sampling/*`、`logging/*`、`ping`、订阅等都不会被调用——实现了也用不上。应用**不给子进程做沙箱或权限限制**（工作区沙箱只约束 agent 自己的文件工具），它就是以你的身份运行的本地进程，「别做危险操作」是硬要求，见 §8。

## 2. 传输与帧格式（踩坑最多的地方）

- **换行分帧**：一行一个完整 JSON，行尾 `\n`。不是 SSE，也不是 `Content-Length` 头。一条报文必须在一行里写完——不要 pretty-print，缩进换行会把消息拆成多行，客户端逐行解析、逐行丢弃，表现为「请求永远没响应，120 秒后超时」。
- **stdout 只写协议报文**。日志、进度、报错一律写 **stderr**：应用会持续读子进程 stderr 并转成自己的日志（前缀 `[mcp:<服务器名>]`），写多少都不会卡住服务器；往 stdout 多写一行则会污染分帧。
- **响应必须原样回传请求的 `id`，而且必须是数字**。客户端只按整数 id 匹配等待中的请求；把 id 回成字符串（`"1"`）的响应会被静默丢弃 → 超时。客户端发的 id 是 1、2、3… 递增整数；没有 `id` 的报文是通知，**不要给通知回响应**，服务器自己发的通知客户端也会忽略（它只看带 id 的报文）。
- 编码 UTF-8。Windows 上 Python 默认按本地代码页输出，中文会变成 GBK 字节，客户端按 UTF-8 解码会直接断开连接。**用 `sys.stdout.buffer.write(...encode("utf-8"))` 显式写字节**（本文示例都这么写），并在注册时加 `PYTHONIOENCODING=utf-8` 兜底。
- 超时：握手 `initialize` **20 秒**，其余请求 **120 秒**；超时后报「MCP 请求超时」，服务器不会被打断。服务器进程退出时，未完成的请求会报「MCP 服务器「x」已退出」。

## 3. 协议最小实现：真实报文

握手（`clientInfo.version` 是本应用的版本号，不用管；服务器只需保证 `serverInfo.name` 存在，其余字段客户端不读）：

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"clientInfo":{"name":"learning-hub","version":"x.y.z"}}}
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"glossary","version":"0.1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
```

上面三行的顺序即实际顺序：客户端先发 `initialize`（超时 20 秒），拿到响应后发**通知** `notifications/initialized`（无 id，收到后不要回）。`protocolVersion` 客户端不校验，回 `2024-11-05` 最省事。

握手完立刻拉工具清单、调用工具、协议层报错（`error.message` 会原样报给模型，前缀「MCP 报错：」）：

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"glossary_lookup","description":"在本地词表里查一个词条。","inputSchema":{"type":"object","properties":{"term":{"type":"string","description":"要查的词，例如 entropy 或 熵"}},"required":["term"],"additionalProperties":false}}]}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"glossary_lookup","arguments":{"term":"entropy"}}}
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"- entropy（熵）：系统无序程度的度量。"}],"isError":false}}
{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"不支持的方法：resources/list"}}
```

客户端怎么处理 `result`：

- `content` 数组里只认 **text 块**（`{"type":"text","text":"…"}`），多个 text 块用换行拼起来；`image` 块只显示成一句「图片结果…暂不显示」的占位说明，`resource` 块只留一个 uri，其它类型直接丢弃。
- `isError: true` 表示这次调用失败，「参数不对」「查不到」用它表示，比抛协议错误更好。一个 text 块都没有时，应用把整个 `result` 的 JSON 美化后交给模型（能救急，别指望）。
- 应用**不截断** MCP 返回的内容，它会原样进模型上下文——长度要服务器自己控制（见 §5）。

## 4. 最小骨架（两份都完整可运行）

Python，只用标准库：

```python
#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""最小 MCP 服务器骨架：stdio、换行分帧、四个方法。"""
import datetime, json, sys

PROTOCOL_VERSION = "2024-11-05"
TOOLS = [{"name": "server_now", "description": "返回当前日期与星期几。",
          "inputSchema": {"type": "object", "properties": {}, "required": [], "additionalProperties": False}}]


def call_tool(name, args):
    if name == "server_now":
        now = datetime.datetime.now()
        return f"现在是 {now:%Y-%m-%d %H:%M}，星期{'一二三四五六日'[now.weekday()]}", False
    return f"没有名为 {name} 的工具", True


def handle(req):
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        return {"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}},
                "serverInfo": {"name": "skeleton", "version": "0.1.0"}}}
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}}
    if method == "tools/call":
        p = req.get("params") or {}
        text, is_err = call_tool(p.get("name"), p.get("arguments") or {})
        return {"jsonrpc": "2.0", "id": rid, "result": {"content": [{"type": "text", "text": text}], "isError": is_err}}
    if rid is None:
        return None                                    # 通知：不回
    return {"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": f"不支持的方法：{method}"}}


def main():
    while True:
        raw = sys.stdin.buffer.readline()              # 逐行读字节，不依赖本地代码页
        if not raw:                                    # stdin 关了（应用退出/重连）
            return
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as exc:                       # 丢掉坏行，别让进程退出
            print(f"skip unparsable line: {exc}", file=sys.stderr)
            continue
        resp = handle(req)
        if resp is not None:
            sys.stdout.buffer.write((json.dumps(resp, ensure_ascii=False) + "\n").encode("utf-8"))
            sys.stdout.buffer.flush()                  # 必须立刻 flush，否则握手超时


if __name__ == "__main__":
    main()
```

Node，无第三方依赖（日志用 `console.error`，`console.log` 会写进协议流）：

```js
#!/usr/bin/env node
// 最小 MCP 服务器骨架：stdio、换行分帧、四个方法。
const readline = require("readline");
const PROTOCOL_VERSION = "2024-11-05";
const now = () => new Date().toLocaleString("zh-CN");
const TOOLS = [{ name: "server_now", description: "返回当前日期与星期几。", inputSchema: { type: "object", properties: {}, required: [], additionalProperties: false } }];

function handle(req) {
  if (req.method === "initialize") return { jsonrpc: "2.0", id: req.id, result: { protocolVersion: PROTOCOL_VERSION,
    capabilities: { tools: {} }, serverInfo: { name: "skeleton-node", version: "0.1.0" } } };
  if (req.method === "tools/list") return { jsonrpc: "2.0", id: req.id, result: { tools: TOOLS } };
  if (req.method === "tools/call") {
    const name = (req.params || {}).name, ok = name === "server_now";
    const text = ok ? `现在是 ${now()}` : `没有名为 ${name} 的工具`;
    return { jsonrpc: "2.0", id: req.id, result: { content: [{ type: "text", text }], isError: !ok } };
  }
  if (req.id === undefined || req.id === null) return null;   // 通知：不回
  return { jsonrpc: "2.0", id: req.id, error: { code: -32601, message: `不支持的方法：${req.method}` } };
}

readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let req;
  try { req = JSON.parse(trimmed); } catch (e) { console.error(`skip unparsable line: ${e.message}`); return; }
  const resp = handle(req);
  if (resp) process.stdout.write(JSON.stringify(resp) + "\n");
});
```

## 5. 工具定义怎么写

- `name`：小写字母 + 下划线，1–3 个词的动宾短语（`glossary_lookup`）。别用空格、中文、大写；**别在名字里重复服务器名**（应用会拼成 `mcp__<服务器>__<工具>`）。
- `description`：中文，1–3 句，写清「什么时候用 + 返回什么 + 有没有副作用」。应用会加上 `[来自 MCP 服务器「x」] ` 前缀交给模型，别自己再写服务器名。风格同内置工具：陈述式、克制、不用感叹号。
- `inputSchema`：JSON Schema，`type: "object"` + `properties`（每个属性写 `description`）+ `required`，可选 `additionalProperties: false`。**缺 `inputSchema` 时应用会兜一个空对象 schema**，模型就看不到参数说明了——每个参数都要声明。
- 返回：只用 text 块，内容是给模型看的一小段文本（Markdown 也行）：先给结论，列表带上总数与截断说明（「共 42 条，只显示前 5 条」），让模型知道结果不完整。工具数建议 3–8 个，一个工具只做一件事；参数名别用 `arg1` 这类名字。
- 长度：应用不截断，所以**服务器自己限长**。单次几百到一两千字；大文件只回当前要用的那一段，并说明怎么取更多（「要别的词再调一次」）。

## 6. 依赖与环境

- **优先标准库**：Python 只用 `json` / `sys` / `csv` / `os` / `datetime`，Node 只用内置模块。没有依赖就没有安装步骤，也就没有「用户机器上装没装」这个问题。
- 确实要第三方依赖时：目录里放 `requirements.txt` / `package.json`，README 写清装什么、怎么装、装到哪个解释器。**`mcp_publish` 只搬文件、登记配置、连一次，不会替你装依赖**——要么无依赖，要么把安装命令写给用户，并让服务器缺依赖时把原因写 stderr（`except ImportError as e: print(..., file=sys.stderr)`）后退出。
- Windows 上的解释器名字：`python` / `py` / `python3` 哪个可用取决于用户机器。`command` 会被**直接交给系统执行，不经过 shell**，所以不能写 `cmd /c ...`、不能用管道或重定向；参数全放进 `args` 数组（逐项传递，路径带空格也不用加引号）。不确定就用绝对路径，或直接问用户。
- `mcp_publish` 会**把 `cwd` 自动设成服务器自己的目录**（`<工作区>/.hub/mcp/<id>/`），
  所以 `command: "python"` + `args: ["server.py"]` 这类写法能直接跑起来；你也可以显式传 `cwd`
  覆盖它。即便有这条兜底，**读数据文件时仍建议用脚本所在目录推导**
  （`os.path.dirname(os.path.abspath(__file__))`）或绝对路径——用户以后可能在配置里改 cwd，
  那种改动不该让服务器读不到自己的数据。
  建议带的环境变量：`PYTHONIOENCODING=utf-8`（防本地代码页写坏中文）、`PYTHONUNBUFFERED=1`（少一层缓冲）；
  Node 不需要。`npx` 起服务是允许的，但它首次运行会联网下载包，容易超过 20 秒的握手超时——
  自己写一个无依赖脚本更稳。

## 7. 注册与测试

1. 草稿放 `mcp/<服务器id>/`（工作台根是 `<工作区>/.hub/workshop/`）：源码 + `README.md`（做什么、怎么装依赖、怎么排查；启动失败的原因要能在 stderr 里看到）。
2. `mcp_publish(dir, name, command, args, env, cwd?)`：把目录搬到 `<工作区>/.hub/mcp/<id>/`，登记进全局 MCP 配置（`config.json` 的 `agent.mcp_servers`），立刻重连，返回连接状态与暴露的工具名。
   - `dir`：相对工作台根的目录，例如 `mcp/glossary`。
   - `name`：服务器名，会成为工具名前缀。短小写英文（`glossary`），**不要含 `__`**（工具名按第一个 `__` 切分，含 `__` 会切错）。
   - `command`：可执行文件名或绝对路径，例如 `python`；不带参数。
   - `args`：参数数组，例如 `["server.py"]`（相对路径按 `cwd` 解析），或直接给发布后的绝对路径 `<工作区>\.hub\mcp\glossary\server.py`。
   - `env`：字符串键值，例如 `{"PYTHONIOENCODING": "utf-8", "PYTHONUNBUFFERED": "1"}`。
   - `cwd`：可选。不传时自动用服务器自己的目录；`args` 里指向工作台的绝对路径会被自动改写过去，不用你手算新位置。
3. 看结果：`mcp_status()` 列出每个服务器的连接状态、命令、`serverInfo`、工具数与工具名；发布工具本身也返回连接状态与工具名。工具以 **`mcp__<服务器>__<工具>`** 出现在 agent 的工具表里（描述带 `[来自 MCP 服务器「x」]` 前缀）。
4. 迭代：改完代码再 `mcp_publish` 一次即可——重连会先杀掉旧子进程、摘掉上一轮的 `mcp__*` 工具再重新连接，不会越堆越多。
5. 调用新工具：它在发布时就登记好了，但**本轮对话开始时装好的工具清单不会重算**，所以更稳的验证是让用户发下一条消息，或按规则拼名字直接调用（`mcp__glossary__glossary_lookup`）。
6. 报错定位（会出现在 `mcp_status` 的 error 或工具结果里）：
   - `启动 MCP 服务器「x」失败（python）：…` → `command` 写错或不在 PATH 上。
   - `MCP 服务器「x」握手失败：握手超时` → 20 秒内没收到 `initialize` 响应。常见原因：stdout 混入非协议输出、忘了 flush、编码不是 UTF-8、脚本一启动就抛异常（看 stderr 里的 `[mcp:x] …`）。
   - 请求超时 120 秒 → 响应没带数字 `id`、JSON 跨了多行、或工具真的跑了太久。
   - 服务器起来又被判「已退出」→ 脚本读 stdin 读到 EOF 就退出了，别在 `main` 之外提前 return。

## 8. 安全与边界

- 不写会**删除或覆盖用户文件**的服务器，只读优先；写入类工具必须在 `description` 里说明「会写什么、写到哪里」。
- 不联网上传任何数据（讲义、笔记、词表都留在本机）；需要联网的工具要写清访问哪个站点、做什么。
- 不做常驻/后台服务：子进程随应用关闭、禁用服务器或重连时被 kill，不要在服务器里再 spawn 常驻进程；单个工具控制在 120 秒内（轮询、等待、长计算拆成多次调用）。
- 应用把所有 MCP 工具都按「写入」风险对待：在「每次确认」权限下每次调用都会弹窗让用户点头，而在「自动编辑 / 完全访问」下**不会打断**。别把审批当兜底，危险操作就别做。
- 日志一律 stderr，且别用 emoji / 生僻符号：Python 的 stderr 在 Windows 上按本地代码页编码，编不出来的字符会抛 `UnicodeEncodeError` 把进程搞死。

## 9. 完整示例：本地词表（CSV）查询服务器

`mcp/glossary/server.py`：

```python
#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""glossary：查本地词表的 MCP 服务器（stdio）。词表默认取本脚本同目录的 glossary.csv，
也可用第一个命令行参数指定。CSV 列：term,reading,meaning,example（UTF-8，含表头）。"""
import csv, json, os, sys

PROTOCOL_VERSION = "2024-11-05"
MAX_HITS, MAX_TEXT = 5, 3000          # 单次返回上限：别把模型上下文塞满
PATH = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "glossary.csv")

TOOLS = [{
    "name": "glossary_lookup",
    "description": "在本地词表里查一个词条（中文或原文都行），返回读音、释义与例句。用户问「这个词什么意思」时用。",
    "inputSchema": {"type": "object", "additionalProperties": False, "required": ["term"],
                    "properties": {"term": {"type": "string", "description": "要查的词，例如 entropy 或 熵"}}},
}]


def reply(rid, text, is_err=False):
    return {"jsonrpc": "2.0", "id": rid, "result": {"content": [{"type": "text", "text": text}], "isError": is_err}}


def handle(req, rows):
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        return {"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}},
                "serverInfo": {"name": "glossary", "version": "0.1.0"}}}
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}}
    if method == "tools/call":
        params = req.get("params") or {}
        if params.get("name") != "glossary_lookup":
            return reply(rid, f"没有名为 {params.get('name')} 的工具", True)
        term = str((params.get("arguments") or {}).get("term", "")).strip().lower()
        if not term:
            return reply(rid, "缺少参数 term（要查的词）", True)
        hits = [r for r in rows
                if any(term in (r.get(k) or "").lower() for k in ("term", "reading", "meaning"))]
        if not hits:
            return reply(rid, f"词表里没有「{term}」（共 {len(rows)} 条）。")
        out = []
        for r in hits[:MAX_HITS]:
            out.append(f"- {r.get('term', '')}（{r.get('reading', '')}）：{r.get('meaning', '')}")
            if r.get("example"):
                out.append(f"  例：{r['example']}")
        if len(hits) > MAX_HITS:
            out.append(f"…另有 {len(hits) - MAX_HITS} 条命中，把词写得更具体即可。")
        return reply(rid, "\n".join(out)[:MAX_TEXT])
    if rid is None:
        return None
    return {"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": f"不支持的方法：{method}"}}


def main():
    try:
        with open(PATH, "r", encoding="utf-8-sig", newline="") as fh:
            rows = [{k: (v or "").strip() for k, v in r.items()} for r in csv.DictReader(fh)]
    except Exception as exc:
        rows = []
        print(f"cannot read glossary ({PATH}): {exc}", file=sys.stderr)
    print(f"glossary loaded: {len(rows)} rows from {PATH}", file=sys.stderr)
    while True:
        raw = sys.stdin.buffer.readline()
        if not raw:
            return
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as exc:
            print(f"skip unparsable line: {exc}", file=sys.stderr)
            continue
        resp = handle(req, rows)
        if resp is not None:
            sys.stdout.buffer.write((json.dumps(resp, ensure_ascii=False) + "\n").encode("utf-8"))
            sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()
```

同目录的 `glossary.csv`（示例数据，让用户换成自己的词表）：

```csv
term,reading,meaning,example
entropy,熵,系统无序程度的度量，热力学里等于 Q/T 的积分,热传导让熵增加。
enthalpy,焓,定压过程常用的状态量 H = U + pV,定压反应热等于焓变。
```

`README.md` 至少写：这个服务器做什么、数据文件在哪、怎么改词表、启动失败时看 stderr 的哪一行。

注册参数（`<工作区>` = 工作区根目录）：

```
mcp_publish(
  dir     = "mcp/glossary",
  name    = "glossary",
  command = "python",                                  # 不通就换 py 或解释器绝对路径
  args    = ["<工作区>\\.hub\\mcp\\glossary\\server.py"],
  env     = {"PYTHONIOENCODING": "utf-8", "PYTHONUNBUFFERED": "1"}
)
```

发布后按这个清单验证：

1. 看 `mcp_publish` 的返回 / `mcp_status()`：服务器应「已连接」，`serverInfo` 是 `glossary 0.1.0`，工具名里有 `glossary_lookup`。
2. 调用 `mcp__glossary__glossary_lookup`，参数 `{"term": "entropy"}`：应返回带释义与例句的文本。
3. 传一个不存在的词：应返回「词表里没有…」而不是失败（业务上的「查不到」用文本回答，不要用 `isError`）。
4. 故意把 `glossary.csv` 改名再 `mcp_publish` 一次：stderr（应用日志里 `[mcp:glossary]` 那几行）应出现 `cannot read glossary (...)`，工具仍能连上并回「没有」。修好后重新发布。
5. 若状态是「未连接」，按 §7 第 6 条的报错分别排查；改完代码重新 `mcp_publish` 即生效。
