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
npm run dist                                           # 打包（tauri build + 安装包 + 绿色版 zip）
npm run portable                                       # 只出绿色版 zip（要先 build 过）
npm run shortcut                                       # 把打包版同步到 release/app 并在桌面建快捷方式（应用开着时会跳过同步并提示）
npm run demo:seed && npm run demo:pdf                  # 造演示数据（workspace/）
npm run dev:fake-llm                                   # 本地假模型（无需 API Key 验证对话链路）
npm run test:e2e                                       # 端到端冒烟（需要应用带调试端口启动）
npm run test:remote                                    # 远程控制的端到端（假模型自起自收 + 真浏览器量布局）
```

跑完改动后至少执行：`npm run typecheck` + `npm run rust:check`。改到 `domain/` 或 `viewer/` 时必须跑 `npm run rust:test`；
改了前端的纯逻辑（`src/lib/`、hook、统计口径）把 `npm run test:ui` 也跑上（测试放在被改文件旁边，`*.test.ts(x)`）。

### 验证 UI 的正确姿势

不要靠截图目测像素。用 CDP 直接量 DOM、点按钮：

```bash
# 带调试端口启动
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
npm run app:dev
# 注意：机器上**已经开着**一份学习中枢时，dev 实例会因 WebView2 用户数据目录被占用而
# 起不来（HRESULT 0x8007139F）。给 dev 单独指一份：
#   export WEBVIEW2_USER_DATA_FOLDER="C:/Users/<你>/AppData/Local/Temp/lh-dev-webview"

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
- **从 GUI 进程起控制台程序会弹一个黑框**（用户报的「运行一些命令的时候会出现黑框」）：
  Windows 会给子进程**分配一个新的控制台窗口**，`powershell` / `cmd` / MCP 服务器
  （`npx` / `uvx` / `python`）都是控制台程序，一 spawn 就闪一下黑框。必须加
  `CREATE_NO_WINDOW`——统一走 `src-tauri/src/proc.rs` 的 `no_console()`（`tokio::process`
  的命令用 `cmd.as_std_mut()` 转过去）。踩过三处：MCP 服务器连接、远程面板枚举网卡的
  PowerShell（缓存 60 秒，所以症状是「每分钟闪一下」）、用默认程序打开文件的 `cmd /C start`。
  **新加起进程的地方都要过 `no_console`**；唯一例外是更新器启动 NSIS 安装器
  （那边要的是 `DETACHED_PROCESS`，自己设 flags）。
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
- **选中色不能用 `--accent-soft`**（用户报的「夜晚模式选中了还是黑的」）：它在深色下是
  `#2c1e17`，压在 `#1a1a19` 的底色上等于没选中。全局 `::selection` 用半透明的强调色
  （`color-mix(in srgb, var(--accent) 30%, transparent)`，和编辑器里的 `--selection` 一个路子）：
  半透明在任何底色上都看得见，也不用为深浅两套配色各写一遍。
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
  agent 下一次 `viewer_read` 自动重新提取。新窗口请求（弹窗 / target=_blank / 中键）一律
  `Deny` 真正创建、转成内置浏览器**新标签**并前台激活（发 `ViewerEvent::Sync` 让前端刷新；
  非普通网页如 `mailto:` 原样拒绝）——用户明确不要「点链接跑系统浏览器」。
  Ctrl+滚轮缩放挂在 `zoom_hotkeys_enabled(true)` 上：wry 把 WebView2 的
  `IsZoomControlEnabled` / `IsPinchZoomEnabled` 都绑在这个属性，默认 false（网页缩不了）；
  键盘加速键（Ctrl+P 等）在 wry 里恒为关，开了这个也不受影响。
- **原生子 WebView 浮在一切 DOM 之上**：它是独立的原生窗口，Modal / 图片大图 / 命令面板 /
  拖放提示 / 演示卡片全屏（Fullscreen API）都会被它盖住。所以这些全屏浮层都要挂
  `useOverlay()`（`store` 里的 `overlayCount`），`WebTab` 计数非零 / 阅读模式 / zen 时调
  `viewer_webview_set_visible(false)` 让路；右下角 Toast 压不过它，`nativeWebviewUp` 为真时
  ToastHost 换到左下。bounds 同步是 **CSS px → 物理 px**：前端量 `getBoundingClientRect`
  原样传（相对视口），Rust 按 `scale_factor` 换算取整（`to_physical_rect`，有单测）；
  宿主尺寸/位置变了要 `viewer_webview_bounds`（前端用 rAF 合并）。w/h 不足 2 视为不可见，
  只 `hide()` 不设 bounds。安全边界：`web-<tab_id>` 不在 capabilities 的 windows 列表里，
  外部网页拿不到任何 IPC 权限；`on_navigation` 只放行 http/https（含挡 `http://ipc.localhost`）。
  **分界线手柄同理不能放面板里**：`.viewer` 的 `overflow: hidden` 会把伸出边框的半截裁掉，
  原生层又从面板内容区开始画——手柄放 `.viewer` 里时头部以下整段被盖住（用户报的
  「分界线只有上面一截能拉」）。它现在挂在 `.main-body` 上、贴在面板**外侧**，别挪回去；
  拖动时 `setViewerWidth` 会给第二栏留 400px 最小宽度，别把钳制删了（否则中间被挤成一列字）。
