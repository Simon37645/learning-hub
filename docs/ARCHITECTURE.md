# 学习中枢 · 架构

> 面向要改这个项目的人（包括未来的自己和 AI agent）。读完应该能回答：
> 「加一个工具要改哪几个文件」「为什么 PDF 要自己渲染」「对话状态存在哪」。

## 1. 分层

```
┌──────────────────────────────────────────────────────────────┐
│  前端 React / TS                                             │
│  store/app.ts  ← 唯一状态源，订阅 4 个事件频道               │
│  components/*  ← 只负责渲染与收集用户输入                    │
├──────────────────────────────────────────────────────────────┤
│  IPC：commands/*.rs   （60 个 #[tauri::command]）             │
│  纯搬运：校验参数 → 调领域逻辑 → 组装视图对象                │
├──────────────────────────────────────────────────────────────┤
│  agent/     对话循环 · 模型接入(SSE) · 工具注册表 · 提示词   │
│  viewer/    内置浏览器状态机 · 网页正文提取                  │
│  domain/    主题 / 笔记 / 卡片 / 任务 / 会话（纯数据）       │
│  store.rs   原子写 · JSON/JSONL · 目录扫描                   │
│  config.rs · net.rs · paths.rs · error.rs                    │
└──────────────────────────────────────────────────────────────┘
```

依赖方向单向向下。`agent` 与 `viewer` 需要读配置、发事件、访问工作区，
统一通过 `state::AppCore`（`Arc<AppCore>` 到处传），不下沉到 `commands`。

**为什么没有数据库**：这个应用的寿命以「年」计，用户随时可能换电脑、同步网盘、
用别的编辑器改笔记。磁盘上的普通文件是最耐久的接口，所以宁可每次扫目录也不引入 SQLite。

## 2. 一次对话的完整链路

```
用户按 Enter
  → 前端 store.send()
  → agent_send 命令：用户消息先落库，返回 (turnId, message)
  → 后端 tokio::spawn 跑 run_turn（不占 IPC 通道）
      ┌─ 载入 transcript（.hub/chats/<id>.jsonl）
      ├─ 组装系统提示词（主题 / 阶段 / 笔记清单 / 资料清单 / 工具清单）
      ├─ 循环（最多 max_iterations 次）：
      │    请求模型（流式 SSE）
      │      ├─ Text/Thinking 增量 → 事件 hub://agent {kind:"delta"}
      │      ├─ ToolCall 增量按 index 累积
      │      └─ 流结束 → 组装 assistant 消息（含 tool_use 块）落库 + 发事件
      │    若有工具调用 → 逐个执行：
      │      ├─ 风险 < 阈值 → 直接执行
      │      └─ 否则 → 发 tool_approval 事件，等前端 oneshot 回执（最长 10 分钟）
      │      └─ 结果作为 tool 消息回灌到 messages，进入下一轮
      └─ 结束 → 发 finished 事件
```

几个关键决定：

- **用户消息由命令同步落库再返回**，前端不做乐观插入，避免消息重复。
- **工具串行执行**：学习场景里工具之间有依赖（先 `fs_search` 再 `fs_read`），并行反而容易互相踩。
- **工具结果不单独发事件**，而是累积到 `activities`，前端把工具卡片挂在触发它的 assistant 消息上；
  历史回看时从 transcript 里的 `tool` 消息重建，两条路径渲染同一套 UI。
- **上下文预算**：超限时按「完整轮次」从最早开始丢（`trim_to_budget`），
  绝不切断 `tool_use` 与 `tool_result` 的配对——切开会让服务端直接报错。

## 3. 事件协议

