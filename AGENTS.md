# AGENTS.md — 给在这个仓库里干活的 AI agent

你正在改「学习中枢」：一个 Tauri 2 + Rust + React 的个人学习助手。
先读 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)，那里有分层、事件协议、扩展点和取舍说明。

## 常用命令

```bash
npm install                                            # 前端依赖（走 .npmrc 里的本地代理）
npm run app:dev                                        # 开发模式（首次编译 Rust 约 3-8 分钟）
npm run typecheck                                      # 前端类型检查（必须过）
npm run build                                          # 前端构建
npm run rust:check                                     # 后端类型检查
npm run rust:test                                      # 后端单测（19 个）
npm run dist                                           # 打包（tauri build + scripts/package.mjs）
npm run shortcut                                       # 把打包版同步到 release/app 并在桌面建快捷方式（应用开着时会跳过同步并提示）
npm run demo:seed && npm run demo:pdf                  # 造演示数据（workspace/）
npm run dev:fake-llm                                   # 本地假模型（无需 API Key 验证对话链路）
npm run test:e2e                                       # 端到端冒烟（需要应用带调试端口启动）
```

跑完改动后至少执行：`npm run typecheck` + `npm run rust:check`。改到 `domain/` 或 `viewer/` 时必须跑 `npm run rust:test`。

### 验证 UI 的正确姿势

不要靠截图目测像素。用 CDP 直接量 DOM、点按钮：

```bash
# 带调试端口启动
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
npm run app:dev

node scripts/cdp.mjs metrics                      # 量布局（各区域宽高、是否居中、有没有溢出）
node scripts/cdp.mjs eval "document.title"        # 任意 JS 求值
node scripts/cdp.mjs click ".wb-tab:nth-child(4)" # 真实点击
node scripts/cdp.mjs screenshot .screenshots/x.png
npm run test:e2e                                  # 完整对话链路冒烟（15 项断言）
```

注意 `window.innerWidth` 受系统缩放影响（2560 屏 @150% → 1707 CSS px），
判断「有没有水平溢出」要用 `document.documentElement.scrollWidth` 与视口宽度比。

## 代码约定

### Rust

- 分层严格：`commands` → `agent`/`viewer`/`domain` → `store`/`paths`/`error`。
  **`domain` 不许依赖 Tauri**（要能脱离应用单测）。
- 错误统一 `AppError`，命令返回 `AppResult<T>`。用户可见的文案用中文，写清「哪儿错了、怎么办」。
- 文件访问必须走 `paths::resolve_in_root`（防止 `../` 越界）与 `store::atomic_write`（防止半截文件）。
- 注释解释**为什么**，不复述代码。每个模块头部写一段模块级说明（做什么、为什么这么设计）。
- 新增能力优先"写一个 Tool"而不是改 agent 主循环。

### TypeScript / React

- 类型以 `src/lib/types.ts` 为唯一来源，字段名必须与 Rust serde 输出一致。
  **坑**：带 tag 的枚举，`rename_all` 只改变体名，变体内的字段仍是 snake_case（如 `turn_id`）；
  普通结构体是 camelCase（如 `createdAt`）。改字段名要同时改两端。
- 所有 IPC 调用都经过 `src/lib/api.ts`，组件里不直接 `invoke`。
- 全局状态只在 `src/store/app.ts`；组件内部状态用 `useState`。
- 样式统一写在 `src/styles/app.css`，用 CSS 变量（`--bg` `--text` `--accent` …），
  不引入 CSS-in-JS / Tailwind。
- 图标用 `components/ui.tsx` 里的 `<Icon name="..." />`，不引入图标库。
- 用户可见文案用中文，语气克制（参考现有文案：陈述事实，不堆感叹号）。

## 别踩的坑

下面几条都是真踩过的（修之前分别表现为：回复内容整齐翻倍、PDF 溢出面板、评完分卡住不动）：

- **StrictMode 会让 effect 跑两遍**。凡是「注册监听 / 订阅事件」的初始化都必须幂等
  （见 `store/app.ts` 的 `initPromise`）。不幂等的症状是每条消息、每个工具卡片都渲染两次。
- **别在组件里用 `useRef` 拿「可能还没渲染出来的节点」测尺寸**。`PdfView` 最初把
  ResizeObserver 挂在 `.viewer-scroll` 上，但文档没加载完时那个节点还不存在，
  结果宽度永远是 0、画布按 100% 溢出。现在改成固定 2 倍栅格 + CSS 控制显示尺寸，不再测量。
- **复习队列要显式维护**。`cards[]` 是「列表」，不是「队列」；评完分重新排序会让同一张卡反复出现。
  进复习时把队列快照下来，评一张 `slice(1)`。
