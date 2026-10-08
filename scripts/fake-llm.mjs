// 本地假模型服务：模拟一个支持 function calling 的 OpenAI 兼容接口。
//
// 用途：没有 API Key 也能端到端验证对话链路（流式增量、工具调用累积、工具结果回灌、
// 第二轮回复）。也可以用来复现「模型乱给参数」这类边界情况。
//
// 用法：
//   node scripts/fake-llm.mjs [端口]          默认 4321
// 然后在「设置 → 模型档案」里加一条：
//   协议 OpenAI 兼容 / 地址 http://127.0.0.1:4321/v1 / 模型 fake-model
//
// 行为脚本：
//   第 1 轮 → 返回一个 tool_call（默认 fs_list，可用环境变量 FAKE_TOOL 改）
//   第 2 轮 → 返回一段带 Markdown 与公式的正文
//   之后   → 每轮都在正文里自报是第几轮

import { createServer } from "node:http";
import fs from "node:fs";

const PORT = Number(process.argv[2] ?? 4321);
const TOOL = process.env.FAKE_TOOL ?? "fs_list";
const TOOL_ARGS = process.env.FAKE_TOOL_ARGS ?? '{"path":"notes"}';
// 想换一段更像样的回答用于演示/截图时，用 FAKE_ANSWER_FILE 指一个 Markdown 文件
const ANSWER_FILE = process.env.FAKE_ANSWER_FILE ?? "";

const log = (...a) => console.log(`[fake-llm ${new Date().toISOString().slice(11, 19)}]`, ...a);

function sse(res, obj) {
  res.write(`data: ${JSON.stringify(obj)}\n\n`);
}

function chunk(delta, finish = null) {
  return {
    id: "chatcmpl-fake",
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model: "fake-model",
    choices: [{ index: 0, delta, finish_reason: finish }],
  };
}

/** 把一段文本切成小块，模拟真实的流式节奏 */
function pieces(text, size = 6) {
  const out = [];
  for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size));
  return out;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function streamChat(res, body) {
  // 把「思考强度」相关字段打出来：验证客户端有没有正确接线（各家写法不同）
  const reasoning = {
    reasoning_effort: body.reasoning_effort,
    enable_thinking: body.enable_thinking,
    thinking: body.thinking,
    temperature: body.temperature,
  };
  if (reasoning.reasoning_effort || reasoning.enable_thinking || reasoning.thinking) {
    log("收到思考参数：", JSON.stringify(reasoning));
  }
  const messages = body.messages ?? [];
  const toolResults = messages.filter((m) => m.role === "tool");
  const hasToolCall = messages.some((m) => m.role === "assistant" && m.tool_calls);

  // 多模态：用户消息里的图片要按协议编码成 parts 数组（`image_url.url` 是内联 data URL）。
  // 打一行日志就能验证客户端有没有真的把图发出来、发的是不是 base64。
  const images = messages.flatMap((m) =>
    Array.isArray(m.content) ? m.content.filter((p) => p?.type === "image_url") : [],
  );
  if (images.length > 0) {
    const heads = images.map((p) => String(p.image_url?.url ?? "").slice(0, 32));
    log(`收到 ${images.length} 张图：${heads.join(" | ")}`);
  }

  res.writeHead(200, {
    "Content-Type": "text/event-stream; charset=utf-8",
    "Cache-Control": "no-cache",
    Connection: "keep-alive",
  });

  // 角色先出来（真实服务也是这样）
  sse(res, chunk({ role: "assistant" }));

  if (toolResults.length === 0 && !hasToolCall) {
    // ---- 第一轮：调用工具 ----
    log(`第 1 轮：请求工具 ${TOOL} ${TOOL_ARGS}`);
    const id = "call_fake_1";
    sse(res, chunk({ tool_calls: [{ index: 0, id, type: "function", function: { name: TOOL, arguments: "" } }] }));
    await sleep(120);
    // 参数也分片，用来验证前端/后端的增量累积
    for (const part of pieces(TOOL_ARGS, 4)) {
      sse(res, chunk({ tool_calls: [{ index: 0, function: { arguments: part } }] }));
      await sleep(60);
    }
    sse(res, chunk({}, "tool_calls"));
    res.write("data: [DONE]\n\n");
    res.end();
    return;
  }

  // ---- 第二轮：给最终回答 ----
  const round = toolResults.length;
  const toolText = toolResults
    .map((m) => String(m.content ?? "").split("\n").slice(0, 3).join(" / "))
    .join("；");
  log(`第 ${round + 1} 轮：已有 ${toolResults.length} 条工具结果，开始输出正文`);

  const answer = ANSWER_FILE && fs.existsSync(ANSWER_FILE)
    ? fs.readFileSync(ANSWER_FILE, "utf8")
    : [
    `我是本地假模型（第 ${round + 1} 轮）。我看到了工具返回的内容：${toolText || "（空）"}`,
    "",
    "### 用于验证渲染的片段",
    "",
    "- 列表项一，含 **加粗** 与 `行内代码`",
    "- 列表项二，含链接 [示例](https://example.com)",
    "",
    "行内公式 $E = mc^2$，行间公式：",
    "",
    "$$\\int_0^1 x^2 \\, dx = \\frac{1}{3}$$",
    "",
    "```python",
    "def sm2(interval, ease):",
    "    return interval * ease",
    "```",
    "",
    "> 引用块也应该正常显示。",
  ].join("\n");

  for (const part of pieces(answer, 8)) {
    sse(res, chunk({ content: part }));
    await sleep(28);
  }
  sse(res, chunk({}, "stop"));
  res.write("data: [DONE]\n\n");
  res.end();
}

const server = createServer((req, res) => {
  if (req.method !== "POST" || !req.url?.includes("/chat/completions")) {
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: { message: `no route: ${req.method} ${req.url}` } }));
    return;
  }

  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", async () => {
    let body = {};
    try {
      body = JSON.parse(raw);
    } catch {
      res.writeHead(400).end(JSON.stringify({ error: { message: "bad json" } }));
      return;
    }
    try {
      await streamChat(res, body);
    } catch (e) {
      log("出错:", e.message);
      try {
        res.end();
      } catch {
        /* ignore */
      }
    }
  });
});

server.listen(PORT, "127.0.0.1", () => {
  log(`listening on http://127.0.0.1:${PORT}/v1  (tool=${TOOL})`);
});
