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
/** agentSend 返回前触发的钩子：模拟「发送在途时用户切走」用 */
let duringSend: (() => void) | null = null;

vi.mock("../lib/api", () => ({
  api: {
    agentSend: async (req: { chatId?: string | null; text?: string }) => {
      duringSend?.();
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
  duringSend = null;
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

it("不是它等的那一轮结束、且等的那一轮还在跑：继续留着", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("等我这一轮", [], [], false);
  // 另一条对话的轮次在跑并收尾：等的那轮（T1）还在跑，不能撞上去
  useApp.setState({ streaming: { turnId: "T-别的", chatId: "c9", text: "", thinking: "" } });
  await useApp.getState().flushQueued("T-别的");
  expect(sent).toEqual([]);
  expect(useApp.getState().queued).not.toBeNull();
  // 旧版本这里在「streaming 已空」时也断言留着——那正是队列永久滞留的成因
  //（T1 收尾后任何事件都不再带它的 turn_id），「等的那一轮早已收尾」用例钉住了新语义
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

it("已有一条排队时再排：拒收不覆盖，第一条保留", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  const first = await useApp.getState().queueSend("第一条", [], [], false);
  const second = await useApp.getState().queueSend("第二条", [], [], true);
  expect(first).toBe(true);
  expect(second).toBe(false);
  // 被覆盖 = 用户打的字无声丢失（输入框已清空、还没落库），所以必须拒收
  expect(useApp.getState().queued?.text).toBe("第一条");
  expect(useApp.getState().queued?.interrupting).toBe(false);
  // 拒收的那条不该触发打断
  expect(cancelled).toEqual([]);
  expect(sent).toEqual([]);
});

it("发送期间切走且发送成功：按 send 的返回值清队列（列表长度信号在切走后失真）", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("切走也发出去了", [], [], false);
  useApp.setState({ streaming: null });
  // flushQueued 的守卫通过（此刻还在 c1），agentSend 返回前用户切到 c2：
  // moved 分支不把消息追加进 c2 的列表——旧代码用「列表有没有变长」判断，
  // 会把已发出的消息当成没发出去，回到 c1 时重发一份
  duringSend = () => useApp.setState({ chatId: "c2" });
  await useApp.getState().flushQueued("T1");
  expect(sent.length).toBe(1);
  expect(useApp.getState().queued).toBeNull();
});

it("发送期间切走且发送失败：队列留着（旧长度信号会反向误清、丢排队内容）", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("这条失败且切走了", [], [], false);
  useApp.setState({ streaming: null });
  failNextSend = true;
  duringSend = () => useApp.setState({ chatId: "c2" });
  await useApp.getState().flushQueued("T1");
  failNextSend = false;
  expect(useApp.getState().queued?.text).toBe("这条失败且切走了");
});

it("等的那一轮早已收尾：之后任何一轮收尾都要能补发（afterTurnId 过期不再永久滞留）", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("别滞留", [], [], false);
  // 用户手动发了 T2（T1 已结束），T2 还在跑时按「同对话有轮在跑」留着
  useApp.setState({ streaming: { turnId: "T2", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().flushQueued("T2");
  expect(useApp.getState().queued).not.toBeNull();
  expect(sent).toEqual([]);
  // T2 收尾：endedTurnId(T2) ≠ afterTurnId(T1)，但 T1 早就不在跑——
  // 这一拍必须把排队的发出去，否则凭证条永远挂着「这一轮结束后自动发送」
  useApp.setState({ streaming: null });
  await useApp.getState().flushQueued("T2");
  expect(sent.length).toBe(1);
  expect(useApp.getState().queued).toBeNull();
});

it("等待的那一轮确实还在跑：别的轮次收尾不打扰（原有语义不变）", async () => {
  useApp.setState({ streaming: { turnId: "T1", chatId: "c1", text: "", thinking: "" } });
  await useApp.getState().queueSend("等 T1", [], [], false);
  // 另一条对话的轮次在跑并收尾：队列的 T1 还没消息，继续等
  useApp.setState({ streaming: { turnId: "TX", chatId: "c9", text: "", thinking: "" } });
  await useApp.getState().flushQueued("TX");
  expect(useApp.getState().queued).not.toBeNull();
  expect(sent).toEqual([]);
});