- **`State<'_, AppState>` 别跨 await 用**：命令开头先 `let core = state.0.clone();`。
- **别在持锁时 await**：`parking_lot::RwLock` 的 guard 不能跨 await，配置读取用 `config_read()` 拿克隆。
- **改 `AgentEvent` / 命令签名要同步改前端**：`lib/types.ts` + `store/app.ts` + `lib/api.ts`。
- **Windows 路径**：目录名可能含中文（本项目根目录就是 `E:\学习中枢`），不要假设 ASCII。
- **第一次 `tauri dev` 很慢**：Rust 首次编译要几分钟，别以为卡住了。
- **图标变了要重跑**：`npm run icon`（缺 `src-tauri/icons/icon.ico` 会让 build script 直接失败）。
- **改完 Rust 不用手动重启**：`tauri dev` 会监听 `src-tauri` 自动重编译并重启应用。
- **`topic_get` 必须带笔记清单**：前端在 agent 写文件后会重新拉它来刷新界面；
  若返回空的 notes，界面会被刷空（踩过一次）。
- **Mermaid 的渲染必须收口到一处**：界面上有两个入口会画图（聊天/内置浏览器的 Markdown 渲染器、
  编辑器里的图表组件）。各自调 `mermaid.render()` 会**撞 id**（两边都从 1 开始编号），
  而且 render 本身不是并发安全的。症状很迷惑人：合法的图报「Syntax error in text」，
  并且 Mermaid 会把那个错误框塞进 DOM、一直留在界面上（曾经被截进 README 的图里）。
  现在统一走 `src/inknote/lib/mermaid.ts` 的 `renderDiagram()`：唯一 id + 串行队列 +
  `suppressErrorRendering`。**别再绕过它直接调 mermaid。**
- **widget 里不要在 await 之前判断 `target.isConnected`**：widget 刚创建时节点可能还没挂到文档上，
  检查会直接 return，图就永远不渲染。要么先 await 一次（加载 mermaid 天然会让出微任务），
  要么把这个判断放到渲染之后。
- **README 的截图脚本带守卫**（`scripts/shots.mjs`）：拍照前会检查界面上有没有渲染失败的残留
  （Mermaid 错误框、`pre[data-mermaid=failed]`），有就直接报错。文档里的图必须来自干净状态。
- **主题走 `html[data-theme]`，不是媒体查询**：用户要能强制明亮。
  新写样式如果要区分深浅色，用 `html[data-theme="dark"]` 选择器，别用 `@media (prefers-color-scheme)`。
- **vite 不能监视 `workspace/`**：Windows 上被监视的目录会一直被握着句柄，开发模式下
  「删除主题」「改主题目录名」这类 `rename` 会直接失败（`EPERM 拒绝访问`），
  但打包版没这个问题——排查时很容易怀疑到应用自己头上。`vite.config.ts` 里已经
  `watch.ignored: ["**/src-tauri/**", "**/workspace/**"]`，别删。同理，写脚本自己做
  rename/删除验证时要意识到：**只要 vite 在跑，任何进程**改工作区里的目录都会失败。
- **自动化点击发送要认 `.send-btn`**：`composer-bar` 里「思考强度」按钮的 title 里也有
  「发送」二字（「…可在设置里改发送方式」），按 `title.includes("发送")` 找会点到它，
  表现是「点了发送但什么都没发生」——`test:e2e` 已经改成 `document.querySelector('.send-btn')`。
- **父子主题的坑**：子主题是工作区根下的**平级目录**，不是嵌套目录（见 ARCHITECTURE「主题与章节」）。
  动 `list()` / `slug` 之前先想清楚：所有按 slug 的路径假设都建立在「一层」之上。
- **引用里的「第 X 章」要容错**：模型常写 `【来源：materials/x.pdf 第三章】`，我们只教过「第 N 页」。
  解析路径时统一走 `paths::strip_locator_if_missing`（**原路径存在就不动**，不存在才去掉尾巴上的
  定位词），三个入口——内置浏览器命令、agent 的 `viewer_open`、paths 单测——都从这里走。
  别只在某一处加正则去尾巴：文件名真的叫「第一讲.pdf」时会被误伤。
- **内置浏览器的头部只有一行**：标签 + 页码 + 已读字数 + 重载/外部打开都在 40px 那一行里
  （完整路径放标签的 tooltip）。原来下面还有一条路径栏，白占一行还把正文挤掉一截，被用户点名拿掉，别再往回加。
- **窗口是自绘标题栏**（`tauri.conf.json` 里 `decorations: false`，见 `components/TitleBar.tsx`）：
  系统标题栏属于 Windows 画的非客户区，配 WebView2 时它会压在网页内容上面
  （用户报的「一条黑框一直在挡界面，上面是最小化/窗口化/关闭」）。自绘后窗口里只有 DOM。
  注意两点：**调 window API 需要 capabilities 里显式授权**
  （`core:window:allow-minimize`、`allow-toggle-maximize`、`allow-close`、`allow-is-maximized` 等；
  漏了会在运行时报权限错误，typecheck 查不出来）；拖拽靠 `data-tauri-drag-region`
  （需要 `core:window:allow-start-dragging`），按钮上别加这个属性，否则点不灵。