- **内置浏览器的头部只有一行**：标签 + 页码 + 已读字数 + 重载/外部打开都在 40px 那一行里
  （完整路径放标签的 tooltip）。原来下面还有一条路径栏，白占一行还把正文挤掉一截，被用户点名拿掉，别再往回加。
- **窗口是自绘标题栏**（`tauri.conf.json` 里 `decorations: false`，见 `components/TitleBar.tsx`）：
  系统标题栏属于 Windows 画的非客户区，配 WebView2 时它会压在网页内容上面
  （用户报的「一条黑框一直在挡界面，上面是最小化/窗口化/关闭」）。自绘后窗口里只有 DOM。
  注意两点：**调 window API 需要 capabilities 里显式授权**
  （`core:window:allow-minimize`、`allow-toggle-maximize`、`allow-close`、`allow-is-maximized` 等；
  漏了会在运行时报权限错误，typecheck 查不出来）；拖拽靠 `data-tauri-drag-region`
  （需要 `core:window:allow-start-dragging`），按钮上别加这个属性，否则点不灵。
- **`core:window:allow-destroy` 少不得，而且它连着两个用户可见的 bug**（都踩过）：
  JS 侧一旦注册了 `onCloseRequested`，Tauri 就会 `prevent_close()`（见 tauri
  `manager/window.rs`），**关窗口只能由 JS 调 `destroy()` 完成**。这个权限漏掉的症状是
  「右上角关闭点了没反应」——而且异常落在事件回调里没人接，界面上一点提示都没有；
  更糟的是**一键更新也一起卡死**（见下面 updater 那条）。`lib.rs` 里有个单测盯着这张
  权限清单，删一条会红。
- **`element.click()` 在自绘标题栏里不一定灵**：非可信的合成 click 事件，
  标题栏里那几个按钮**有时**收不到 React 的 `onClick`（同一个按钮，同一次会话里
  时灵时不灵；`dispatchEvent(new MouseEvent(...))` 和真实鼠标事件都正常）。
  写自动化脚本要验标题栏上的按钮（尤其是关闭），**用 CDP 的 `Input.dispatchMouseEvent`
  发真实鼠标事件**（`scripts/restore-check.mjs` 就是这么做的），别用 `.click()`。
- **页面被滚走 = 自绘标题栏整个消失**（用户报的「上面的三个图标和 logo 都不见了」）：
  `body { overflow: hidden }` 会传给视口，视口**滚轮滚不动，但 `scrollIntoView` / `focus()`
  仍然滚得动**。壳（`.app-shell`）里只要有个元素越出下沿（绝对定位的浮层最容易；`.app-main`
  没有 overflow，它的绝对定位子元素能直接顶到页面外），一次 `scrollIntoView`（`Viewer.tsx`
  的引用跳转、`PdfView.tsx` 的翻页都会调）就把**页面**滚了上去，32px 的标题栏滑出可视区：
  logo 与最小化/最大化/关闭三个按钮全没了，而且滚不回来，只能重启应用。
  **现在有三道锁，都别拆**：`html, body { overflow: clip }`（clip = 连程序化滚动都不行；
  写成 `hidden` 就还是「可滚动容器」，只是滚轮滚不动）、`.app-shell` 上的 `overflow: clip`、
  `.titlebar` 的 `position: sticky; top: 0`。
  这个 bug 的**引子**是 2026-10 才查清的：**pdf.js 的 TextLayer 会往 `document.body` 上挂一个
  `<canvas class="hiddenCanvasElement">` 量字体宽度**，它不占尺寸但仍是行内元素，于是给页面
  撑出一个约 **21.33px** 的行框——页面从此「可滚动」，翻一页 PDF 就把整个界面顶上去：
  标题栏以下的 chat-head / 内置浏览器标签条被自己的 overflow 裁掉上半截，露出来的那半截压在
  侧栏与正文的文字上（用户报的「遮挡正常文字」）。`app.css` 里
  `canvas.hiddenCanvasElement { display: none }` 就是治它的（`display: none` 不影响
  `measureText`，已 A/B 验过文字层宽度与 scaleX 完全一致）。**往 `document.body` 上加东西时
  先想一下它占不占位置**——`#root` 之外的任何行内/块级元素都会加进页面的可滚动量。
  排查这类问题时**别信 `documentElement.scrollHeight`**（它会被 `overflow: hidden` 骗成视口高）：
  用 `window.scrollTo(0, 99999)` 再看读回来的 `scrollY`，那才是页面真正的可滚动量；
  想找出是谁撑高的，用「把子树一个个藏起来看高度掉不掉」——但注意 `.app` 是 flex，
  藏一个孩子会让别的孩子重排，逐个隐藏会漏，要反过来「全藏起来、一次放一个」。