| 频道 | 载荷 | 语义 |
| --- | --- | --- |
| `hub://agent` | `AgentEvent`（tagged by `kind`） | 对话进展：`turn_started` `delta` `message` `iteration` `tool_approval` `tool_started` `tool_finished` `usage` `finished` `failed` |
| `hub://viewer` | `ViewerEvent` | 内置浏览器：`sync` `snapshot_request` `goto` `reload` `updated` |
| `hub://topics` | `TopicsEvent` | 主题集合变化：`created` `updated` `deleted` `refresh` |
| `hub://memory` | `MemoryEvent` | 长期记忆变化（agent 记下/删掉，或用户改过）：前端把 `memoryTick` +1，面板与角标据此重拉 |
| `hub://toast` | `{level, message}` | 轻提示 |

**命名注意**：带 tag 的枚举，serde 的 `rename_all` 只作用于变体名，
所以事件里变体的字段仍是 snake_case（`turn_id`），而结构体字段是 camelCase（`createdAt`）。
前端 `src/lib/types.ts` 严格照此声明，改任一端的字段都要同时改另一边。

## 4. 内置浏览器

核心权衡：**渲染在前端，状态在 Rust**。

```
        ┌────────────── 用户操作 ──────────────┐
        ▼                                      │
前端 Viewer.tsx ── viewer_report_state ──→ ViewerService（Rust）
   │  ▲                                        │
   │  └── hub://viewer {goto|reload|sync} ─────┘
   │                                           ▲
   └──── viewer_load_text / viewer_load_bytes ─┘   （Rust 侧做路径校验后读文件）
                                               ▲
agent viewer_* 工具 ────────────────────────────┘
```

- 每个标签页在 Rust 侧有一份 `ViewerTab`：类型、来源、页码、滚动位置、**文本快照**。
- 文本快照的来源，按优先级：
  1. Rust 直接提取（本地 Markdown/文本直读，PDF 走 `pdf-extract`，网页走正文提取器）
  2. 前端上报（PDF 用 pdf.js 逐页抽取，带页码；前端通过 `registerSnapshotProvider` 注册回调，
     Rust 需要时发 `snapshot_request` 事件，前端回 `viewer_report_snapshot`）
- **PDF 为什么自己画**：内嵌的原生阅读器无法被脚本控制，agent 就永远没法「翻到第 3 页读那一段」。
  用 pdf.js 逐页渲染 + 抽取文本后，agent 才能检索、引用页码、指哪打哪。
- 网页的「阅读模式」显示的正是 agent 读到的那份正文——用户可以直接看到 agent 的视野。

## 5. 磁盘格式

```
<工作区>/
├── README.md                 工作区说明
├── .hub/                     应用内部状态
│   ├── chats/<id>.jsonl      对话记录（一行一条 ChatMessage）
│   ├── memory/memories.jsonl 长期记忆（全局：跨主题都成立）
│   └── trash/                所有删除操作的回收站
└── <主题>/
    ├── topic.json            TopicMeta
    ├── README.md             agent 每次都会读的背景资料
    ├── notes/*.md            可选 front matter（title / tags）
    ├── materials/**          任意文件
    ├── cards/cards.jsonl     Card（含 SRS 状态内联）
    ├── plan/tasks.jsonl      PlanTask
    ├── sessions/<id>.json    StudySession
    └── .hub/
        ├── chats/<id>.jsonl       该主题的对话（一行一条 ChatMessage）
        ├── chats/<id>.meta.json   该对话的名字/置顶/归档/分叉血缘（可选，见第 7 节）
        └── memory/memories.jsonl  该主题的长期记忆
```

- 写文件一律走 `store::atomic_write`（临时文件 + rename），不会出现半截文件。
- JSONL 单行损坏只跳过该行并打日志，不毁掉整个文件。
- 主题列表靠扫目录得到；`topic.json` 缺失时会**自动认领**该目录并补写元数据，
  所以用户手动扔进来的文件夹也能直接当主题用。
- 路径安全：agent 给的一切路径都经 `paths::resolve_in_root` 归一化并校验前缀，
  `../`、绝对路径、越界软链一律拒绝。

### 主题与章节（父子主题）

