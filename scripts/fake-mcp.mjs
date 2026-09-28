// 一个极简的 MCP 服务器（stdio + 换行分隔 JSON-RPC），用来验证客户端实现。
// 用法：node scripts/fake-mcp.mjs
import { createInterface } from "node:readline";

const TOOLS = [
  {
    name: "echo",
    description: "把输入原样返回，用来验证链路",
    inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  },
  {
    name: "now",
    description: "返回当前时间",
    inputSchema: { type: "object", properties: {} },
  },
];

const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.method === "notifications/initialized") return;
  let result = null;
  switch (msg.method) {
    case "initialize":
      result = { protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "fake-mcp", version: "0.1.0" } };
      break;
    case "tools/list":
      result = { tools: TOOLS };
      break;
    case "tools/call": {
      const { name, arguments: args = {} } = msg.params;
      if (name === "echo") {
        result = { content: [{ type: "text", text: `echo: ${args.text ?? "(空)"}` }] };
      } else if (name === "now") {
        result = { content: [{ type: "text", text: `现在是 ${new Date().toLocaleString("zh-CN")}` }] };
      } else {
        result = { content: [{ type: "text", text: `没有这个工具：${name}` }], isError: true };
      }
      break;
    }
    default:
      result = {};
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }) + "\n");
});
process.stderr.write("[fake-mcp] 已启动\n");
