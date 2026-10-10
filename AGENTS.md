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
npm run rust:test                                      # 后端单测（79 个）
npm run test:ui                                        # 前端单测（vitest：InkNote 回归网 + 应用自己的纯逻辑）
npm run dist                                           # 打包（tauri build + scripts/package.mjs）
npm run shortcut                                       # 把打包版同步到 release/app 并在桌面建快捷方式（应用开着时会跳过同步并提示）
npm run demo:seed && npm run demo:pdf                  # 造演示数据（workspace/）
npm run dev:fake-llm                                   # 本地假模型（无需 API Key 验证对话链路）
npm run test:e2e                                       # 端到端冒烟（需要应用带调试端口启动）
```

跑完改动后至少执行：`npm run typecheck` + `npm run rust:check`。改到 `domain/` 或 `viewer/` 时必须跑 `npm run rust:test`；
改了前端的纯逻辑（`src/lib/`、hook、统计口径）把 `npm run test:ui` 也跑上（测试放在被改文件旁边，`*.test.ts(x)`）。

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
- **远端网页走原生子 WebView，不再吃 X-Frame-Options**：`WebTab` 默认用挂在主窗口上的
  child webview（`viewer::webview` 的 `WebviewManager`，label = `web-<tab_id>`，需要 tauri 的
  **unstable** feature）渲染网页——它是顶层浏览上下文，GitHub / 知乎 / Bing 那些发
  `X-Frame-Options` / `CSP frame-ancestors` 的站点照常显示。iframe 只剩两个角色：
  本地 HTML（`sandbox=""`）和**原生视图创建失败时的兜底**——只有兜底路径才跑
  `web_frame_check` 自动切阅读模式，阅读模式本身是 TabBar 上的手动按钮，别把自动切换
  加回原生路径。页面状态（标题 / url / loading）由 `on_navigation` / `on_document_title_changed` /
  `on_page_load` 回调经 `ViewerEvent::Updated` 推回前端；url 变了会**清空正文快照**，
  agent 下一次 `viewer_read` 自动重新提取。弹窗一律 `Deny` 并用 opener 交给系统浏览器。
- **原生子 WebView 浮在一切 DOM 之上**：它是独立的原生窗口，Modal / 图片大图 / 命令面板 /
  拖放提示 / 演示卡片全屏（Fullscreen API）都会被它盖住。所以这些全屏浮层都要挂
  `useOverlay()`（`store` 里的 `overlayCount`），`WebTab` 计数非零 / 阅读模式 / zen 时调
  `viewer_webview_set_visible(false)` 让路；右下角 Toast 压不过它，`nativeWebviewUp` 为真时
  ToastHost 换到左下。bounds 同步是 **CSS px → 物理 px**：前端量 `getBoundingClientRect`
  原样传（相对视口），Rust 按 `scale_factor` 换算取整（`to_physical_rect`，有单测）；
  宿主尺寸/位置变了要 `viewer_webview_bounds`（前端用 rAF 合并）。w/h 不足 2 视为不可见，
  只 `hide()` 不设 bounds。安全边界：`web-<tab_id>` 不在 capabilities 的 windows 列表里，
  外部网页拿不到任何 IPC 权限；`on_navigation` 只放行 http/https（含挡 `http://ipc.localhost`）。
- **内置浏览器的头部只有一行**：标签 + 页码 + 已读字数 + 重载/外部打开都在 40px 那一行里
  （完整路径放标签的 tooltip）。原来下面还有一条路径栏，白占一行还把正文挤掉一截，被用户点名拿掉，别再往回加。
- **窗口是自绘标题栏**（`tauri.conf.json` 里 `decorations: false`，见 `components/TitleBar.tsx`）：
  系统标题栏属于 Windows 画的非客户区，配 WebView2 时它会压在网页内容上面
  （用户报的「一条黑框一直在挡界面，上面是最小化/窗口化/关闭」）。自绘后窗口里只有 DOM。
  注意两点：**调 window API 需要 capabilities 里显式授权**
  （`core:window:allow-minimize`、`allow-toggle-maximize`、`allow-close`、`allow-is-maximized` 等；
  漏了会在运行时报权限错误，typecheck 查不出来）；拖拽靠 `data-tauri-drag-region`
  （需要 `core:window:allow-start-dragging`），按钮上别加这个属性，否则点不灵。