「这门课我只学某一章」是个高频需求，所以主题支持父子：**子主题也是工作区根下的一个目录**，
层级关系只记在 `topic.json` 的 `parent` 字段里（存**父主题的 id**，不是目录名，改名不断链）。

为什么不做成目录嵌套：`list()` 只扫工作区根的一级子目录、任何一级子目录都会被自动认领成主题、
`slug` 就是目录名且长期驻留在会话记录/浏览器标签页/InkNote 映射里——嵌套会让这些全部要改，
而收益只是文件管理器里看起来更整齐。平铺 + `parent` 字段则一行都不用动。

继承规则（`Workspace::ancestors` / `descendants` 走祖先链）：

| 数据 | 子主题 | 说明 |
| --- | --- | --- |
| `materials/`、`kb/`、`notes/` | 可见，只读 | 资料面板分组显示「继承自『父主题』」；KB 索引与系统提示词都收进来 |
| 卡片 / 计划 / 会话 / 测验 / 讲解 / 对话 | 严格本主题 | 「这章的复习队列」就该是这章的 |
| 技能 / MCP 开关 | 沿链取并集 | 父主题关掉的，子主题也关（父主题私有技能子主题也能用） |
| 删除 | 级联进 `.hub/trash` | 子主题一起进回收站，手动搬回来自动恢复父子关系 |

引用格式：继承来的文件在提示词与 KB 里都写成**工作区相对路径**（`线性代数/materials/lecture1.pdf`），
用户点这个引用时前端按第一段认出主题并切过去打开——所以父主题的文件不需要复制一份。

## 6. 长期记忆

目的：让 agent 记住**用户是什么样的人**和**以后要注意的地方**（这是他第三次在同一个点上出错、
他不喜欢一上来就给公式、他还没学过向量），而不是每开一条新对话就重新认识一遍。

### 为什么自己做，而不是接一套记忆服务

| 现成方案 | 为什么没直接用 |
| --- | --- |
| mem0 / Letta / Zep 等记忆层 | 它们的大头是「向量检索挑几条记忆塞进上下文」。个人学习场景的记忆量在几十条量级，**全量注入反而更准**（不会因为相似度没排上而漏掉关键的一条），也少一个向量库和一套 Key 要维护 |
| 各家 agent 的 `CLAUDE.md` / `AGENTS.md` 约定 | 那是**用户手写**的项目说明，agent 自己不会往里写；这里要的是 agent 能主动记 |

所以规则很简单：**磁盘上的 JSONL 就是记忆本身**，能直接用记事本打开、改、删、进版本库。

### 分层与继承

```
<工作区>/.hub/memory/memories.jsonl                 全局：称呼、作息、通用偏好
<工作区>/<主题>/.hub/memory/memories.jsonl          本主题：这一科里的易错点、说好的讲法
```

- **作用域由文件位置决定**，不写进记录里——同一份文件搬到别处语义就变了，记两个地方迟早不一致。
- 主题级的记忆沿父子链继承：学「某一章」时，整门课里记下的「他总把 A 和 B 搞混」照样生效。
  注入顺序是**本主题 → 父主题 → 全局**，同一条内容（按去标点后的指纹判重）只注入一次。
- 六类记忆（`fact` 情况 / `preference` 偏好 / `goal` 目标 / `pitfall` 注意 / `style` 讲法 / `gap` 缺口）
  在提示词里**按类分组**输出。分类不是装饰：模型据此决定这条怎么用。
- 每条带**日期**，模型据此判断还成不成立（已经考完的考试不该再当目标）。

### 写入与注入

```
agent 对话中判断「这事以后还用得上」
  → memory_write（默认当前主题，跨主题才写 global）
  → 内容指纹去重：同义内容更新原条，而不是新增
  → 落盘 + hub://memory 事件（前端面板/角标刷新）

每轮对话开始
  → AppCore::memory_digest（本主题 + 父主题 + 全局，按优先级去重截断）
  → 塞进系统提示词「长期记忆」段（在「当前主题」之前：讲什么之前先知道该对谁讲）
  → 记录 last_used / use_count（面板上显示「用过几次」）
```