- **侧栏拖拽用自己的指针事件实现**（`Sidebar.tsx` 的 `ChatRow`）：Tauri 在 Windows 上
  `dragDropEnabled: true` 会接管文件拖放（资料导入要用），HTML5 的 `dragstart`/`drop`
  **根本不会触发**——想加「拖动某个元素到另一处」的交互时别再试 HTML5 拖放。
  现在的做法：pointerdown 记住起点 → 移动超过 5px 才算拖拽（否则仍是点击）→
  跟随一个 body 上的幽灵小卡片 → `elementFromPoint` 找落点（主题行带 `data-slug`）→
  pointerup 调命令。`setPointerCapture` 要包 try/catch，合成事件下会抛。
- **分界线的光标接管别和 `hasPointerCapture` 绑在一起**（用户报的「拖动分界线后光标回不来」）：
  两个手柄（`SidebarResizer`、`Viewer` 的 `.viewer-resizer`）原来写的是
  `if (!el.hasPointerCapture(id)) return;` 之后才 `document.body.style.cursor = ""`，
  而 `setPointerCapture` 是会失败的（合成事件、指针已失效、元素中途卸载），一旦失败
  **光标就永远停在 `col-resize`**。现在统一走 `src/lib/dragcursor.ts` 的 `useDragCursor()`：
  「是不是我的拖动」看自己的 pointerId，「还光标」无条件做，而且 pointerup / pointercancel /
  lostpointercapture / 窗口失焦 / 组件卸载五条路都做（有回归网 `dragcursor.test.tsx`）。
- **手柄元素要用 state（回调 ref）而不是 `useRef` + 依赖数组**（用户报的「分界线不可拉动」）：
  两个手柄都是**条件渲染**的（内置浏览器收起时不存在），用 `useRef` 时 effect 只会在
  挂载那一刻看一次 `ref.current`——那一刻手柄不在就是 null，之后手柄出现了**监听也没接上**，
  于是「面板打开之后分界线再也拖不动」。这个坑踩了两次（第一次只补了依赖数组，漏一个还会再犯）。
  现在两边都写 `const [handle, setHandle] = useState<HTMLDivElement | null>(null)` +
  `ref={setHandle}`，effect 依赖 `[handle, …]`：手柄一出现就跑，结构上不会再漏。
  另外两个手柄都加了 `touch-action: none`——不写的话触摸设备上手指一拖会被浏览器当成
  滚动手势接管（发 pointercancel），鼠标下完全看不出来。
- **对话清单是「按需缓存」的，别的窗口改了必须跟着刷**（用户报的「移动端网页会显示已经删除的对话」）：
  `chatIndex[slug]` 只在侧栏展开某个主题时读一次，读过就再也不会自动重读（Sidebar 里那个
  lazy effect 还会 `if (chatIndex[t.slug]) continue` 跳过）。所以手机（＝同一个前端、另一头）
  删掉一条对话时，桌面这边会一直显示它——反过来也一样。
  现在 `hub://topics` 的 Refresh 处理里会调 `store.refreshChatLists()`：**只重拉缓存过的那些键**
  （外加拉过的工坊清单），不把所有主题扫一遍。Rust 那边不用动：几个 `chat_*` 改命令本来就会发
  Refresh（`overview_after_change` / `chat_move` / `chat_delete` / `chat_fork`），
  而 `chat_overview` 这个**读**命令不发（读命令发事件会让「收到事件就重拉」自激）。
- **`AgentEvent::Message` 必须带 `chat_id`**（用户报的「模型认为它输出了讲解和提问，我却没读到」）：
  前端只把「当前正在看的这条对话」的消息追加进列表，以前事件里只有 `turn_id`，只能靠
  `streaming` 还在不在来猜——**用户中途切走一下（切对话、切主题都会清 `streaming`），
  这一轮剩下的回复就整条进不了界面**：模型生成了、jsonl 里也写了、下一轮它自己还引用得到，
  界面上却永远缺那一段（看着像「工具调用坏了」或「被截断」）。判据改成 `ev.chat_id === s.chatId`，
  并按 message id 去重（重连/重试会把同一条再推一次）。
  另外 `finished` 时如果这一轮就是当前对话的，顺手 `syncChatTail()` 跟盘上对一次尾巴
  ——**只追加不覆盖**：直接 `loadChat` 重读会跟「用户马上又发一条」抢（`loadChat` 的竞态序号
  只认切对话，不认新一轮）。
  查这类问题时**先看 jsonl**（`<主题>/.hub/chats/<id>.jsonl`）：界面上没有、盘上有 →
  丢在「事件/渲染」这一侧；盘上也没有 → 才是模型或请求的问题。