- **页面被滚走 = 自绘标题栏整个消失**（用户报的「上面的三个图标和 logo 都不见了」）：
  `body { overflow: hidden }` 会传给视口，视口**滚轮滚不动，但 `scrollIntoView` / `focus()`
  仍然滚得动**。壳（`.app-shell`）里只要有个元素越出下沿（绝对定位的浮层最容易；`.app-main`
  没有 overflow，它的绝对定位子元素能直接顶到页面外），一次 `scrollIntoView`（`Viewer.tsx`
  的引用跳转、`PdfView.tsx` 的翻页都会调）就把**页面**滚了上去，32px 的标题栏滑出可视区：
  logo 与最小化/最大化/关闭三个按钮全没了，而且滚不回来，只能重启应用。
  所以 `.app-shell` 上必须留着 `overflow: clip`——**别改成 `hidden`**（hidden 会让壳自己变成
  可滚动容器，同一个坑往上挪一层），`.titlebar` 上再留一道 `position: sticky; top: 0` 兜底。
  排查这类问题时**别信 `documentElement.scrollHeight`**（它会被 `overflow: hidden` 骗成视口高）：
  用 `window.scrollTo(0, 99999)` 再看读回来的 `scrollY`，那才是页面真正的可滚动量。
- **侧栏拖拽用自己的指针事件实现**（`Sidebar.tsx` 的 `ChatRow`）：Tauri 在 Windows 上
  `dragDropEnabled: true` 会接管文件拖放（资料导入要用），HTML5 的 `dragstart`/`drop`
  **根本不会触发**——想加「拖动某个元素到另一处」的交互时别再试 HTML5 拖放。
  现在的做法：pointerdown 记住起点 → 移动超过 5px 才算拖拽（否则仍是点击）→
  跟随一个 body 上的幽灵小卡片 → `elementFromPoint` 找落点（主题行带 `data-slug`）→
  pointerup 调命令。`setPointerCapture` 要包 try/catch，合成事件下会抛。
- **对话可以在主题之间搬**（`chat_move` 动 `.hub/chats/<id>.jsonl` **和**同名的 `.meta.json`）：
  搬的是文件，聊天记录里已有的相对引用不会跟着重算——所以这是「整理」语义。
  正在看的那条被搬走后，前端会给源主题新开一条，免得接着聊又写回旧主题。
- **技能 / MCP 一律「只给开着的」**：判断只看开关（全局 + 本主题 + 父主题的禁用并集，
  `skills::effective` 是唯一的规则入口，面板与系统提示词都走它，别在别处再写一套过滤）。
  **不要按名字或目录写特例**——用户会自己往任意目录加技能，规则必须对以后新增的也成立。
  技能多了靠面板上的「全部关掉 / 全部打开」一键归零，那两个命令（`skills_set_all` /
  `mcp_set_all`）也是按「当前扫到什么就管什么」实现的。
- **`~/.agents/servers/` 导入本机 MCP 配置**（面板「从本机导入」）：**工具前缀用文件里的 `id`，
  不是 label**——label 可能带空格，`mcp__<名>__<工具>` 的函数调用名会废。导入即写 config 并
  `mcp_reload`（已导入过的保留现有 enabled，别把用户手动关掉的打开）；只支持 stdio
  （transport/http、带 url 的只标 `blocked_reason`，不算解析失败）；`agents_mcp_import` 的
  **id 必须按文件名校验**（含 `/` `\` `..` 一律拒绝），否则拼 `{id}.json` 就是路径穿越。
  导入对目录里所有文件一视同仁，别给任何具体名字写特例。
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
- **```html 演示卡片里的脚本不会执行**（`markdown.ts` 的 `renderDemoBlocksIn`）：卡片是
  `sandbox=""` 的 iframe——模型产出的 HTML 只被当成画面，碰不到应用的 DOM 与 IPC。
  所以动画只能用 CSS（animation / transition）或 SVG 的 `<animate>`，交互只能用
  `:hover` / `:checked` / `<details>` 这类纯 CSS 手段；提示词里已经这么要求模型了。
  **别为了「让演示能动起来」把 sandbox 改成 `allow-scripts`**：那等于让模型写的脚本进到应用里跑
  （srcdoc 还继承应用的 CSP，改成 allow-scripts 也大概率只是白忙）。卡片上会标「脚本已禁用」。