- **侧栏拖拽用自己的指针事件实现**（`Sidebar.tsx` 的 `ChatRow`）：Tauri 在 Windows 上
  `dragDropEnabled: true` 会接管文件拖放（资料导入要用），HTML5 的 `dragstart`/`drop`
  **根本不会触发**——想加「拖动某个元素到另一处」的交互时别再试 HTML5 拖放。
  现在的做法：pointerdown 记住起点 → 移动超过 5px 才算拖拽（否则仍是点击）→
  跟随一个 body 上的幽灵小卡片 → `elementFromPoint` 找落点（主题行带 `data-slug`）→
  pointerup 调命令。`setPointerCapture` 要包 try/catch，合成事件下会抛。
- **对话可以在主题之间搬**（`chat_move` 只动 `.hub/chats/<id>.jsonl`）：
  搬的是文件，聊天记录里已有的相对引用不会跟着重算——所以这是「整理」语义。
  正在看的那条被搬走后，前端会给源主题新开一条，免得接着聊又写回旧主题。
- **网页「拒绝连接」不是网络问题**：很多站点（GitHub / 知乎 / Bing…）发
  `X-Frame-Options: DENY|SAMEORIGIN` 或 `CSP frame-ancestors` 拒绝被 iframe 嵌入，
  Chromium 就把内嵌窗口画成一张「拒绝了我们的连接请求」的错误页。
  内置浏览器现在会先调 `web_frame_check` 读响应头，不能嵌就**自动切阅读模式**
  （正文来自服务端提取，agent 读的也是它），并说明原因、给「用系统浏览器打开」的按钮。
  别把这个错误当成断网去查代理。
- **技能 / MCP 一律「只给开着的」**：判断只看开关（全局 + 本主题 + 父主题的禁用并集，
  `skills::effective` 是唯一的规则入口，面板与系统提示词都走它，别在别处再写一套过滤）。
  **不要按名字或目录写特例**——用户会自己往任意目录加技能，规则必须对以后新增的也成立。
  技能多了靠面板上的「全部关掉 / 全部打开」一键归零，那两个命令（`skills_set_all` /
  `mcp_set_all`）也是按「当前扫到什么就管什么」实现的。
- **InkNote 的样式表是全局的**（`src/inknote/editor.css` 在 `main.tsx` 里引入）：
  里面那些通用类名会直接盖住应用自己的同名样式，CSS 只看优先级和顺序。
  真踩过：右下角的操作提示（`.toast`）被它改成了 `fixed + left:50%` 的居中横幅，
  而且 `bottom` 用了未定义的 `--statusbar-height` 导致 calc 失效，提示有一半跑到窗口外面。
  往组件里加通用类名前先跑 `node scripts/style-collisions.mjs`（退出码非 0 就是撞了）。
- **复习是内置的，外部 Anki 已整体移除**：卡片调度（SM-2）、复习界面都在本应用里。
  曾经的 AnkiConnect 同步与 TSV 导出已经删干净——`anki.rs` 模块、`anki_status` / `anki_sync` /
  `anki_ping` / `card_export_anki` 四个命令、`AnkiConfig`、`Card::anki_note_id` 与 `anki_fields`、
  设置页那一节都不在了。**别再往回加**：用户明确不要外部依赖。
  复习按钮上的「10 分钟 / 4 天」来自后端 `preview_secs`（同一个 `apply` 跑在副本上），
  别在前端另写一套间隔推算。

## 当前状态（v1）

已完成：主题/笔记/资料/卡片/计划/会话/测验 七个模块、内置浏览器（PDF 含文字层 / Markdown / 网页 / 图片 / 文本）、
内置笔记编辑器（移植自 InkNote 的 CodeMirror 6 所见即所得）、讲解模式（讲解方案 + HTML 演示页 + 来源标注）、
知识库（讲义入库 + 带出处的检索）、技能与 MCP（侧栏入口，支持全局/本主题两级开关，沿父子链继承）、
思维导图（Mermaid）、拖放导入资料、主题模式（跟随系统/明亮/深色）、
工作区沙箱与越权申请、权限分级、两类模型协议（OpenAI 兼容 + Anthropic）、日程视图、设置页、
父子主题（「只学一门课里的一章」：资料继承、笔记/卡片/计划独立）。

已知待办（按价值排序）：

1. 语音/图片输入（多模态消息）
2. 复习提醒（系统通知 + 到点弹窗）
3. 周报/月报生成（`sessions/` 的数据已经够用）
4. SPA 网页的正文提取（现在依赖服务端渲染的 HTML）
5. 父主题的进度汇总（子主题的待复习数、任务合并到父主题视图）