几个刻意的取舍：

- **注入走系统提示词，不走检索工具**：不需要模型先想到「我该去查记忆」——那一步经常忘。
  条数（40）与字符数（1600）都有硬上限，不会把上下文预算吃光。
- **写入不弹审批**。记忆是可撤销的轻写入（面板里一键删），每条都要确认会把用户打断到直接关掉功能；
  真正危险的动作是删文件，那才走审批。
- **不做「自动抽取」的后台模型调用**：那会凭空多一次模型请求、多一份 API 费用，
  而且抽出来的东西用户看不见。现在每条记忆都来自对话里的显式动作，都能在面板里对账。
- **记账与落盘分开**：`last_used` 是每次注入都要改的，逐条写盘会让一轮对话多出几十次文件写；
  所以只在内存里改，切换主题（`topic_open`）或关窗口时 `memory_flush` 一起写。
- **单个作用域 60 条上限**：到顶时报错并让用户去面板清理，而不是悄悄丢掉最旧的一条。
- **总开关只影响「注入 + 工具暴露」**，不影响已有记忆的查看与编辑——
  用户临时不想要它干扰，不该以看不到自己记了什么为代价。

系统提示词预览（`prompt_preview`）走的是 `memory_peek`：同样组装那一段但**不**更新计数，
免得连点几次预览把「用过几次」刷满。

## 7. 会话元数据（重命名 / 置顶 / 分叉 / 归档）

侧栏里每条对话的「⋯」菜单提供四件事，都记在**侧车文件**里：

| 操作 | 语义 |
| --- | --- |
| 重命名 | 空标题＝恢复「第一句话」的自动标题（侧车文件随之删掉） |
| 置顶 | 排在该主题对话列表最前面；与归档互斥（置顶会自动取消归档） |
| 分叉 | 在**最后一个完整回合**处截断复制成新对话，原对话不动 |
| 归档 | 收进侧栏的「已归档」分组，不占日常视线；随时可取消 |

为什么放 `chats/<id>.meta.json` 而不是插进 jsonl：

- jsonl 的每一行都是 `ChatMessage`，模型读的、前端渲染的、将来导出的都是它。
  插一行「特殊行」意味着**每一处**读对话的地方都要先学会跳过它——漏一处就是一条假消息。
- 侧车文件与对话**同名同目录**（`<id>.jsonl` + `<id>.meta.json`），搬主题、进回收站都一起走
  （`chat_move` 会把两个文件一起 rename）。
- 元数据**可缺省**：没改过名就永远不会有这个文件，老对话读出来就是默认值
  （`Chats::load` 解析失败也只打日志、退回默认，绝不让一条坏元数据把对话锁死）。

### 几个刻意的取舍

- **排序在 Rust 里算一次**（置顶 → 最近使用 → 归档沉底），前端直接用后端返回的顺序渲染。
  两边各排一次，迟早会出现「界面上看是置顶了、点进去却排在后面」。
- **条数上限（60）放在排序之后**：否则置顶的那条可能因为文件太旧被截掉。
- **分叉点定在最后一条 assistant 消息之后**，而不是字面上的最后一条：
  末尾若挂着「用户刚提问、模型还没答」，复制过去会让新对话一开头就欠一个回答。
- **归档只是元数据**，文件不动——归档的东西必须还能被检索到（`agent_transcripts`、
  全文检索都照样看得见），语义是「先收起来」而不是「藏起来」。
- **删除走回收站**，与主题、笔记一致：`chat_delete` 把 jsonl 移进 `.hub/trash/`，
  顺带删掉侧车（对话能从回收站还原，名字再起一个即可）。
- 菜单里**没有「删除」以外的破坏性动作**；归档是默认推荐的那条路（确认框里也这么写）。

## 8. 扩展点

### 加一个工具

