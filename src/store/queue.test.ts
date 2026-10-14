// 排队发送（Alt+Enter / Ctrl+Enter）的回归网。
//
// 这里最容易写错的是三件事：**等的是哪一轮、发到哪条对话、什么时候才清队列**
// （清早了会把用户打的字弄丢，清晚了或者发串台更糟）。所以逐条钉住。
//
// 不碰真后端：`../lib/api` 整个换成假的，只记「发出去的请求」。

import { beforeEach, expect, it, vi } from "vitest";

let failNextSend = false;
const sent: { chatId?: string | null; text?: string }[] = [];
const cancelled: string[] = [];

vi.mock("../lib/api", () => ({
  api: {
    agentSend: async (req: { chatId?: string | null; text?: string }) => {
      if (failNextSend) throw new Error("没有 API Key");
      sent.push(req);
      return {
        turnId: "turn-new",
        chatId: req.chatId ?? "c1",
        mode: "study",
        message: { id: "m-" + sent.length, role: "user", blocks: [], createdAt: "", meta: {} },
      };
    },
    agentCancel: async (turnId: string) => {
      cancelled.push(turnId);
      return true;
    },
    agentNewChat: async () => "c1",
  },
  errText: (e: unknown) => String(e),
}));

import { useApp } from "./app";

beforeEach(() => {
  sent.length = 0;
  cancelled.length = 0;
  failNextSend = false;
  useApp.setState({
    chatId: "c1",
    messages: [],
    streaming: null,
    queued: null,
    chatError: null,
    view: "topic",
  });
});

it("正在跑的时候 Alt+Enter：只排队，不打断", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("接着说", [], [], false);
  const q = useApp.getState().queued;
  expect(q?.text).toBe("接着说");
  expect(q?.afterTurnId).toBe("T1");
  expect(q?.chatId).toBe("c1");
  expect(q?.interrupting).toBe(false);
  expect(cancelled).toEqual([]);
  expect(sent).toEqual([]);
});

it("正在跑的时候 Ctrl+Enter：排队 + 打断那一轮", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("换个方向", [], [], true);
  expect(useApp.getState().queued?.interrupting).toBe(true);
  expect(cancelled).toEqual(["T1"]);
  expect(sent).toEqual([]);
});

it("没在跑的时候：直接发，不入队", async () => {
  await useApp.getState().queueSend("直接发", [], [], false);
  expect(sent.length).toBe(1);
  expect(useApp.getState().queued).toBeNull();
});

it("不是它等的那一轮结束：继续留着", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("等我这一轮", [], [], false);
  useApp.setState({ streaming: null });
  await useApp.getState().flushQueued("T-别的");
  expect(sent).toEqual([]);
  expect(useApp.getState().queued).not.toBeNull();
});

it("等的那一轮结束了：发出去并清队列", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("轮到我了", [], [], false);
  useApp.setState({ streaming: null });
  await useApp.getState().flushQueued("T1");
  expect(sent.length).toBe(1);
  expect(sent[0]?.text).toBe("轮到我了");
  expect(useApp.getState().queued).toBeNull();
});

it("用户切到别的对话：留着，等回到那条对话再发", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("回 c1 再发", [], [], false);
  useApp.setState({ streaming: null, chatId: "c2" });
  await useApp.getState().flushQueued("T1");
  expect(sent).toEqual([]);
  expect(useApp.getState().queued).not.toBeNull();

  // 回到 c1（loadChat 会在切回来后调 flushQueued，不带 turnId）
  useApp.setState({ chatId: "c1" });
  await useApp.getState().flushQueued();
  expect(sent.length).toBe(1);
  expect(useApp.getState().queued).toBeNull();
});

it("它那条对话又有一轮在跑：再等等，别撞上并发轮次", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("等等再发", [], [], false);
  // 第一轮结束了，但用户手动又发了一条（新一轮起来了）
  useApp.setState({ streaming: { turnId: "T2", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().flushQueued("T1");
  expect(sent).toEqual([]);
  expect(useApp.getState().queued).not.toBeNull();
});

it("发送失败：队列留着，别把用户打的字弄丢", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("这条会失败", [], [], false);
  useApp.setState({ streaming: null });
  failNextSend = true;
  await useApp.getState().flushQueued("T1");
  expect(sent).toEqual([]);
  expect(useApp.getState().queued?.text).toBe("这条会失败");
});

it("取消排队：清掉，也不再发", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("不发了", [], [], false);
  useApp.getState().cancelQueued();
  useApp.setState({ streaming: null });
  await useApp.getState().flushQueued("T1");
  expect(useApp.getState().queued).toBeNull();
  expect(sent).toEqual([]);
});