- **`pdf_screenshot` 是请求-响应，前端必须回话**：工具在 Rust 侧登记等待位 → 发
  `render_request` 事件 → 前端 `lib/pdfshot.ts` 用 pdf.js 渲染 → 调 `viewer_report_render`。
  成功要回，**失败也要回**（带 `error`），否则工具一直等到 25s 超时才报错，用户盯着一个不动的卡片。
  渲染模块自己读字节、自己开文档，**不依赖 `PdfView` 是否挂载**（内置浏览器收起来时也能用）；
  它按 tab 缓存文档，`PdfView` 重新加载时会 `forgetPdfDoc`，别把那句删了（否则截到旧版本的页）。
- **工具结果带图的编码两边不一样**：Anthropic 允许 `tool_result.content` 是内容块数组（图放里面），
  **OpenAI 的 `tool` 消息只吃字符串**，所以 `to_openai_messages` 会在带图的工具结果后面
  补一条 `user` 消息专门放图。改协议编码时别把这条丢了，也别在 `tool` 角色里塞数组。
- **`setSize` 设的是「内容区」尺寸，显示器/工作区给的是「外框」尺寸**——差那圈隐形边框
  （无边框可缩放窗口照样有，实测 **22×13 物理像素**）。拿外框尺寸直接 `setSize`，窗口就永远
  「大出去一点点」：多显示器时右边那 22px 正好落到旁边那块屏上（用户报的「突出一部分到
  另一块屏幕」）。更坑的是「贴合工作区」的兜底逻辑用同样的口径去量、去设，是个不动点，
  永远修不回来。所以**所有「贴满某块屏 / 某个工作区」都走 `lib/winfit.ts` 的 `fitOuter`**
  （先量一次边框，减掉再设；全屏时内外相等，减 0）。位置那边不用管：`setPosition` 与
  `outerPosition` 都是外框口径。
- **F11 专注模式的窗口几何要自己指定显示器**（`store/app.ts` 的 `applyZenWindow`）：
  直接 `setFullscreen(true)` 时由系统挑屏，**多显示器下会摆到别的那块屏上**。现在按
  `currentMonitor()` 用 `setFullscreenOnMonitor`（Tauri 2.12 起有，capabilities 要
  `core:window:allow-set-fullscreen-on-monitor`）指定目标屏，再 `fitOuter` 对齐一次。
  另一个连带坑：**TitleBar 的「贴合工作区」在全屏期间必须让路**（`clampToWorkArea` 开头有守卫），
  否则它会在系统刚铺好全屏之后把窗口缩成工作区大小（任务栏露出来）。
  样式那边 `.app-shell.zen` 收起的范围：标题栏 / 侧栏 / 内置浏览器 / 顶部分段 /
  工作台功能区（`.wb-head`、`.wb-tabs`）与笔记自己的头部（`.note-head`）——
  **只留编辑区和「路径 / 未保存 / 保存」那一行**：全屏写作看不见保存状态才是真的糟。

- **长期记忆的作用域由文件位置决定，不是记录里的字段**
  （`<工作区>/.hub/memory/memories.jsonl` = 全局，`<主题>/.hub/memory/memories.jsonl` = 本主题）。
  同一份文件搬到别处语义就变了，所以别在 `Memory` 里再加一个 `scope` 字段——
  那迟早会和文件位置不一致。注入顺序靠 `AppCore::memory_scopes`（本主题 → 父主题 → 全局），
  面板、工具、提示词都从这里拿，别各写一套。
- **记忆的写入走 `memory_write` 工具，但注入不走工具**：每轮由 `AppCore::memory_digest`
  直接塞进系统提示词（模型不会忘），并顺手记 `last_used` / `use_count`。
  预览系统提示词要用 `memory_peek`（只渲染不记账），否则连点几次预览就把计数刷满了。
- **`memory_digest` 全程持 `parking_lot` 写锁**：别在它里面 await，也别在持有
  `state.0.memory()` guard 时 await。攒着的「用了几次」由 `memory_flush` 落盘
  （`topic_open` 与关窗口时各调一次）。
