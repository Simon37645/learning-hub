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
│   └── trash/                所有删除操作的回收站
└── <主题>/
    ├── topic.json            TopicMeta
    ├── README.md             agent 每次都会读的背景资料
    ├── notes/*.md            可选 front matter（title / tags）
    ├── materials/**          任意文件
    ├── cards/cards.jsonl     Card（含 SRS 状态内联）
    ├── plan/tasks.jsonl      PlanTask
    ├── sessions/<id>.json    StudySession
    └── .hub/chats/<id>.jsonl 该主题的对话
```

- 写文件一律走 `store::atomic_write`（临时文件 + rename），不会出现半截文件。
- JSONL 单行损坏只跳过该行并打日志，不毁掉整个文件。
- 主题列表靠扫目录得到；`topic.json` 缺失时会**自动认领**该目录并补写元数据，
  所以用户手动扔进来的文件夹也能直接当主题用。
- 路径安全：agent 给的一切路径都经 `paths::resolve_in_root` 归一化并校验前缀，
  `../`、绝对路径、越界软链一律拒绝。

## 6. 扩展点

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

## 7. 安全与隐私

- **密钥不出后端**：`PublicConfig` 只给 `hasApiKey` 与 `sk-12…cd` 形式的提示；前端拿不到明文。
- **文件访问有边界**：agent 与查看器都只能碰当前主题目录内的路径。
- **删除可撤销**：`fs_delete` 只删空目录，界面上的删除一律进 `.hub/trash/`。
- **网页内容一律净化**：模型/网页产出的 Markdown 经 DOMPurify（html+svg+mathml 白名单）后才进 DOM；
  本地 HTML 用 `sandbox=""` 的 iframe 渲染，不允许脚本执行。
- **CSP** 在 `tauri.conf.json` 里收敛：脚本只允许 `self`，iframe 允许 http(s)，字体只允许 `self`/`data:`。
- 没有任何遥测、账号、云同步；所有数据只在本机。

## 8. 已知取舍

| 决定 | 原因 | 代价 |
| --- | --- | --- |
| 不用数据库，直接读文件 | 数据可带走、可手改、耐迁移 | 主题多时列表会慢（几百个以内无感） |
| PDF 自绘 | 要让 agent 能操作文档 | 暂不支持选中文字（待补文本层） |
| 工具串行 | 学习类工具存在依赖关系 | 批量操作略慢 |
| 不发工具结果事件 | 减少事件噪声 | 前端要同时处理「实时」与「历史」两条渲染路径 |
| 删除走回收站 | 学习资料误删代价高 | 需要用户自己清理 `.hub/trash/` |

## 9. 第二轮新增的模块

| 模块 | 位置 | 说明 |
| --- | --- | --- |
| 知识库 | `kb.rs` | 把 `kb/`+`materials/`+`notes/` 抽成带来源的文本块，索引缓存在 `.hub/kb.json`（按 mtime 增量刷新）。工具：`kb_build` / `kb_search` |
| 讲解方案 | `commands/lesson.rs` + `agent/tools/lesson.rs` | 讲解模式的骨架：方案落成 `lessons/<id>.json`，进度注入提示词，跨轮次不迷路 |
| 测验 | `domain/quiz.rs` + `commands/quiz.rs` + `agent/tools/quiz.rs` | 六种题型；客观题本地判分，主观题把采分点交给模型逐条对照（`quiz_grade_subjective`） |
| Anki | `anki.rs` | AnkiConnect 客户端（`version` / `deckNames` / `createDeck` / `addNotes`）。卡片带 `kind`（basic/reversed/cloze），完形卡会校验 `{{c1::}}` 标记 |
| 技能 | `skills.rs` | 扫 `SKILL.md`（frontmatter 只解析 name/description，不引 YAML）。渐进式披露：提示词里只给名字+适用场景 |
| MCP | `mcp.rs` + `agent/tools/mcp.rs` | 手写的 stdio JSON-RPC 客户端（换行分帧）。握手 → `tools/list` → 包装成 `mcp__<服务器>__<工具>` |
| 沙箱 | `state.rs` 的 `is_inside_workspace` / `needs_escalation` / `approve_root` | 越权时发 `sandbox_request` 事件等用户点头；批准粒度是**目录**并写进配置 |
| 内置编辑器 | `src/inknote/**` | 移植自 InkNote 的 CodeMirror 6 编辑器。宿主是 `src/components/NoteEditor.tsx`，通过 `setEditorDocumentContext` 把「当前主题」注入适配层 `inknote/lib/tauri.ts` |

### 几个关键取舍

- **技能的 frontmatter 不做完整 YAML 解析**：只认 `key: value` 两行。理由是可预测——
  技能文件是用户手写的，解析失败要让用户看到「没写 description」而不是静默出错。
- **MCP 手写而不引 SDK**：只用四个方法，300 行能讲清楚；出问题时日志就在自己手里。
- **外部 MCP 工具一律按「写入」级处理**：它们的能力不可知，宁可多问一次。
- **主观题判分复用 `agent::complete_once`**：不起对话循环、不带工具，只做一次结构化输出，
  并要求返回 JSON（`extract_json` 会容忍 ``` 围栏与前后解释）。
- **沙箱只管文件**：联网由 `agent.allow_web` 单独控制；两者互不影响。

### 移植 InkNote 时的改造点

| 原实现 | 改造后 |
| --- | --- |
| `lib/tauri.ts` 直连 InkNote 的 30 个命令 | 只保留 readFile / writeBinary / removePath，转调学习中枢的 `file_*` 命令 |
| 绝对路径贯穿全局 | `setEditorDocumentContext({slug, dir})` + 绝对↔相对映射（文件仍只在主题目录内） |
| `load_app_settings` 持久化 | 换成 localStorage（界面偏好不值得占后端配置） |
| unified/remark/rehype 导出管线 | 复用学习中枢已有的 marked + KaTeX + DOMPurify（`inknote/render/export.ts` 只有 10 行） |
| `.modal` / `.toast` 类名 | 加 `.inknote-scope` 前缀限定作用域，避免与学习中枢的同名样式互相覆盖 |
