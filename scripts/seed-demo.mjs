// 生成演示工作区：一个「线性代数」主题，含笔记 / 卡片 / 任务 / 会话 / PDF。
//
// 用途：刚 clone 下来就能看到界面长什么样、各模块怎么联动；
// 也用来做冒烟测试（有数据才好验证渲染）。
// 用法：node scripts/seed-demo.mjs [工作区根目录]

import { mkdirSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";

const root = resolve(process.argv[2] ?? "workspace");
const TOPIC = "线性代数";
const topicDir = resolve(root, TOPIC);

const now = new Date();
const iso = (d) => d.toISOString();
const days = (n) => new Date(now.getTime() + n * 86400_000);
const minutes = (n) => new Date(now.getTime() + n * 60_000);

const write = (rel, content) => {
  const p = resolve(topicDir, rel);
  mkdirSync(resolve(p, ".."), { recursive: true });
  writeFileSync(p, content, "utf8");
};

const topicId = randomUUID();

// ---------------------------------------------------------------- 元数据

write(
  "topic.json",
  JSON.stringify(
    {
      id: topicId,
      name: "线性代数",
      emoji: "📐",
      description: "看懂机器学习论文里的矩阵分解与特征值，能自己推导 PCA",
      tags: ["数学", "机器学习基础"],
      stage: "learn",
      createdAt: iso(days(-21)),
      updatedAt: iso(now),
      lastOpenedAt: iso(now),
    },
    null,
    2,
  ) + "\n",
);

write(
  "README.md",
  `# 线性代数

看懂机器学习论文里的矩阵分解与特征值，能自己推导 PCA。

> 这份 README 会被 agent 当作背景资料读取，可以随手补充学习目标、参考书目、考试时间。

## 学习目标

- 能说清「矩阵就是一个线性变换」这句话的确切含义
- 能手算 3×3 以内的特征值与特征向量
- 能独立推导 PCA 的两个等价视角（最大方差 / 最小重构误差）

## 参考资料

- 3Blue1Brown《线性代数的本质》（视频）
- Gilbert Strang, Introduction to Linear Algebra
- materials/demo-lecture.pdf

## 备注

- 我的基础：会算行列式，但矩阵乘法一直靠死记
- 每周三、周日晚上各 1 小时
`,
);

// ---------------------------------------------------------------- 笔记

write(
  "notes/特征值与特征向量.md",
  `---
title: 特征值与特征向量
tags: 特征值, 线性变换, 对角化
created: ${iso(days(-9)).slice(0, 16).replace("T", " ")}
---

# 特征值与特征向量

## 直觉

矩阵乘向量 = 把这个向量「拧」一下。绝大多数向量被拧完之后方向会变，
但总有一些特殊方向只被**拉长或压短**、方向不动——这些方向就是特征向量，
拉长的倍数就是特征值。

写成式子：$Av = \\lambda v$，其中 $v \\neq 0$。

## 怎么算

1. $Av = \\lambda v \\iff (A - \\lambda I)v = 0$ 要有非零解
2. 所以 $\\det(A - \\lambda I) = 0$ —— 这就是**特征多项式**
3. 解出 $\\lambda$，再代回去解 $(A-\\lambda I)v=0$ 得到特征向量

例子：$A = \\begin{pmatrix} 2 & 1 \\\\ 1 & 2 \\end{pmatrix}$

$$\\det(A-\\lambda I) = (2-\\lambda)^2 - 1 = (\\lambda-1)(\\lambda-3)$$

所以 $\\lambda_1 = 1$，$\\lambda_2 = 3$；对应特征向量分别是 $(1,-1)$ 与 $(1,1)$。

## 为什么重要

- **对角化** $A = P\\Lambda P^{-1}$：把「拧」分解成「换坐标 → 纯拉伸 → 换回来」
- **PCA**：协方差矩阵的特征向量就是主成分方向，特征值就是该方向的方差
- **稳定性**：迭代 $x_{k+1} = Ax_k$ 最终会被最大特征值的方向主导

## 容易踩的坑

- 特征值可能是复数（旋转矩阵就是），这时实特征向量不存在
- 特征值全为正 $\\Rightarrow$ 正定，但特征值有正有负也可能是可逆的
- 几何重数可能小于代数重数，这时矩阵不能对角化
`,
);

write(
  "notes/学习地图.md",
  `---
title: 学习地图
tags: 规划
created: ${iso(days(-20)).slice(0, 16).replace("T", " ")}
---

# 线性代数 · 学习地图

按依赖顺序排列，前面的没弄懂不要跳：

1. **向量与线性组合** —— 什么是「线性」，什么是「组合」
2. **矩阵作为变换** —— 列 = 基向量的落点（3B1B 的核心视角）
3. **矩阵乘法** —— 变换的复合，为什么是那样算的
4. **行列式** —— 面积/体积的伸缩因子，正负号的含义
5. **逆、秩、列空间** —— 什么时候解存在、什么时候唯一
6. **特征值 / 特征向量** —— 不改变方向的那些方向
7. **对角化与 SVD** —— 换基之后一切都简单
8. **PCA 与最小二乘** —— 落到机器学习上的用途

## 自检问题

- 能否不用公式，用一句话解释「矩阵是线性变换」？
- 为什么 $AB \\neq BA$？（用变换的顺序解释）
- 行列式为 0 意味着什么？（至少三种说法）
`,
);

// ---------------------------------------------------------------- 卡片

const cards = [
  {
    front: "特征值的定义式是什么？（并说明为什么要求 $v \\neq 0$）",
    back: "$Av = \\lambda v$。要求 $v\\neq 0$ 是因为 $v=0$ 对任何 $\\lambda$ 都成立，无法定义出「这个矩阵的特征值」。",
    tags: ["特征值", "定义"],
    module: "特征值",
  },
  {
    front: "为什么求特征值要解 $\\det(A-\\lambda I)=0$？",
    back: "因为 $(A-\\lambda I)v=0$ 要有**非零解**，等价于这个矩阵不可逆，也就是行列式为 0。",
    tags: ["特征值", "推导"],
    module: "特征值",
  },
  {
    front: "矩阵 $\\begin{pmatrix}2&1\\\\1&2\\end{pmatrix}$ 的特征值是？",
    back: "$\\lambda = 1$ 与 $\\lambda = 3$，特征向量分别是 $(1,-1)$ 与 $(1,1)$。",
    tags: ["计算"],
    module: "特征值",
  },
  {
    front: "PCA 里，协方差矩阵的特征向量和特征值分别代表什么？",
    back: "特征向量 = 主成分的方向；特征值 = 数据在该方向上的方差大小。按特征值从大到小排就是主成分顺序。",
    tags: ["PCA", "应用"],
    module: "PCA",
  },
].map((c, i) => ({
  id: randomUUID(),
  front: c.front,
  back: c.back,
  source: "notes/特征值与特征向量.md",
  tags: c.tags,
  module: c.module,
  createdAt: iso(days(-8 + i)),
  srs:
    i === 0 || i === 1
      ? {
          // 前两张今天到期，侧栏与日程页能立刻看到待复习
          ease: 2.5,
          intervalDays: 3,
          repetitions: 2,
          lapses: i,
          due: iso(minutes(-30 + i * 10)),
          lastReview: iso(days(-3)),
          reviews: 2 + i,
        }
      : {
          ease: 2.5,
          intervalDays: 0,
          repetitions: 0,
          lapses: 0,
          due: iso(now),
          lastReview: null,
          reviews: 0,
        },
  ankiNoteId: null,
}));

write("cards/cards.jsonl", cards.map((c) => JSON.stringify(c)).join("\n") + "\n");

// ---------------------------------------------------------------- 计划

const tasks = [
  {
    title: "手推一遍 3×3 矩阵的特征多项式",
    detail: "别查公式，自己展开一次，重点感受「为什么是 n 次多项式」",
    status: "doing",
    priority: 3,
    due: iso(minutes(180)),
    estimateMin: 40,
    stage: "learn",
    module: "特征值",
  },
  {
    title: "看完 3B1B 特征向量那一集并做笔记",
    status: "todo",
    priority: 2,
    due: iso(days(-1)),
    estimateMin: 25,
    stage: "preview",
    module: "特征值",
  },
  {
    title: "把 PCA 的两种视角各写一段解释",
    status: "todo",
    priority: 2,
    due: iso(days(3)),
    estimateMin: 60,
    stage: "learn",
    module: "PCA",
  },
  {
    title: "复习到期卡片（10 分钟）",
    status: "todo",
    priority: 3,
    due: iso(minutes(60)),
    estimateMin: 10,
    stage: "review",
    module: null,
  },
  {
    title: "整理第一周的学习地图",
    status: "done",
    priority: 1,
    due: iso(days(-14)),
    estimateMin: 30,
    stage: "preview",
    module: null,
  },
];

write(
  "plan/tasks.jsonl",
  tasks.map((t) => JSON.stringify({ id: randomUUID(), detail: t.detail ?? "", createdAt: iso(days(-10)), updatedAt: iso(now), doneAt: t.status === "done" ? iso(days(-14)) : null, ...t })).join("\n") + "\n",
);

// ---------------------------------------------------------------- 会话

const sessions = [
  { offset: 0, stage: "learn", title: "特征值：从几何直觉到计算", cards: 3, notes: 1 },
  { offset: -3, stage: "review", title: "复习：矩阵作为变换", cards: 0, notes: 0 },
  { offset: -7, stage: "preview", title: "预习：线性代数的全局地图", cards: 0, notes: 1 },
  { offset: -14, stage: "learn", title: "行列式的三种解释", cards: 1, notes: 0 },
];

for (const s of sessions) {
  const started = new Date(now.getTime() + s.offset * 86400_000 - 3600_000);
  const ended = new Date(started.getTime() + 52 * 60_000);
  const id = randomUUID();
  write(
    `sessions/${id}.json`,
    JSON.stringify(
      {
        id,
        topicId,
        topicSlug: TOPIC,
        title: s.title,
        stage: s.stage,
        goals: s.stage === "learn" ? ["搞懂特征值的几何含义", "能手算 2×2 与 3×3"] : [],
        materials: s.offset === 0 ? ["materials/demo-lecture.pdf"] : [],
        chatId: randomUUID(),
        startedAt: iso(started),
        endedAt: iso(ended),
        summary:
          s.stage === "learn"
            ? "把「特征向量 = 不被改变方向的方向」这句话坐实了；手算了 (2,1;1,2) 的特征值。还剩一个问题没解决：为什么对称矩阵一定能正交对角化。"
            : "（历史会话）",
        highlights: s.stage === "learn" ? ["特征多项式 = 那个让矩阵变奇异的多项式", "特征值可能是复数"] : [],
        openQuestions: s.offset === 0 ? ["对称矩阵为什么一定能正交对角化？"] : [],
        cardsCreated: s.cards,
        notesCreated: s.notes,
      },
      null,
      2,
    ) + "\n",
  );
}

// ---------------------------------------------------------------- 说明

writeFileSync(
  resolve(root, "README.md"),
  `# 演示工作区

这是 scripts/seed-demo.mjs 生成的示例数据，用来让应用一打开就有东西看。

里面的每个子目录都是一个学习主题。想清空：
\`\`\`bash
rm -rf "${root}"
\`\`\`
`,
  "utf8",
);

console.log(`演示工作区已生成：${root}`);
console.log(`  主题：${TOPIC}（${cards.length} 张卡片、${tasks.length} 条任务、4 次会话）`);
console.log("  记得把 PDF 也生成出来：node scripts/make-demo-pdf.mjs \"<工作区>/线性代数/materials/demo-lecture.pdf\"");