- **记忆的改动统一发 `hub://memory`**（`core.emit_memory_changed`）：前端把它折成
  `memoryTick` 计数，面板与侧栏角标据此重拉。新增写记忆的入口时别忘了发这个事件，
  否则面板会显示过期数据。
- **用量（token / 缓存命中）只认服务商报的数字**：`ProviderUsage` 由两个协议各自解析
  （OpenAI 兼容在 `usage`，Anthropic 在 `message_start` + `message_delta`，两处口径不同：
  Anthropic 的 `input_tokens` **不含**缓存命中的部分，要加回去）。两个坑：
  带 `usage` 的那一块 SSE `choices` 是**空数组**，解析必须早于 choices 判断，否则永远拿不到；
  官方 OpenAI 端点要请求体里带 `stream_options.include_usage` 才会报用量，但有的网关不认这个字段，
  所以失败要能退回不带它的请求（`chat()` 里就是这么做的）。前端只统计报过用量的轮次，
  轮次为 0 时整块不显示——**别把「没数据」算成「没命中」**。
- **`materials/` 里有什么就显示什么**：类型不做白名单（PDF / Markdown / txt / 图片 / 代码都能读、
  都能在内置浏览器预览），资料面板与系统提示词的清单都得走 `store::walk_files`，
  **两边的深度要一致**（都 4 层），否则放进子目录的讲义模型看不见、会答「你的资料里没有」。
  另外提醒：用户可能在**资源管理器里**直接丢文件进来，前端收不到通知——
  `App.tsx` 里挂了个 window focus 监听调 `store.refreshTopicFiles()` 轻量重拉，
  别改成 `openTopic`（那会清空 streaming 并把对话切回最近一条）。
- **对话的元数据是侧车文件**（`agent/chats.rs` 的 `<id>.meta.json`：名字 / 置顶 / 归档 / 分叉血缘），
  **不要往 `chats/<id>.jsonl` 里插特殊行**——那个文件每行都必须是 `ChatMessage`，
  模型、界面、导出都直接读它。代价是「搬对话」要搬两个文件：`chat_move` 已经这么做了，
  以后加类似操作记得跟上（`chats::meta_path` + `chats::remove`）。
- **对话列表的排序只在 Rust 里算一次**（`chats::sort_items`：置顶 → 最近使用 → 归档沉底），
  前端照返回顺序渲染。两边各排一次迟早出现「显示置顶了、点进去排在后面」。
  条数上限（60）放在排序**之后**，否则置顶那条会被文件时间截掉。
- **图片消息只存路径，不存字节**：用户贴的图落在 `<工作区>/.hub/attachments/`，
  jsonl 里的块是 `{"type":"image","path":".hub/attachments/x.png",…}`。
  别图省事把 base64 写进 jsonl（那个文件会被整份读进上下文预算、被全文检索扫、
  被用户用记事本打开）。另注意**带 tag 的枚举里字段是 snake_case**：
  前端类型里写 `media_type`，写成 `mediaType` 会永远取到 undefined。
  发请求时才读盘编码，读不到/超上限/档案关了图片输入 → 降级成一行文字，别让请求失败。
- **拖文件进窗口不会触发 HTML5 的 drop**（Tauri 接管了文件拖放，见下一条的姐妹坑）：
  拖进来 = 导入到 `materials/`（没有主题就提示）。所以「贴图给 agent 看」这条路
  走的是 **Ctrl+V 粘贴**和输入框里的「图片」按钮，别再花时间试拖拽。
- **工具的默认 `scope()` 是 `Study`**：想在工坊模式下也出现的工具必须显式实现
  `fn scope() -> ToolScope::Both`。反过来，工坊专用的工具写 `Studio`。
  模式过滤在 `registry::ToolRegistry::specs_for` 一处做，主循环只管拿。
- **工坊的对话身份在侧车文件里**（`ChatMeta.mode`）：工坊没有主题，它的对话和首页的
  日常问答都在 `<工作区>/.hub/chats/` 下，靠这一个字段分成两份清单。
  加新的「按模式分」的界面时照 `chat_overview(topic_slug, mode)` 的写法传 mode，
  别去改 `chats_dir_for` 的路径规则。