- **同一轮里模型分几条消息到达，但界面上是「一轮」**（用户报的「不要一次循环就出现一次头像」）：
  模型每次工具调用之后都会再回一条 assistant 消息（jsonl 里也是分开的多条，协议要求如此），
  所以渲染要按轮分组：头像只在这一轮的第一条上画，后面几条用 `.msg-avatar.cont`
  （`visibility: hidden`，占位不塌陷）撑住左边距。判据是「前一条不是 assistant」
  （`tool` 消息不参与渲染，见 `Chat.tsx` 的 `visible`）。活动卡片与流式气泡同理：
  这一轮已经画过头像就不再画。
- **输入框三个回车键**（`Composer` 传给 `AutoTextarea` 的三个回调）：Enter＝直接发；
  **Ctrl+Enter＝打断当前一轮再发**（`queueSend(…, interrupt=true)`）；
  **Alt+Enter＝让这一轮跑完（当前工具循环结束）再发**（`queueSend(…, false)`）。
  两个「排队」的都不立刻发：先入 `store.queued`（里面记着**哪条对话的哪一轮**），真正发送交给
  `flushQueued`，在那一轮的 finished / failed 里触发。**别改成「立刻塞进去」**：中止那一轮要先给
  没执行完的工具调用补上「已被用户中止」的 `tool_result`（tool_use / tool_result 必须成对落库，
  否则这条对话之后每次请求都被服务商 400），等它收尾再发，落库顺序天然是对的（中止标记在前、
  新消息在后）。队列**发出去了才清**——被「上一轮还在进行中」挡回来或发送报错时留着，
  别把用户打的字弄丢；回到那条对话时（`loadChat`）也会再试一次。
  回归网：`src/store/queue.test.ts`（9 例：等哪一轮 / 发到哪条对话 / 什么时候清）。
- **对话可以在主题之间搬**（`chat_move` 动 `.hub/chats/<id>.jsonl` **和**同名的 `.meta.json`）：
  搬的是文件，聊天记录里已有的相对引用不会跟着重算——所以这是「整理」语义。
  正在看的那条被搬走后，前端会给源主题新开一条，免得接着聊又写回旧主题。
  它发的是 `TopicsEvent::Updated`（两个主题各一条），不是 `Refresh`——**测「别的窗口会不会刷新」
  时别只看标题在不在**：搬走之后标题会出现在目标主题下面，得按「每个主题挂着的对话行」分组看。
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
- **MCP 服务器起不来时，原因必须在界面上看得见**：发布版是 GUI 程序
  （`windows_subsystem = "windows"`），**没有控制台**——子进程的 stderr 只 `eprintln!`
  就等于丢掉。踩过的连锁反应：agent 看到「启动失败」却没有任何原因，只能瞎猜，
  改一次发一次；`tools/list` 失败还被算成「已连接、0 个工具」，看着像服务器没干活。
  现在的收口是 `mcp::McpLog`（每个服务器一圈环形缓冲：启动命令、解析到哪个文件、
  stderr 尾巴、退出码）+ `ProbeReport`：面板能展开看、`mcp_status` 会带上、
  `mcp_publish` 失败时返回、`mcp_probe` 专门用来「不写配置地试一次」。
  **新加的「起子进程」的地方都要想到这一条**。
- **`CreateProcess` 不认 `PATHEXT`**：`npx` / `npm` / `uvx` 是 `.cmd` shim，
  `Command::new("npx")` 只会去找 `npx.exe`，必然 ENOENT。统一走
  `proc::resolve_command`（按 PATH + PATHEXT 找，`.cmd` / `.bat` 套一层 `cmd.exe /C`），
  它和 `no_console` 是起进程的两件套。
- **工坊（studio）卡住的多半是「环境」，不是「协议」**：那里没有终端、没有 shell 是
  一开始的设计，结果 agent 想装个依赖、跑个脚本验证都没路，只能把活推给用户——
  而用户明确说过**他不懂技术，丢一个需求就该 agent 自己做完**。现在练习目录里给了
  `run_command`（一次一条、不能交互、无管理员权限、有超时、走 `no_console`），
  测 MCP 服务器给了 `mcp_probe`（起进程 → 握手 → 列工具 → 关掉，不写配置）。
  **加能力时想的是「让 agent 自己闭环」，而不是「让用户去跑一行命令」**；
  两份规范文档（`src-tauri/docs/studio/*.md`，`include_str!` 编进二进制）与
  `prompt.rs` 的工坊提示词里写清了这些边界，改工具要同步改它们。
- **仓库根的 `package.json` 写着 `"type": "module"`**：这个字段对**目录树里所有 `.js`**
  生效，所以工作区里（以及本仓库里）写的 Node 脚本必须用 ESM `import`，
  用 `require` 会直接崩（`require is not defined`）——工坊文档里的 Node 示例已经改成
  `import`，别改回去；真要 CommonJS 就把文件命名成 `.cjs`。
- **切主题 / 切工坊 / 新建对话都不许清 `streaming`**：它同时喂着侧栏那个转圈与输入框的
  「停止」按钮，清了就既看不到「正在跑」也没法叫停（用户报过「工坊里没有停止对话的按键」）。
  要判断那一轮还在不在，用 `agent_running`（前端 `reconcileRunning`，窗口重新聚焦时对账）。