1. 在 `src-tauri/src/agent/tools/` 选一个文件（或新建），实现 `Tool`：

```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn description(&self) -> &'static str { "给模型看的说明，写清何时用/参数/返回" }
    fn schema(&self) -> Value { object_schema(json!({ "x": str_prop("说明") }), &["x"]) }
    fn risk(&self) -> Risk { Risk::Write }        // 决定要不要打断用户
    fn summarize(&self, input: &Value) -> String { format!("做事 {}", arg_str(input,"x").unwrap_or_default()) }
    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> { /* ... */ }
}
```

2. 在 `agent/tools/mod.rs` 的 `registry()` 里 `register`。
3. 不需要动主循环、提示词、前端——工具清单与摘要都是自动生成的。

### 加一个模型服务商

实现 `agent/provider/mod.rs` 的 `LlmProvider`（`kind` / `label` / `stream`），
在 `provider::build` 里按 `ProviderKind` 分发，再在 `config.rs` 的枚举与设置页加一项。
共用部分已经抽好了：SSE 解析（`pump_sse`）、错误整形（`error_from_response`）。

### 加一种查看器类型

1. `viewer::ViewerKind` 加变体 + `detect()` 里的扩展名映射。
2. `commands/viewer.rs` 里决定用 `viewer_load_text` 还是 `viewer_load_bytes`。
3. 前端 `Viewer.tsx` 的 `TabBody` 加一个分支，并在 `registerSnapshotProvider` 里上报文本
   （不上报也能用，Rust 会退回到自己提取）。

### 加一个领域模块

在 `domain/` 建文件 → 实现数据结构与纯函数 → 加单测 → `commands/` 加命令 →
`lib.rs` 的 `generate_handler!` 注册 → `src/lib/api.ts` 加类型化封装。
`domain` 不依赖 Tauri，所以可以脱离应用直接 `cargo test`。

## 9. 安全与隐私

- **密钥不出后端**：`PublicConfig` 只给 `hasApiKey` 与 `sk-12…cd` 形式的提示；前端拿不到明文。
- **文件访问有边界**：agent 与查看器都只能碰当前主题目录内的路径。
- **删除可撤销**：`fs_delete` 只删空目录，界面上的删除一律进 `.hub/trash/`。
- **网页内容一律净化**：模型/网页产出的 Markdown 经 DOMPurify（html+svg+mathml 白名单）后才进 DOM；
  本地 HTML 用 `sandbox=""` 的 iframe 渲染，不允许脚本执行。
- **CSP** 在 `tauri.conf.json` 里收敛：脚本只允许 `self`，iframe 允许 http(s)，字体只允许 `self`/`data:`。
- 没有任何遥测、账号、云同步；所有数据只在本机。

## 10. 已知取舍

| 决定 | 原因 | 代价 |
| --- | --- | --- |
| 不用数据库，直接读文件 | 数据可带走、可手改、耐迁移 | 主题多时列表会慢（几百个以内无感） |
| PDF 自绘 | 要让 agent 能操作文档 | 暂不支持选中文字（待补文本层） |
| 工具串行 | 学习类工具存在依赖关系 | 批量操作略慢 |
| 不发工具结果事件 | 减少事件噪声 | 前端要同时处理「实时」与「历史」两条渲染路径 |
| 删除走回收站 | 学习资料误删代价高 | 需要用户自己清理 `.hub/trash/` |

## 11. 第二轮与第三轮新增的模块