- **工坊模式下相对路径的根是练习目录**（`ToolCtx.root`），不是主题目录。
  工具里别自己拼 `ctx.topic_or(...)?.dir`——用 `ctx.resolve_path` / `ctx.root_for`，
  否则那个工具一到工坊就报「没有打开主题」。
- **加/删 CSS 变量要同步 `theme.rs` 的 `VAR_DOCS`**：底下有个单测直接解析 `app.css`
  对齐两边（文档漂了比没有文档更糟）。自定义主题只覆盖变量、用 `style.setProperty` 生效，
  **不要**改成注入样式表——那样一个变量名写错就能把整个界面搞白。
- **自定义主题自带底色**（`base`）：它生效时 `data-theme` 取 `base`，用户点「明亮/深色」
  会先退回内置配色（并提示原主题还在）。别把 `applyTheme` 改成只看 `theme`——
  那会让自定义主题在切模式后看起来「没生效」。
- **归档只是元数据，文件不动**：归档的对话仍会被 `agent_transcripts` / 全文检索看到，
  语义是「先收起来」。侧栏把它渲染在各主题展开后的「已归档」分组里。
  分叉点定在**最后一条 assistant 消息之后**（不是字面上的最后一条），
  否则末尾那句「用户刚提问、模型还没答」会让新对话一开头就欠一个回答。
- **web_scan / web_eval / web_screenshot 只对「已建原生视图的远端网页标签」可用**
  （`agent/tools/webbrowser.rs` + `viewer/webview.rs` 的 `eval_js` / `cdp_call`）：
  本地 HTML（也是 `Web` kind，靠 `path` 是否为 Some 区分）、PDF、Markdown 一律在
  `require_remote_web_tab` 报错，别放宽这个判断；内置浏览器面板收起时原生视图不存在，
  报错文案要让模型引导用户先展开面板。三条通道的机械细节：
  - **eval 的完成值是 JSON 字符串**（tauri `eval_with_callback` 的约定：字符串结果
    外面还包着一层引号），Rust 侧要先解一层再 `serde_json` 解析；
    Windows 上页面异常会被吞成 `null`，所以扫描脚本必须自己 try/catch。
  - **CDP 调用必须 `run_on_main_thread` + `with_webview` 拿 controller**，
    完成回调在主线程触发，**只往 channel send，绝不等待**；等待（`recv_timeout`）
    是阻塞调用，包在 `spawn_blocking` 里，别让 async worker 陪着卡。
  - **`data-lh-ref` 是页面上的临时属性**（web_scan 打的标记，编号挂在
    `window.__lhRefSeq` 上递增、复用），刷新页面就没了——不是持久状态，别往数据库写。
  - eval 结果 20k 字符封顶（`EVAL_RESULT_MAX`）才给模型；调试这两个通道有
    `debug_web_eval` / `debug_web_screenshot` 两个命令，**只在 debug 构建存在**
    （`generate_handler!` 参数加不了 `#[cfg]`，注册在 `lib.rs` 的
    `main_invoke_handler` 里用两张表合并，release 构建没有这两个符号）。
    依赖只有 `webview2-com 0.39` + `windows-core 0.62`——**必须跟 wry/tauri
    依赖树里的版本一致**（0.62 不是 0.61，差一个 minor HSTRING 就是两个类型）。
- **一键更新走自研三步，别换成 tauri-plugin-updater**（`updater.rs`）：官方插件要
  minisign 签名 + 固定 endpoint 的 latest.json，预发布也不在 `/releases/latest`
  别名里，跟「β 满天飞、`gh release` 手动发」对不上。要点：GitHub API **无认证
  限流 60 次/小时**，所以只在用户点按钮时查（挂载时那一次 + 手动按钮），别加自动
  轮询；`/releases` 列表里 draft 要跳过、prerelease 要算（semver 原生支持
  `0.3.2-beta.1 < 0.3.2`）；asset 名字必须与 `scripts/package.mjs` 产出的
  `LearningHub-<版本>-x64-setup.exe` 对齐（没有安装包的版本跳过、退而取次新的）；
  安装是分离启动 NSIS 安装器 `/S /R`（静默 + 装完自动重启，应用还在运行由安装器
  处理），`creation_flags` 里的 `CREATE_BREAKAWAY_FROM_JOB` 失败要去掉它**降级重试**——
  有的宿主不让脱离 job，硬失败会让「下载成功却装不上」。