- **Windows 控制台程序写的是本地代码页，不是 UTF-8**：cmd 内建命令、PowerShell 5.1、
  Python 往管道里吐的都是 GBK（中文机器上）。按 UTF-8 硬解 → 一串问号，
  而且 `BufReader::lines()` 会因编码不合法**直接结束读循环**——MCP 服务器崩在
  Python traceback 上时，「起不来」的原因就这么丢了。命令输出与子进程日志一律走
  `proc::decode_output`（先严格试 UTF-8，失败按 GBK 兜底；GBK 兼容 ASCII，
  纯英文输出两种解出来一样）。
- **本地 HTML 有两种渲染路径，别混**：普通本地 HTML（用户自己丢进来的、`lessons/*.html` 里的旧文件）
  走 `sandbox=""` 的 srcdoc iframe——**不跑脚本**；agent 用 `page_create` 建的**演示页**走
  `lhpage://` 协议 + 原生子 WebView，脚本、WebGL、`import` three.js、读同目录文件全都成立。
  两条路的判据在前端是 `isPageUrl(tab.url)`（地址形如 `http://lhpage.localhost/<token>/…`），
  在 Rust 侧是 `viewer::pages` 那一套。**演示页标签不要按 `path` 当本地文件处理**：
  地址栏 / 阅读模式 / 「用系统程序打开」对它都不成立（内部协议，系统浏览器打不开）。
- **`lhpage://` 的可读范围靠通行证钉死**：token → 目录的映射（`viewer/pages.rs`），
  页面只能读**它自己那一级目录**里的文件，`..` / 绝对路径 / 盘符 / `%2e%2e` 一律拒。
  想做「让页面读别的地方」之前先想清楚：那等于把用户的资料暴露给模型写的脚本。
  另外这张表**只在内存里**，所以启动恢复标签时要 `AppCore::republish_page_urls()`
  重新发一遍（漏了的话用户开机看到一张「通行证无效」的页）。
- **演示页的 CSP 里 `connect-src` 是刻意收死的**（只给 `'self'`，不给 https）：
  本地 `.html` 都会被当演示页跑起来，用户从别处下载的页面可能就躺在 `materials/` 里，
  放开 `connect-src` 等于给它一条「读走讲义再发出去」的路。CDN 上的**库**仍然能加载
  （走 `script-src`，那条放开了）——**要联网取数据的页面做不到**，让 agent 先把数据写进页面目录。
  改这段 CSP 之前先想清楚这两句。
- **演示页是唯一会被 agent 自己 eval 的本地页面**：`require_remote_web_tab` 对
  `lhpage` URL 开了口子——因为「动画到底画出来没有」只能靠 `web_eval` + `web_screenshot` 看
  （真踩过：模型写的页面 `new OrbitControls(...)` 少了 `THREE.` 前缀，脚本在第 23 行抛错、
  画布一片空白，而**只有**截图能发现）。PDF / Markdown / 普通本地 HTML 仍然一律拒绝。
- **`/_vendor/three.js` 是编进二进制的**（`include_str!`，见 `src-tauri/assets/vendor/README.md`）：
  改它要重新编译，不是热更新。升级用 `node scripts/vendor-three.mjs [版本]`——
  three 从 r160 起拆成 `three.module.js` + `three.core.js` 且不给 min 版，OrbitControls 又是
  `from 'three'` 的裸模块名，所以是**构建期用 esbuild 打成一个 min 模块**再提交。
  只预置这一个库：别的库让页面自己写 CDN（提示词里说明了断网就会白屏）。
- **`chat_delete` 的 id 一定要拿「当前」的**：`useApp.getState()` 返回的是**快照对象**，
  `await s.newChat()` 之后 `s.chatId` 还是旧值（导致测试脚本删掉了用户正在用的那条对话，
  已从回收站恢复）。要读新值就重新 `getState()`。**验收脚本删东西之前先确认自己删的是自己建的。**
- **`scripts/cdp.mjs` 会挑目标**：演示页 / 远端网页跑在子 WebView 里，也会出现在
  `/json/list`；默认按 `localhost:1420` / `tauri://` 挑回应用自己那一页（要指定用
  `CDP_TARGET=url 子串`）。不挑的症状是 eval 报「找不到 /src/store/app.ts」。
- **Mermaid 图放大要放大 svg，而不是画布容器**：`.mermaid-block` 是 `overflow: hidden` 的，
  把 `.mermaid-canvas` 自己放宽到 160% 的话，超出部分被裁掉、滚动条永远不出现
  （症状：点「放大」只能看到局部，没有任何办法挪）。正确做法：可视区固定块宽、
  `svg.style.width = k*100%`，让容器的 `overflow: auto` 出滚动条；居中用 `safe center`
  （普通 center 会把溢出的左半边裁进不可滚动区）。放大后按住拖动平移的实现见
  `markdown.ts` 的 `attachPan`（只在真的超出时接管指针）。