| 模块 | 位置 | 说明 |
| --- | --- | --- |
| 知识库 | `kb.rs` | 把 `kb/`+`materials/`+`notes/` 抽成带来源的文本块，索引缓存在 `.hub/kb.json`（按 mtime 增量刷新）。工具：`kb_build` / `kb_search` |
| 讲解方案 | `commands/lesson.rs` + `agent/tools/lesson.rs` | 讲解模式的骨架：方案落成 `lessons/<id>.json`，进度注入提示词，跨轮次不迷路 |
| 测验 | `domain/quiz.rs` + `commands/quiz.rs` + `agent/tools/quiz.rs` | 六种题型；客观题本地判分，主观题把采分点交给模型逐条对照（`quiz_grade_subjective`） |
| 技能 | `skills.rs` | 扫 `SKILL.md`（frontmatter 只解析 name/description，不引 YAML）。渐进式披露：提示词里只给名字+适用场景 |
| MCP | `mcp.rs` + `agent/tools/mcp.rs` | 手写的 stdio JSON-RPC 客户端（换行分帧）。握手 → `tools/list` → 包装成 `mcp__<服务器>__<工具>` |
| 沙箱 | `state.rs` 的 `is_inside_workspace` / `needs_escalation` / `approve_root` | 越权时发 `sandbox_request` 事件等用户点头；批准粒度是**目录**并写进配置 |
| 内置编辑器 | `src/inknote/**` | 移植自 InkNote 的 CodeMirror 6 编辑器。宿主是 `src/components/NoteEditor.tsx`，通过 `setEditorDocumentContext` 把「当前主题」注入适配层 `inknote/lib/tauri.ts` |
| 长期记忆 | `domain/memory.rs` + `commands/memory.rs` + `agent/tools/memory.rs` + `components/Memory.tsx` | 见第 6 节。JSONL 存储、两级作用域、按分类注入提示词；工具 `memory_write` / `memory_list` / `memory_forget` |
| 会话元数据 | `agent/chats.rs` + `commands/agent.rs` 的 `chat_*` | 见第 7 节。侧车文件记名字/置顶/归档/分叉血缘；侧栏「⋯」菜单：`chat_rename` / `chat_pin` / `chat_fork` / `chat_archive` / `chat_delete` |

### 几个关键取舍

- **技能的 frontmatter 不做完整 YAML 解析**：只认 `key: value` 两行。理由是可预测——
  技能文件是用户手写的，解析失败要让用户看到「没写 description」而不是静默出错。
- **MCP 手写而不引 SDK**：只用四个方法，300 行能讲清楚；出问题时日志就在自己手里。
- **外部 MCP 工具一律按「写入」级处理**：它们的能力不可知，宁可多问一次。
- **主观题判分复用 `agent::complete_once`**：不起对话循环、不带工具，只做一次结构化输出，
  并要求返回 JSON（`extract_json` 会容忍 ``` 围栏与前后解释）。
- **沙箱只管文件**：联网由 `agent.allow_web` 单独控制；两者互不影响。
- **记忆全量注入而不做检索**：个人规模（几十条）下，全量注入比向量检索更不容易漏，
  也少一个向量库要维护。上限（40 条 / 1600 字）保证它不会反噬上下文预算。
- **会话元数据放侧车文件**（`<id>.meta.json`），不插进 `chats/<id>.jsonl`：
  jsonl 每行都必须是 `ChatMessage`，插特殊行会让每一处读对话的地方都要学会跳过它。
  代价是「搬对话」要记得搬两个文件（`chat_move` 已经这么做了）。

### 移植 InkNote 时的改造点

| 原实现 | 改造后 |
| --- | --- |
| `lib/tauri.ts` 直连 InkNote 的 30 个命令 | 只保留 readFile / writeBinary / removePath，转调学习中枢的 `file_*` 命令 |
| 绝对路径贯穿全局 | `setEditorDocumentContext({slug, dir})` + 绝对↔相对映射（文件仍只在主题目录内） |
| `load_app_settings` 持久化 | 换成 localStorage（界面偏好不值得占后端配置） |
| unified/remark/rehype 导出管线 | 复用学习中枢已有的 marked + KaTeX + DOMPurify（`inknote/render/export.ts` 只有 10 行） |
| `.modal` / `.toast` 类名 | 加 `.inknote-scope` 前缀限定作用域，避免与学习中枢的同名样式互相覆盖 |