## 当前状态（v1）

已完成：主题/笔记/资料/卡片/计划/会话/测验 七个模块、内置浏览器（PDF 含文字层 / Markdown / 网页 / 图片 / 文本）、
内置笔记编辑器（移植自 InkNote 的 CodeMirror 6 所见即所得）、讲解模式（讲解方案 + HTML 演示页 + 来源标注）、
知识库（讲义入库 + 带出处的检索）、技能与 MCP（侧栏入口，支持全局/本主题两级开关，沿父子链继承；
MCP 面板可一键导入本机 `~/.agents/servers/` 里已配置的服务器并重连）、
长期记忆（侧栏「记忆」面板；agent 用 `memory_write` 自己记，每轮注入系统提示词，
全局/本主题两级 + 父主题继承）、
对话管理（侧栏对话行悬停出「⋯」：重命名 / 置顶 / 分叉 / 归档 / 删除；元数据在侧车文件里）、
思维导图（Mermaid）、拖放导入资料、主题模式（跟随系统/明亮/深色）、
工作区沙箱与越权申请、权限分级、两类模型协议（OpenAI 兼容 + Anthropic）、日程视图、设置页、
父子主题（「只学一门课里的一章」：资料继承、笔记/卡片/计划独立）、
用量与缓存（对话框底部「缓存 X%」胶囊，悬停看上下文构成与累计命中率；数字来自服务商报的真实 usage）、
资料不看类型（`materials/` 里的 PDF / Markdown / txt / 图片都能列、能在内置浏览器预览、
  窗口回前台会自动重拉清单）、
内置浏览器按标签读（agent 用 `viewer_list` + `viewer_read(tab_id)` 能读**任意**标签页，
  包括用户当前没在看的后台标签，本地文件与网页都由后端直接提取）、
图片消息（输入框里 Ctrl+V 贴图 / 「图片」按钮选图；长边自动压到 1568px；
  字节落在 `.hub/attachments/`，消息里只存路径；两个协议各自编码成 `image_url` / `image` 块；
  点开是浮层大图）、
自定义外观主题（`<工作区>/.hub/themes/*.json` 与 `~/.learning-hub/themes/*.json`；
  一份「CSS 变量覆盖表」，设置页里能从当前配色复制一份来改，也有变量清单可查）、
工坊（独立于学习的 agent 模式：读内置的 SKILL / MCP 规范 → 在练习目录
  `.hub/workshop/` 里写 → `skill_publish` / `mcp_publish` 发布；发布即重连并回报状态；
  自己的对话清单挂在侧栏「工坊」下面）、
演示卡片（agent 讲难懂内容时在回复里直接放 ```html / ```svg 代码块，前端渲染成沙箱卡片；
  脚本不执行，动画用 CSS / SVG SMIL，卡片可看源码、可全屏）、
PDF 看图（`pdf_screenshot` 把 PDF 的某一页渲染成图片给模型看——图、表、公式、扫描页；
  渲染在前端 pdf.js，图片落进 `.hub/attachments/` 并作为工具结果的图片回灌，
  工具卡片里能看到 agent 看了哪一页）、
网页操作（`web_scan` / `web_eval` / `web_screenshot`：agent 能扫描内置浏览器里网页的结构、
  在页面里执行 JS、把网页截图给自己看——对标外部浏览器操作 MCP 的能力，
  纯 Rust 内置零依赖，走原生子 WebView 的 eval / CDP 通道，只对远端网页标签生效）、
专注模式（F11：窗口全屏 + 收起标题栏/侧栏/内置浏览器，写笔记时整屏）。

已知待办（按价值排序）：

1. 语音输入（图片已经能贴了，语音还没有）
2. 复习提醒（系统通知 + 到点弹窗）
3. 周报/月报生成（`sessions/` 的数据已经够用）
4. SPA 网页的正文提取（现在依赖服务端渲染的 HTML）
5. 父主题的进度汇总（子主题的待复习数、任务合并到父主题视图）
6. 附件回收（删对话后 `.hub/attachments/` 里的孤儿图片要手动清）
