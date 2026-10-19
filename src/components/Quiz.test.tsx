// 多选题（X 型）作答控件：真实踩过的 bug——选了一个再选第二个，两个勾全消失，
// 之后一个都选不上（作答串 "AC" 被按「非字母分隔」拆成了单个元素 ["AC"]）。
// 这里按真实交互（点复选框）盯住这条回归，顺带守住与 Rust 侧 normalize_keys
// 的契约：作答值是「排序去重的连写串」，全取消时是空串。
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it } from "vitest";
import { QuestionInput } from "./Quiz";
import type { Question } from "../lib/types";

afterEach(() => {
  document.body.replaceChildren();
});

const X_QUESTION: Question = {
  id: "q1",
  type: "x",
  stem: "以下哪些说法正确？",
  options: [
    { key: "A", text: "甲" },
    { key: "B", text: "乙" },
    { key: "C", text: "丙" },
  ],
  answer: ["A", "B"],
  keyPoints: [],
  explanation: "",
  score: 2,
};

/** 和真实使用一致：作答存在外层 state 里，控件是受控的。 */
function Harness({ seen }: { seen: string[] }) {
  const [value, setValue] = useState("");
  seen.push(value);
  return (
    <QuestionInput
      q={X_QUESTION}
      options={X_QUESTION.options}
      value={value}
      onChange={(v) => setValue(v)}
    />
  );
}

function render() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const seen: string[] = [];
  act(() => root.render(<Harness seen={seen} />));
  const boxes = () => Array.from(host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
  const click = (i: number) => act(() => boxes()[i].click());
  return { boxes, click, seen };
}

it("多选：连选两个选项都要勾上（曾经第二个一点，两个勾全灭）", () => {
  const { boxes, click, seen } = render();
  click(0);
  expect(boxes()[0].checked).toBe(true);
  click(1);
  expect(boxes()[0].checked).toBe(true);
  expect(boxes()[1].checked).toBe(true);
  expect(seen.at(-1)).toBe("AB");
});

it("多选：取消一个后其余照常勾着，还能继续选", () => {
  const { boxes, click, seen } = render();
  click(0);
  click(1);
  click(0); // 取消 A
  expect(boxes()[0].checked).toBe(false);
  expect(boxes()[1].checked).toBe(true);
  click(2);
  expect(boxes()[1].checked).toBe(true);
  expect(boxes()[2].checked).toBe(true);
  expect(seen.at(-1)).toBe("BC");
});

it("多选：全部取消后还能重新选（空串不能变成死状态）", () => {
  const { boxes, click, seen } = render();
  click(0);
  click(0); // 取消，作答变回空串
  expect(boxes()[0].checked).toBe(false);
  expect(seen.at(-1)).toBe("");
  click(2);
  expect(boxes()[2].checked).toBe(true);
  expect(seen.at(-1)).toBe("C");
});

it("单选（A1）点第二个选项时替换而不是叠加", () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const seen: string[] = [];
  act(() =>
    root.render(
      <HarnessRadio seen={seen} />,
    ),
  );
  const boxes = () => Array.from(host.querySelectorAll<HTMLInputElement>('input[type="radio"]'));
  act(() => boxes()[0].click());
  act(() => boxes()[1].click());
  expect(boxes()[1].checked).toBe(true);
  expect(boxes()[0].checked).toBe(false);
  expect(seen.at(-1)).toBe("B");
});

function HarnessRadio({ seen }: { seen: string[] }) {
  const [value, setValue] = useState("");
  seen.push(value);
  const q: Question = { ...X_QUESTION, type: "a1" };
  return <QuestionInput q={q} options={q.options} value={value} onChange={setValue} />;
}