- **「界面缩放」用的是 WebView2 的 zoom（等效浏览器缩放），别改成 CSS zoom**：
  浏览器 zoom 下 `getBoundingClientRect` 的数值不随缩放变（CSS px 是页面自己的单位），
  但**视觉尺寸 = CSS px × zoom**——所以 Rust 侧把前端量来的矩形换算成物理像素时
  必须再乘配置里的 `ui_scale`（`viewer_webview_bounds` / `viewer_webview_ensure` 两处，
  都走 `to_physical_rect(scale × zoom)`；将来加「原生层按矩形摆放」的新功能也要记得乘）。
  应用缩放统一走 `commands::app::apply_ui_zoom`（启动 setup 一次 + `ui_scale_set` 一次），
  别在别处再调 `set_zoom`。
- **`chatMode` 是侧栏判断「刚开、还没落盘的工坊对话」的依据**：只看 `view` 不够——
  切视图那一刻 `chatId` 还是上一条，会闪出一行假的「新对话」。`loadChat` 的第二个参数
  就是给这个用的，从主题/工坊清单进来的点击要显式传模式。
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
- **```svg 演示卡片的壳必须给 svg 显式宽高**（`markdown.ts` 的 `demoDocument`，踩过一次）：
  模型常写「只有 `viewBox`」的 SVG，这种图在 `display:grid` 的壳里按 auto 尺寸排版会塌成
  **0×0**——srcdoc 正常加载、元素都在，就是什么都不画，看着像「最新版本渲染坏了」。
  `max-width` / `max-height` 救不回来（真机 + 独立浏览器对照实验都试过；去掉 grid 也不对，
  会丢垂直居中）。现在壳里写 `svg{width:100%;height:100%;…}`，由 viewBox 默认的等比缩放居中。
  再遇到「演示卡片空白」别急着怀疑 iframe / CSP / 沙箱：先做一个**去掉 sandbox 的复现页**
  （这样才能 `contentDocument` 进去），量元素尺寸、看 srcdoc 加没加载。
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
- **模型请求的 HTTP 客户端要现建**（`AppCore::http_fresh`，踩过一次）：reqwest
  **只在建客户端时读一次**系统代理，之后不再刷新。以前对话/测试共用启动时建的那个
  `core.http`——用户开着 v2rayN 启动、中途关掉代理之后，所有请求继续往已经死掉的
  `127.0.0.1:10808` 里送（症状：`网络请求失败：error sending request`，失败耗时
  和走死代理完全一致，重启应用才能恢复；`ProxyEnable=0` 也没用，客户端记的是启动那刻的值）。
  现在对话轮 / `complete_once` / `profile_test` 三处都走 `core.http_fresh()?`——
  成本只是每轮重建一次连接池，换来「发请求那一刻的系统代理」。抓网页 / 搜索 / 更新检查
  仍用共享客户端（连接池复用），那些路径要现建的话照抄这个方法。
  排查这类问题时：先 `curl -m 8 -x <代理> <url>` 对比耗时，再 `netstat` 看端口在不在听。
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
- **远程控制是「手写的迷你 HTTP + SSE」，别顺手换成框架**：`remote/http.rs` 只实现
  需要的那一小块（定长正文、定长响应、SSE 分块），每个上限都有单测兜着。
  几个连带约定：**静态资源不需要令牌、`/api/*` 一律要**（分界就在 `api::route` 开头）；
  shim 用 `include_str!` 编进二进制，改完必须重新编译（不是热更新）；
  所有事件都从 `AppCore::emit_event` 出去（本地一份 + 远程一份），
  **别在对话循环里塞远程逻辑**（`receiver_count() == 0` 时连序列化都省了），
  也别绕开它直接 `app.emit` ——那样新事件会静默地不推给手机。
- **手机上的界面就是桌面端那份产物，别再写第二套**：远程服务把 `dist/`
  （`AssetResolver::get`，dev 模式下从磁盘读）直接送出去，注入 `remote/web/shim.js`
  把 `window.__TAURI_INTERNALS__` 换成 HTTP/SSE 版。加功能不用管手机——**除非**它在
  手机上根本做不了（窗口控制、系统文件框、原生子 WebView、拖放）：那就在
  `src/lib/remote.ts` 的 `isRemote()` 上收起来或换成浏览器做法，窄屏样式挂在
  `html[data-remote]` 上。前端改完要 `npm run build`（送出去的是编译产物）。
- **远程的能力边界在 `remote/bridge.rs` 的一张表里**（`ALLOWED` / `DENIED` 带原因），
  而且**每个命令都必须被分类**——底下那个单测直接解析 `lib.rs` 的命令表，漏一个就红。
  界线：**「让 agent 代劳 + 点一下允许」能做到的，远程也放开**（读、聊、审批、复习打分、
  写笔记、建卡片/任务/测验、整理对话、记忆增删改）；**改应用配置与密钥、改工作区结构、
  批量导入/重建、直接删文件、更新应用、技能与 MCP 的开关、桌面外壳**一律拒。
  `config_patch` 只放行 `activeProfileId` / `theme` 两个字段——**权限分级（`permissionMode`）
  明确不放**，从一台可能只是借来的手机上打开「完全访问」等于关掉审批。
  同理：附件路径必须走 `paths::resolve_in_root`，令牌比较必须是常量时间的
  （`http::token_matches`），新增接口记得在 `route` 里放在鉴权之后。
- **手机上的一切都经电脑上那个前端执行**（`hub://remote-invoke` → `lib/remoteinvoke.ts`
  → 同一个 `invoke` → `remote_report_invoke`）。两条铁律：**两端都要回话**（失败也要回，
  否则手机等满 30 秒），以及**等待要有上限**。二进制结果（`viewer_load_bytes`）不走这条桥，
  改走 `/api/tab-bytes` 直接流字节——别往 JSON 事件里塞 base64。
- **`TcpListener::from_std` 必须在 tokio 运行时里调**：在 `setup()` / 命令线程里直接调
  会 panic（`there is no reactor running`），而且是在**启动路径**上——整个应用起不来，
  报错看着像 Tauri 的锅。现在的写法：同步 `std::net::TcpListener::bind`（这样「端口被占用」
  能立刻回报给面板）→ 把监听器交给 `tauri::async_runtime::spawn` 里再 `from_std`。
- **远程的窄屏样式里，靠 hover 才出现的操作必须常显**：`.chat-actions` / `.row-actions` /
  `.mini-btn` / `.mermaid-tools` 平时是 `opacity: 0` 或 `display: none`，触摸设备上
  根本没有 hover——漏掉的症状是「按钮明明在那儿就是点不到」，而且没有任何报错。
  这一段的收口是 `app.css` 末尾 `html[data-remote]` 那一块（桌面端一行都不受影响）。
  另外手机上浮层显隐要控制 `display` 或加 `!important`：UA 的 `[hidden]{display:none}`
  优先级不如作者样式表里的 `.viewer{display:grid}`（历史坑，症状是「藏起来的浮层照样显示」）。
- **一键装中继的脚本要当「用户输入」对待**：`remote/deploy.rs` 的 `build_script` 把端口、
  口令、SSH 密码拼进一段 shell 脚本。密码走 `sh_quote`（有单测，`'; rm -rf /` 这种也要对），
  脚本本身走 SSH 的 **stdin**（`bash -s`）——走命令行的话服务器上 `ps` 就能看见密码。
  脚本里**不许出现 nginx / ufw / iptables 的写操作**（默认不反代、不动防火墙），
  这条也有单测盯着。
- **SSH 客户端用 russh，别换成调系统 ssh**：Windows 上没有 sshpass，密码登录只能交互输入。
  russh 关掉默认特性用 `ring`（默认的 aws-lc-rs 在 Windows 上要 cmake + NASM）。
  几个 API 坑：`check_server_key` 收的是 `russh::keys::PublicKeyOrCertificate`（`cert` 模块是私有的，
  公开路径在 `keys` 下）；`Channel` 不直接实现 `AsyncWrite`，发脚本用 `data_bytes`；
  handler 会被 move 进 `connect`，要在连接后读指纹就把 `seen` 做成 `Arc<Mutex<..>>`。
- **反向隧道只有三句话，方向不能反**：`LH1 <token>` / `PING` / `GO`，
  **空闲时只允许中继往客户端发**——客户端若自己发心跳，那几个字节会撞进
  「刚配对、即将开始原始搬运」的窗口，被当成 HTTP 请求灌给本机服务。
  默认端口是 **54713**（8787 是 RStudio 的，用户点名提过），旧配置里的 8787 会在迁移时换掉。
  池子由 `tunnel::run` 一个监管循环统一补（worker 配对成功后立刻通知它），
  别让 worker 自己再开连接：那样每次配对都会翻倍地长。
  等 GO 的超时是**每次读**重新计时（中继每 30 秒 PING），写成整体超时的话
  空闲隧道每 5 分钟就集体轮换一次、日志里全是假警报（踩过）。
- **局域网地址要枚举 + 排序，别只问默认路由**：装了 Tailscale / ZeroTier 的机器上，
  默认路由那块网卡不是手机能到的那块，二维码里会出现 `100.64.x.x`（实测踩过）。
  现在走 `remote::rank_addresses`：家用网段（192.168 / 10 / 172.16-31）优先，
  VPN 用的 CGNAT 段靠后，并把多个地址都列在面板上让用户换着试。
- **一键更新走自研三步，别换成 tauri-plugin-updater**（`updater.rs`）：官方插件要
  minisign 签名 + 固定 endpoint 的 latest.json，预发布也不在 `/releases/latest`
  别名里，跟「β 满天飞、`gh release` 手动发」对不上。要点：GitHub API **无认证
  限流 60 次/小时**，所以只在用户点按钮时查（挂载时那一次 + 手动按钮），别加自动
  轮询；`/releases` 列表里 draft 要跳过、prerelease 要算（semver 原生支持
  `0.3.2-beta.1 < 0.3.2`）；asset 名字必须与 `scripts/package.mjs` 产出的
  `LearningHub-<版本>-x64-setup.exe` 对齐（没有安装包的版本跳过、退而取次新的）；
  安装是分离启动 NSIS 安装器 `/S /R`（静默 + 装完自动重启），`creation_flags` 里的
  `CREATE_BREAKAWAY_FROM_JOB` 失败要去掉它**降级重试**——有的宿主不让脱离 job，
  硬失败会让「下载成功却装不上」。
- **安装器要靠「关得掉本应用」才能干活，这是更新链路上最脆的一环**：NSIS 静默安装
  起来第一件事是拿 Restart Manager 关掉正占着 exe 的那个进程，**关不掉它就往自己
  那个（分离进程看不见的）控制台写一行然后 `Abort`**——界面上的表现是「卡在『正在启动
  安装程序…』、什么都没发生、也没有任何报错」。所以：`allow-destroy` 那条权限是
  更新链路的一部分（见上面自绘标题栏那条）；前端在 `update_run` 前先
  `saveUiState()`（之后可能是强制终止，没有第二次机会）；`update_run` 里先
  `memory_flush()`。界面上备了一条 12 秒的「卡住」提示，会给出安装包路径和重试按钮——
  **别删**，那是这条链路唯一能看见失败的地方。

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
  **agent 自己闭环**——`run_command` 在练习目录里跑脚本装依赖、`mcp_probe` 不写配置地试运行，
  失败时 stderr 与退出码直接回到对话里；自己的对话清单挂在侧栏「工坊」下面，可新建对话）、
演示卡片（agent 讲难懂内容时在回复里直接放 ```html / ```svg 代码块，前端渲染成沙箱卡片；
  脚本不执行，动画用 CSS / SVG SMIL，卡片可看源码、可全屏）、
**会跑的演示页**（`page_create`：agent 写一个**本地真网页**（动画 / three.js 三维 / 可拖参数），
  在内置浏览器里当网页跑——`lhpage://` 协议给它真 origin，`/_vendor/three.js` 离线可用，
  可读范围用通行证钉死在页面自己的目录里；三条网页操作工具对它放行，agent 能自己
  `web_eval` + `web_screenshot` 确认「真的画出来了」；手机端降级成静态版）、
PDF 看图（`pdf_screenshot` 把 PDF 的某一页渲染成图片给模型看——图、表、公式、扫描页；
  渲染在前端 pdf.js，图片落进 `.hub/attachments/` 并作为工具结果的图片回灌，
  工具卡片里能看到 agent 看了哪一页）、
网页操作（`web_scan` / `web_eval` / `web_screenshot`：agent 能扫描内置浏览器里网页的结构、
  在页面里执行 JS、把网页截图给自己看——对标外部浏览器操作 MCP 的能力，
  纯 Rust 内置零依赖，走原生子 WebView 的 eval / CDP 通道，只对远端网页标签生效）、
专注模式（F11：窗口全屏 + 收起标题栏/侧栏/内置浏览器，写笔记时整屏）、
移动端远程控制（侧栏「远程控制」：手机/平板浏览器里操作这台电脑的 agent——
  **一键装中继**：给服务器 IP + SSH 用户名 + 密码，应用自己 SSH 上去装好、起常驻服务、
  填好配置（密码不落盘、公钥指纹 TOFU；默认端口 54713、默认不反代）；
  **手机上打开的就是电脑上那个界面**：远程服务把同一份 `dist/` 送出去 + 注入 shim，
  所有命令交给电脑上的前端用同一个 invoke 执行（白名单在 `remote/bridge.rs`）；
  同一 WiFi 直接扫码；在外面用就填「中继服务器」（`server/relay.mjs` 跑在自己的
  VPS 上，应用主动连出去建隧道，家里不用开端口）或填「对外地址」走现成的反代；
  竖屏抽屉 / 横屏侧栏常显，可「添加到主屏幕」；默认关闭，令牌分访问/中继两把，
  详见 docs/REMOTE.md）。

已知待办（按价值排序）：

1. 语音输入（图片已经能贴了，语音还没有）
2. 复习提醒（系统通知 + 到点弹窗）
3. 周报/月报生成（`sessions/` 的数据已经够用）
4. SPA 网页的正文提取（现在依赖服务端渲染的 HTML）
5. 父主题的进度汇总（子主题的待复习数、任务合并到父主题视图）
6. 附件回收（删对话后 `.hub/attachments/` 里的孤儿图片要手动清）
7. 演示页的入口（`page_create` 建的页面现在只能靠 agent 再打开、或者标签页还开着；
   可以在资料面板里加一栏 `pages/`，或把「上次讲过哪些演示」列出来）
