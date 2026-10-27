// 全局状态仓库（zustand）。
//
// 职责：
// 1. 持有后端推来的状态（配置、主题、对话、内置浏览器）
// 2. 订阅四类事件：agent / viewer / topics / toast，并转成 UI 状态
// 3. 把所有用户动作翻译成 api 调用
//
// 约定：**能由后端算的都放后端**，这里只做展示层的编排。

import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { registerRemoteInvoker } from "../lib/remoteinvoke";
import { isRemote } from "../lib/remote";
import { currentMonitor, getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import { fitOuter } from "../lib/winfit";
import { api, errText } from "../lib/api";
import { renderPdfPage } from "../lib/pdfshot";
import type {
  ThemeMode,
  ReasoningEffort,
  ReasoningStyle,
  AgentEvent,
  AgentMode,
  AgendaBucket,
  Card,
  ChatMessage,
  ChatOverviewItem,
  ConfigPatch,
  ContextPart,
  DailyBrief,
  ImageUpload,
  MemoryEvent,
  Note,
  NoteSummary,
  OpenRequest,
  PageText,
  PlanTask,
  ProfileInput,
  PublicConfig,
  StudySession,
  StudyStage,
  TabView,
  TaskWithTopic,
  ToastEvent,
  ToolOutcomeView,
  ToolSpec,
  TopicDetail,
  TopicSummary,
  TopicsEvent,
  ViewerEvent,
  ViewerSnapshot,
  ApprovalRequest,
  CardKind,
  CustomTheme,
  StudioInfo,
  ThemesOverview,
} from "../lib/types";

export type ViewName = "home" | "topic" | "agenda" | "settings" | "studio";

export interface ToolActivity extends ToolOutcomeView {
  startedAt: number;
  running: boolean;
}

export interface Toast {
  id: number;
  level: "info" | "warn" | "error" | "success";
  message: string;
}

export interface GotoRequest {
  seq: number;
  tabId: string;
  page: number | null;
  scroll: number | null;
  anchor: string | null;
  highlight: string | null;
}

// 组件注册的「把当前文档文本交出去」的回调（PDF 需要 pdf.js 的分页文本）
type SnapshotProvider = () => { content?: string; pages?: PageText[]; totalPages?: number } | null;
const snapshotProviders = new Map<string, SnapshotProvider>();

export function registerSnapshotProvider(tabId: string, fn: SnapshotProvider | null): void {
  if (fn) snapshotProviders.set(tabId, fn);
  else snapshotProviders.delete(tabId);
}

/** 排队等一轮跑完再发的那条消息（见 `AppStore.queued`）。 */
interface QueuedSend {
  text: string;
  attachments: string[];
  images: ImageUpload[];
  /** 排给哪条对话（切到别处时不串台；null = 还没定对话，就按当前对话发） */
  chatId: string | null;
  /** 等哪一轮结束（排队那一刻正在跑的那一轮） */
  afterTurnId: string | null;
  /** 排队时是否顺便打断了那一轮（Ctrl+Enter）——只影响界面文案 */
  interrupting: boolean;
}

interface AppStore {
  // --- 启动 ---
  ready: boolean;
  bootError: string | null;
  version: string;

  // --- 配置 ---
  config: PublicConfig | null;
  tools: ToolSpec[];
  permissionModes: { id: PublicConfig["agent"]["permissionMode"]; label: string; hint: string }[];
  providerKinds: { id: string; label: string; defaultBaseUrl: string; defaultModel: string }[];

  // --- 主题 ---
  topics: TopicSummary[];
  topic: TopicDetail | null;
  topicLoading: boolean;
  brief: DailyBrief | null;
  agenda: AgendaBucket[];

  // --- 对话 ---
  chatId: string | null;
  /**
   * 当前这条对话属于哪个模式：侧栏要靠它判断「这是刚开、还没落盘的工坊对话吗」。
   *
   * 光有 `view` 不够用：切视图那一刻 chatId 还是上一条（切过去要等 IPC 回来才换），
   * 拿它当工坊的待落盘对话就会闪出一行假的「新对话」。
   */
  chatMode: AgentMode;
  /**
   * 当前这条对话属于哪个主题（null = 工作区级：首页的日常问答或工坊对话）。
   *
   * 发送落库到哪、后端去哪读历史，都按**对话自己的归属**（chatMode + chatSlug）算，
   * 不按「现在停在哪个视图」算——用户可以在首页视图里接着聊一条主题对话
   * （排队自动发送，或者切走之前打的字）。以前按 view/topic 推导，点一下「首页」
   * 再发，消息就会写进工作区目录、模型丢掉全部历史、两条侧栏清单各出现同 id 的一行。
   */
  chatSlug: string | null;
  /** 每个主题的对话清单（侧栏把对话挂在对应主题下面）。键是主题 slug，首页对话用空串 */
  chatIndex: Record<string, ChatOverviewItem[]>;
  /** 工坊的对话清单。工坊没有主题，所以单独一份，键不是 slug */
  studioChats: ChatOverviewItem[];
  messages: ChatMessage[];
  /** 正在流式的那一轮。chatId 用来区分「这轮是谁的」：切到别的对话时，
   * 旧轮次的增量不能打进新对话的气泡里（后端每个事件都带 turn_id/chat_id） */
  streaming: { turnId: string; chatId: string; text: string; thinking: string } | null;
  /**
   * 排着队、等某一轮跑完才发的那条消息（Alt+Enter / Ctrl+Enter）。
   *
   * 它绑在「哪条对话的哪一轮」上：用户中途切到别的对话去，这条也不会串台——
   * 回到那条对话时再发。`interrupting` 只是给界面文案用的（「正在打断当前一轮…」）。
   */
  queued: QueuedSend | null;
  activities: ToolActivity[];
  iteration: { index: number; max: number } | null;
  /** 上一轮的用量与上下文构成；服务商不报真实用量时 cached/cacheWrite 为 0 */
  usage: {
    input: number;
    output: number;
    cached: number;
    cacheWrite: number;
    context: ContextPart[];
  } | null;
  chatError: string | null;
  approval: ApprovalRequest | null;
  /**
   * 排队中的审批（并发轮次各自弹的）：后端按 requestId 是多槽表，前端显示槽
   * 只有单值——后来者直接覆盖的话，被覆盖的那个用户永远看不到，600 秒后被
   * 后端按「拒绝」放行。连发起那一轮的 turn_id 一起记：那一轮收尾时它的审批
   * 要跟着撤（弹窗还开着也不能让人批准一个已经死掉的请求）。
   */
  pendingApprovals: { approval: ApprovalRequest; turnId: string }[];
  /**
   * 后台在跑的轮次（另一端——比如手机——发起的）：不占 streaming 槽
   * （那会顶掉用户正在看的这一轮的流式与停止键），侧栏转圈与收尾清理认它。
   */
  backgroundTurns: { turnId: string; chatId: string }[];
  /** 每跑完一轮 +1：讲解步骤面板靠它刷新（agent 会在这一轮里改方案） */
  lessonTick: number;
  /** 每次记忆变化 +1：记忆面板靠它刷新（agent 可能在对话里刚记下一条） */
  memoryTick: number;

  // --- 内置浏览器 ---
  viewer: ViewerSnapshot;
  viewerWidth: number;
  /** 侧栏宽度（分界线可拖） */
  sidebarWidth: number;
  goto: GotoRequest | null;
  reloadSeq: Record<string, number>;

  // --- UI ---
  view: ViewName;
  /** 主题视图里「对话 / 工作台」哪个在前（进主题视图恢复到上次的那半边） */
  topicTab: "chat" | "workbench";
  /** 启动恢复用的一次性值：下次进主题视图时优先用它，用完即清 */
  pendingTopicTab: "chat" | "workbench" | null;
  /** 专注/全屏模式（F11）：窗口真全屏，界面上只留当前内容（见 App.tsx 与 .app-shell.zen） */
  zen: boolean;
  /**
   * 窄屏（手机）下侧栏是抽屉：这个开关控制它展开没有。
   * 桌面上恒为 false——那边的侧栏是常显的（宽度由 sidebarWidth 决定）。
   */
  sidebarOpen: boolean;
  setSidebarOpen: (open: boolean) => void;
  paletteOpen: boolean;
  toasts: Toast[];
  inspectorOpen: boolean;
  /**
   * 盖满窗口的浮层数量（Modal / 大图 / 命令面板 / 拖放提示 / 演示卡片全屏）。
   * 原生子 WebView 浮在一切 DOM 之上，数量大于 0 时 WebTab 必须把它藏起来让路
   * （计数由 useOverlay() 维护，StrictMode 下 mount→cleanup→mount 净值不变）。
   */
  overlayCount: number;
  /** 原生网页视图当前是否盖在界面上：ToastHost 据此换到左下角，别被它压住 */
  nativeWebviewUp: boolean;

  // --- 工坊 / 外观主题 ---
  /** 工坊的信息（练习目录、内置规范清单），面板打开时拉一次 */
  studio: StudioInfo | null;
  /** 自定义主题总览（列表 + 当前生效的那份） */
  themes: ThemesOverview | null;

  // --- 动作 ---
  init: () => Promise<void>;
  /** 把当前工作区模样存进 `<工作区>/.hub/ui-state.json`（关窗前自动调） */
  saveUiState: () => Promise<void>;
  refreshTopics: () => Promise<void>;
  refreshBrief: () => Promise<void>;
  refreshAgenda: () => Promise<void>;

  setView: (v: ViewName) => void;
  /** 切换专注模式：F11 与界面上的按钮都走这里（窗口全屏与样式在同一个动作里改，避免两边不同步） */
  setZen: (on: boolean) => void;
  /** 打开主题。返回 false = 没打开成（响应被作废/打开失败）——调用方接下来
   *  基于环境态（topic/chatSlug）做的后续动作（比如 loadChat 某条对话）必须停手 */
  openTopic: (slug: string) => Promise<boolean>;
  leaveTopic: () => void;
  /** 窗口回到前台时轻量重拉当前主题的文件清单（见实现处的说明） */
  refreshTopicFiles: () => Promise<void>;
  createTopic: (name: string, description?: string, parent?: string | null) => Promise<TopicDetail | null>;
  updateTopic: (patch: {
    name?: string;
    description?: string;
    emoji?: string;
    tags?: string[];
    stage?: StudyStage;
  }) => Promise<void>;
  deleteTopic: (slug: string) => Promise<void>;
  setTopicParent: (slug: string, parent: string | null) => Promise<void>;
  setStage: (stage: StudyStage) => Promise<void>;

  newChat: () => Promise<void>;
  loadChat: (chatId: string, mode?: AgentMode) => Promise<void>;
  /** 把盘上、界面上还没有的消息补到尾部（只追加、不覆盖，见实现处的说明） */
  syncChatTail: (chatId: string) => Promise<void>;
  /** 读某个主题的对话清单（不传就是当前主题）；侧栏树展开时按需调用 */
  loadChats: (slug?: string | null, mode?: AgentMode) => Promise<void>;
  /** 重拉**已经缓存过**的对话清单（另一个窗口改了对话时用，见实现处的说明） */
  refreshChatLists: () => Promise<void>;
  /** 读工坊的对话清单 */
  loadStudioChats: () => Promise<void>;
  /** 打开工坊：切到工坊视图并续上最近一条对话 */
  openStudio: () => Promise<void>;
  /** 工坊里开一条新对话 */
  newStudioChat: () => Promise<void>;
  /** 把一条对话挪到另一个主题（侧栏拖拽整理） */
  moveChat: (chatId: string, fromSlug: string, toSlug: string) => Promise<void>;
  /** 改对话名字（空串＝恢复「第一句话」的自动标题） */
  renameChat: (chatId: string, title: string, slug?: string | null, mode?: AgentMode) => Promise<void>;
  /** 置顶 / 取消置顶 */
  pinChat: (chatId: string, pinned: boolean, slug?: string | null, mode?: AgentMode) => Promise<void>;
  /** 归档 / 取消归档（归档的收进侧栏「已归档」分组） */
  archiveChat: (chatId: string, archived: boolean, slug?: string | null, mode?: AgentMode) => Promise<void>;
  /** 从某条对话分叉出新的分支（原对话不动），返回新对话 id */
  forkChat: (chatId: string, slug?: string | null, mode?: AgentMode) => Promise<string | null>;
  /** 把一条对话移进回收站 */
  deleteChat: (chatId: string, slug?: string | null, mode?: AgentMode) => Promise<void>;
  /** 发送。返回 false = 没收下（发送在途/有轮次在跑被挡、或发送失败）——
   *  调用方该把内容放回输入框，别静默吞掉用户打的字 */
  send: (text: string, attachments?: string[], images?: ImageUpload[]) => Promise<boolean>;
  /** 排队发送（Alt+Enter：这一轮跑完再发）；`interrupt` 为真时顺便打断当前一轮（Ctrl+Enter）。
   *  当前没有在跑就直接发。返回 false = 没收下（已有一条排队时拒绝，调用方该把内容留在输入框里）。 */
  queueSend: (
    text: string,
    attachments?: string[],
    images?: ImageUpload[],
    interrupt?: boolean,
  ) => Promise<boolean>;
  /** 取消排队 */
  cancelQueued: () => void;
  /** 轮到点了就把排队的消息发出去（等的那一轮结束、或回到那条对话时调） */
  flushQueued: (endedTurnId?: string) => Promise<void>;
  /** 跟后端对一次「现在到底有没有在跑的轮次」（漏了 finished 就自愈：清掉幽灵转圈） */
  reconcileRunning: () => Promise<void>;
  stop: () => Promise<void>;
  approve: (allow: boolean, always: boolean) => Promise<void>;

  patchConfig: (patch: ConfigPatch) => Promise<void>;
  setTheme: (theme: ThemeMode) => Promise<void>;
  /** 界面缩放（设置 → 外观）。Rust 存配置并立刻应用；成功后 config 里的 uiScale 是钳制过的值 */
  setUiScale: (scale: number) => Promise<void>;
  setReasoning: (effort: ReasoningEffort, style?: ReasoningStyle) => Promise<void>;
  upsertProfile: (input: ProfileInput) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;

  // 工坊
  loadStudio: () => Promise<void>;

  // 外观主题
  loadThemes: () => Promise<void>;
  /** 切换自定义主题（null = 回到内置配色） */
  setCustomTheme: (id: string | null) => Promise<void>;
  /** 保存一份主题（新建或覆盖） */
  saveTheme: (theme: CustomTheme) => Promise<boolean>;
  deleteTheme: (id: string) => Promise<void>;

  // 内置浏览器
  openTab: (req: OpenRequest) => Promise<void>;
  closeTab: (tabId: string) => Promise<void>;
  activateTab: (tabId: string) => Promise<void>;
  toggleViewer: (show?: boolean) => Promise<void>;
  openFile: (path: string, title?: string, page?: number, topicSlug?: string) => Promise<void>;
  openUrl: (url: string) => Promise<void>;
  setViewerWidth: (w: number) => void;
  setSidebarWidth: (w: number) => void;

  // 笔记 / 资料
  createNote: (title: string) => Promise<Note | null>;
  saveNote: (path: string, content: string) => Promise<void>;
  deleteNote: (path: string) => Promise<void>;
  importMaterials: (sources: string[]) => Promise<number>;

  // 卡片 / 任务 / 会话
  cards: Card[];
  tasks: TaskWithTopic[];
  sessions: StudySession[];
  loadCards: (opts?: { dueOnly?: boolean }) => Promise<void>;
  createCard: (input: { front: string; back: string; kind?: CardKind; tags?: string[] }) => Promise<void>;
  deleteCard: (id: string) => Promise<void>;
  /** 复习打分。返回 false = 没写进去（没开主题/后端报错，如卡被删、写盘失败）——
   *  调用方不能前进队列，否则失败那张卡被静默跳过、本轮失去评分机会 */
  reviewCard: (id: string, grade: "again" | "hard" | "good" | "easy") => Promise<boolean>;
  loadTasks: () => Promise<void>;
  createTask: (input: { title: string; due?: string | null; priority?: number }) => Promise<void>;
  updateTask: (id: string, patch: { status?: string; due?: string; priority?: number; title?: string }) => Promise<void>;
  deleteTask: (id: string) => Promise<void>;
  startSession: (title: string, goals?: string[]) => Promise<void>;
  finishSession: (summary: string) => Promise<void>;

  // 其它 UI
  setPaletteOpen: (open: boolean) => void;
  toast: (level: Toast["level"], message: string) => void;
  dismissToast: (id: number) => void;
  setInspectorOpen: (open: boolean) => void;
  /** 浮层计数 +1 / -1（useOverlay 在 mount/unmount 时调；不会减成负数） */
  bumpOverlay: (delta: 1 | -1) => void;
  setNativeWebviewUp: (up: boolean) => void;
}

let toastSeq = 1;
let gotoSeq = 1;
/** 当前显示的审批浮层属于哪一轮（收尾时按它撤浮层；并发轮次互不误伤） */
let approvalTurn: string | null = null;

/**
 * 初始化只允许跑一次。
 *
 * React 严格模式会把 effect 执行两遍（挂载 → 卸载 → 再挂载），
 * 如果这里不做保护，`listen()` 会注册两套监听，之后每个事件都被处理两次
 * ——症状是回复内容、工具卡片整齐地翻倍。
 */
let initPromise: Promise<void> | null = null;
/** send() 的同步互斥锁：从「点击发送」到 streaming 置位之间的空窗里挡住第二次发送 */
let sendInFlight = false;
/** 对话切换序号：先点的对话请求晚到时，不许覆盖已经切过去的新对话 */
let chatSwitchSeq = 0;
/** 主题打开序号：快速连点两个主题时，旧主题的响应不许糊回界面 */
let topicOpenSeq = 0;

export const useApp = create<AppStore>((set, get) => ({
  ready: false,
  bootError: null,
  version: "",

  config: null,
  tools: [],
  permissionModes: [],
  providerKinds: [],

  topics: [],
  topic: null,
  topicLoading: false,
  brief: null,
  agenda: [],

  chatId: null,
  chatMode: "study",
  chatSlug: null,
  chatIndex: {},
  studioChats: [],
  messages: [],
  streaming: null,
  queued: null,
  activities: [],
  iteration: null,
  usage: null,
  chatError: null,
  approval: null,
  pendingApprovals: [],
  backgroundTurns: [],
  lessonTick: 0,
  memoryTick: 0,

  viewer: { tabs: [], activeId: null, visible: false },
  viewerWidth: 460,
  sidebarWidth: (() => {
    const saved = Number(localStorage.getItem("hub.sidebarWidth"));
    return saved >= 200 && saved <= 420 ? saved : 248;
  })(),
  goto: null,
  reloadSeq: {},

  view: "home",
  topicTab: "chat",
  pendingTopicTab: null,
  zen: false,
  sidebarOpen: false,
  paletteOpen: false,
  toasts: [],
  inspectorOpen: false,
  overlayCount: 0,
  nativeWebviewUp: false,

  studio: null,
  themes: null,

  cards: [],
  tasks: [],
  sessions: [],

  // ============================================================ 启动

  async init() {
    if (!initPromise) initPromise = bootstrapStore(set, get);
    return initPromise;
  },

  async saveUiState() {
    const s = get();
    const snapshot = {
      savedAt: new Date().toISOString(),
      view: s.view,
      topicSlug: s.topic?.slug ?? null,
      chatId: s.chatId,
      topicTab: s.topicTab,
      sidebarWidth: s.sidebarWidth,
      viewerWidth: s.viewerWidth,
      zen: s.zen,
      // 浏览器标签原样交给 Rust（启动时只重建壳，内容前端挂载时自己补）
      viewer: s.viewer,
    };
    try {
      await api.uiStateSave(JSON.stringify(snapshot));
    } catch (e) {
      console.warn("保存界面快照失败", e);
    }
  },

  async refreshTopics() {    try {
      const topics = await api.topicList();
      set({ topics });
      const cur = get().topic;
      if (cur) {
        const still = topics.find((t) => t.slug === cur.slug);
        if (!still) set({ topic: null, view: "home" });
      }
    } catch (e) {
      console.warn("刷新主题失败", e);
    }
  },

  async refreshBrief() {
    try {
      set({ brief: await api.dailyBrief() });
    } catch (e) {
      console.warn("刷新日报失败", e);
    }
  },

  async refreshAgenda() {
    try {
      set({ agenda: await api.agenda(14) });
    } catch (e) {
      console.warn("刷新日程失败", e);
    }
  },

  // ============================================================ 主题

  setView(v) {
    // 手机上不给进「设置」和「工坊」：那两页整个都是「改配置 / 造技能」，
    // 远程接口一条都不放行（见 remote/bridge.rs 的白名单）。让用户点进去
    // 看一页全是点不动的开关，不如直接说清为什么。
    if (isRemote() && (v === "settings" || v === "studio")) {
      get().toast("info", "设置与工坊只能在电脑上改（远程是拿来用的）");
      return;
    }
    // 离开主题视图的导航（日程/设置）也要作废在途的 openTopic：
    // 晚到的主题响应会把 view 强行拉回 topic（同 leaveTopic 里的说明）
    if (v !== "topic") {
      topicOpenSeq++;
      void api.viewerSetVisible(get().viewer.visible);
    }
    set({ view: v });
  },

  setZen(on) {
    set({ zen: on });
    // 窗口几何在 applyZenWindow 里自己管（见那里的说明：多显示器时系统的全屏会挑错屏）。
    // 失败只记一行：样式那边已经切好了，功能不会因此断掉。
    void applyZenWindow(on);
  },

  setSidebarOpen(open) {
    set({ sidebarOpen: open });
  },

  async openTopic(slug) {
    const seq = ++topicOpenSeq;
    set({ topicLoading: true });
    try {
      const detail = await api.topicOpen(slug);
      // 期间用户又点了别的主题（或点了首页/工坊/日程——那些也会作废在途的
      // openTopic）：这次响应作废，别把旧主题的数据糊上去。返回 false 让
      // 调用方知道环境态已经不属于这个主题，别接着 loadChat 旧主题的对话
      //（不然 chatSlug 记成作废时的环境主题，发送会写错目录）。
      if (seq !== topicOpenSeq) return false;
      const chats = detail.chats ?? [];
      set({
        topic: detail,
        view: "topic",
        chatMode: "study",
        topicLoading: false,
        sessions: detail.sessions,
        cards: [],
        tasks: [],
        activities: [],
        // **不动 streaming**：那一轮可能就是在这条对话里跑着的。清了它，
        // 界面上就再也看不到「正在跑」（侧栏不转圈、输入框没有停止键）——
        // 用户报的「工坊里没有停止对话的按键」就是这么来的。
        usage: null,
      });
      // 优先续上最近一次对话，没有就新开
      if (chats.length > 0) await get().loadChat(chats[0]);
      else await get().newChat();
      void get().loadChats(detail.slug);
      void get().loadCards();
      void get().loadTasks();
      return true;
    } catch (e) {
      set({ topicLoading: false });
      get().toast("error", errText(e));
      return false;
    }
  },

  /**
   * 只把主题「文件层面」的信息重拉一遍（资料清单 / 笔记 / 统计），**不动当前对话与流式状态**。
   *
   * 用在窗口重新获得焦点时：用户可能在资源管理器里往 `materials/` 丢了新文件
   * （比如刚录完的课堂语音转文字 .txt），那时前端不知道，资料面板要等到重开主题才更新。
   * 不能直接调 `openTopic——它会清空 streaming、把对话切回最近一条。
   */
  async refreshTopicFiles() {
    const cur = get().topic;
    if (!cur) return;
    try {
      const detail = await api.topicOpen(cur.slug);
      // 期间可能换了主题：那就别把旧主题的数据糊回去
      if (get().topic?.slug !== cur.slug) return;
      set({ topic: detail, sessions: detail.sessions });
    } catch {
      // 刷新失败不打扰用户：下次动作还会再拉
    }
  },

  leaveTopic() {
    // 同上：切视图不该把「哪条对话在跑」这件事抹掉（streaming 原样留着）。
    // 但 chatId/归属要跟着清：首页是「新起一条」的地方，留着旧对话的 id 的话，
    // 在这里发消息会把内容追加进一条看不见历史的对话（它属于刚才那个主题）。
    // 序号 +1 把还在路上的 loadChat 响应作废，别让它把刚清掉的 chatId 又写回来。
    chatSwitchSeq++;
    // 姐妹路径：在途的 openTopic 也得作废（它的守卫用的是 topicOpenSeq）。
    // 不作废的话，「点开主题 → 立刻点首页」会让晚到的 topicOpen 响应把界面
    // 强行拉回主题视图、把刚清掉的对话身份又写回去——beta.3 修了 loadChat
    // 这条路，漏了 openTopic 这条。
    topicOpenSeq++;
    set({
      view: "home",
      topic: null,
      chatId: null,
      chatMode: "study",
      chatSlug: null,
      messages: [],
      activities: [],
      usage: null,
    });
    void api.viewerSetVisible(false);
    void get().loadChats(null);
  },

  async createTopic(name, description, parent) {
    try {
      const detail = await api.topicCreate(name, description, undefined, parent ?? null);
      await get().refreshTopics();
      get().toast(
        "success",
        parent ? `已在「${parent}」下创建子主题「${detail.meta.name}」` : `已创建主题「${detail.meta.name}」`,
      );
      return detail;
    } catch (e) {
      get().toast("error", errText(e));
      return null;
    }
  },

  async updateTopic(patch) {
    const cur = get().topic;
    if (!cur) return;
    try {
      const detail = await api.topicUpdate(cur.slug, patch);
      set({ topic: detail });
      await get().refreshTopics();
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async deleteTopic(slug) {
    try {
      await api.topicDelete(slug);
      await get().refreshTopics();
      if (get().topic?.slug === slug) set({ topic: null, view: "home" });
      get().toast("info", "主题已移入工作区的 .hub/trash（可手动恢复）");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async setTopicParent(slug, parent) {
    try {
      const detail = await api.topicSetParent(slug, parent);
      await get().refreshTopics();
      // 改的是当前主题时，详情也要跟着更新（面包屑、继承资料都会变）
      if (get().topic?.slug === slug) set({ topic: detail });
      get().toast("info", parent ? `已移到「${parent}」下面` : "已移出父主题");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async setStage(stage) {
    const cur = get().topic;
    if (!cur) return;
    try {
      const detail = await api.topicUpdate(cur.slug, { stage });
      set({ topic: detail });
      get().toast("info", `已切换到「${stage}」阶段`);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  // ============================================================ 对话

  async newChat() {
    const seq = ++chatSwitchSeq;
    const chatId = await api.agentNewChat();
    // 期间用户已经切到别的对话：这条空对话别抢回来
    if (seq !== chatSwitchSeq) return;
    set({
      chatId,
      chatMode: "study",
      // 新对话属于「此刻打开的主题」：首页/工坊进来时 topic 为空 = 工作区级
      chatSlug: get().topic?.slug ?? null,
      messages: [],
      activities: [],
      chatError: null,
      usage: null,
    });
    void get().loadChats();
  },

  async loadChat(chatId, mode) {
    const seq = ++chatSwitchSeq;
    const slug = get().topic?.slug ?? null;
    // 没传就沿用当前模式：打开主题/工坊时那条模式已经在切视图那一刻设好了，
    // 恢复界面快照那条路也是先 openTopic/openStudio 再 loadChat
    const m = mode ?? get().chatMode;
    // 对话自己的归属跟着这次载入走（发送落库按它算，不按环境视图算）
    const owner: string | null = m === "studio" ? null : slug;
    try {
      const messages = await api.agentTranscript(chatId, slug);
      // 先点的对话响应晚到：不能覆盖已经切过去的新对话
      //（否则会出现「标题是 B、正文是 A」的混搭，接着聊还会写错文件）
      if (seq !== chatSwitchSeq) return;
      set({ chatId, chatMode: m, chatSlug: owner, messages, activities: [], chatError: null, usage: null });
      // 回到「排着队等这一轮结束」的那条对话时，把那条消息发出去
      void get().flushQueued();
    } catch (e) {
      if (seq !== chatSwitchSeq) return;
      set({ chatId, chatMode: m, chatSlug: owner, messages: [], usage: null });
      console.warn("读取对话失败", e);
    }
  },

  /**
   * 把「盘上有、界面上还没有」的消息补到尾部。**只追加，不覆盖**。
   *
   * 用途：一轮结束时的兜底（见 `handleAgentEvent` 的 finished 分支）。之所以不直接
   * `loadChat` 重读一遍：万一用户紧接着又发了一条，重读会把新一轮刚追加的消息覆盖掉
   * （`loadChat` 认的是「切对话」的竞态序号，不认新一轮）。只追加就天然没有这个竞态。
   */
  async syncChatTail(chatId) {
    const slug = get().topic?.slug ?? null;
    try {
      const onDisk = await api.agentTranscript(chatId, slug);
      if (get().chatId !== chatId) return; // 期间切走了：别把补的内容塞进别的对话
      set((s) => {
        if (s.chatId !== chatId || s.messages.length === 0) return {};
        // 从「界面上最后一条」开始对：它后面的都要补。对不上就什么都不做（保守）
        const last = s.messages[s.messages.length - 1];
        const at = onDisk.findIndex((m) => m.id === last.id);
        if (at < 0) return {};
        const tail = onDisk.slice(at + 1);
        if (tail.length === 0) return {};
        return { messages: [...s.messages, ...tail] };
      });
    } catch (e) {
      console.warn("补对话尾巴失败", e);
    }
  },

  /** 刷新侧栏的对话清单（换主题、开新对话、聊完一轮之后都要刷） */
  async loadChats(slug, mode) {
    const m: AgentMode = mode ?? (get().view === "studio" ? "studio" : "study");
    // 工坊的对话不是「某个主题的」：它有自己的那份清单
    if (m === "studio") return get().loadStudioChats();
    const target = slug === undefined ? (get().topic?.slug ?? null) : slug;
    try {
      // 带上归档的：它们要显示在「已归档」分组里，前端自己分拣
      const items = await api.chatOverview(target, true, m);
      set((s) => ({ chatIndex: { ...s.chatIndex, [target ?? ""]: items } }));
    } catch (e) {
      console.warn("读取对话清单失败", e);
    }
  },

  /**
   * 重拉**已经缓存过**的对话清单。
   *
   * `chatIndex` 是按需缓存的：侧栏展开某个主题才读一次，读过就不再读第二次
   * （见 Sidebar 里那个 lazy effect）。所以**另一个窗口**（手机上打开的同一个界面、
   * 或另一个桌面窗口）删掉 / 搬走 / 改名一条对话时，这边的列表会一直停在旧内容上
   * ——用户报的「移动端网页会显示已经删除的对话」就是它。
   *
   * 只在收到 `hub://topics` 的 Refresh 时调用（那几个 chat_* 命令改完都会发）。
   * 只重拉缓存过的那些，不顺手把所有主题都扫一遍。
   */
  async refreshChatLists() {
    const st = get();
    const jobs = Object.keys(st.chatIndex).map((key) =>
      // 空串那个键是「工作区级」那份（首页/日常问答）；主题清单一律 study 模式，
      // 不显式传的话工坊视图下会被 loadChats 当成 studio 转发掉
      st.loadChats(key === "" ? null : key, "study"),
    );
    // 工坊那份是单独的字段，拉过就跟着刷
    if (st.studioChats.length > 0 || st.view === "studio") jobs.push(st.loadStudioChats());
    await Promise.all(jobs);
  },

  async loadStudioChats() {
    try {
      const items = await api.chatOverview(null, true, "studio");
      set({ studioChats: items });
    } catch (e) {
      console.warn("读取工坊对话清单失败", e);
    }
  },

  /**
   * 打开工坊：切到工坊视图、清掉当前主题（工坊里没有主题这回事），
   * 并续上最近一条工坊对话——和 `openTopic` 的行为对齐。
   */
  async openStudio() {
    // 工坊是「坐在电脑前造东西」的场景（写技能文件、改 MCP 定义都只在本地放行）
    if (isRemote()) {
      get().toast("info", "工坊只能在电脑上开");
      return;
    }
    // 进工坊也不清 streaming：正在跑的工坊对话要保持「有停止键」的状态。
    // 同时作废在途的 openTopic（同 leaveTopic 里的说明）：不然晚到的主题响应
    // 会把 view 拉回 topic，工坊刚设好的 chatMode 也被覆盖
    topicOpenSeq++;
    set({
      view: "studio",
      topic: null,
      chatMode: "studio",
      chatSlug: null,
      cards: [],
      tasks: [],
      activities: [],
      chatError: null,
      usage: null,
    });
    void get().loadStudioChats();
    void get().loadStudio();
    try {
      const items = await api.chatOverview(null, false, "studio");
      // 等清单的窗口里用户可能已经点了别的主题 / 回了首页：这时候不该再切对话，
      // 否则工坊的 chatId 会骑到主题视图上（下一次发送把工坊对话的后续写进主题目录）
      if (get().view !== "studio") return;
      if (items.length > 0) await get().loadChat(items[0].id);
      else await get().newStudioChat();
    } catch (e) {
      console.warn("打开工坊失败", e);
    }
  },

  async newStudioChat() {
    const seq = ++chatSwitchSeq;
    const chatId = await api.agentNewChat();
    if (seq !== chatSwitchSeq) return;
    set({ chatId, chatMode: "studio", chatSlug: null, messages: [], activities: [], chatError: null, usage: null });
    void get().loadStudioChats();
  },

  async loadStudio() {
    try {
      set({ studio: await api.studioInfo() });
    } catch (e) {
      console.warn("读取工坊信息失败", e);
    }
  },

  // 改名 / 置顶 / 归档都由后端算好新清单，前端直接替换——
  // 排序规则（置顶在前、归档沉底）只在 Rust 里写一份，别在界面上再排一遍
  async renameChat(chatId, title, slug, mode) {
    const t = chatTarget(get, slug, mode);
    try {
      const items = await api.chatRename(chatId, title, t.slug, t.mode);
      putChatList(set, t.mode, t.slug, items);
      get().toast("success", title.trim() ? "对话已改名" : "已恢复自动标题");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async pinChat(chatId, pinned, slug, mode) {
    const t = chatTarget(get, slug, mode);
    try {
      const items = await api.chatPin(chatId, pinned, t.slug, t.mode);
      putChatList(set, t.mode, t.slug, items);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async archiveChat(chatId, archived, slug, mode) {
    const t = chatTarget(get, slug, mode);
    try {
      const items = await api.chatArchive(chatId, archived, t.slug, t.mode);
      putChatList(set, t.mode, t.slug, items);
      get().toast("info", archived ? "已归档，可在「已归档」里找回来" : "已取消归档");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async forkChat(chatId, slug, mode) {
    const t = chatTarget(get, slug, mode);
    try {
      const newId = await api.chatFork(chatId, t.slug, null, t.mode);
      if (t.mode === "studio") void get().loadStudioChats();
      else await get().loadChats(t.slug);
      get().toast("success", "已分叉出一条新对话（原对话没动）");
      return newId;
    } catch (e) {
      get().toast("error", errText(e));
      return null;
    }
  },

  async deleteChat(chatId, slug, mode) {
    const t = chatTarget(get, slug, mode);
    try {
      await api.chatDelete(chatId, t.slug);
      await get().refreshTopics();
      if (t.mode === "studio") void get().loadStudioChats();
      else await get().loadChats(t.slug);
      // 正在看的就是被删的那条：换个空对话，免得接着聊又写回被删的文件
      if (get().chatId === chatId) {
        if (t.mode === "studio") await get().newStudioChat();
        else await get().newChat();
      }
      get().toast("info", "对话已移入回收站");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async moveChat(chatId, fromSlug, toSlug) {
    try {
      await api.chatMove(chatId, fromSlug, toSlug);
      await get().refreshTopics();
      // 快照要在 refreshTopics **之后**取：那是一次 IPC，窗口里用户可能已经
      // loadChat 切到别的对话——用旧快照判断会误触 newChat，把用户刚点开的
      // 对话顶成一条空白新对话（deleteChat 用的是实时 get().chatId，同理）
      const st = get();
      void st.loadChats(fromSlug);
      void st.loadChats(toSlug);
      // 正在看的这条被搬走了：源主题里已经没有它，新开一条，避免接着聊又写回源主题
      if (st.topic?.slug === fromSlug && st.chatId === chatId) {
        await st.newChat();
        void get().loadChats(fromSlug);
      }
      get().toast("success", "对话已移到目标主题");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async send(text, attachments, images) {
    const t = text.trim();
    const pics = images ?? [];
    // 只贴图不打字是合法的（「这张图里的第三行是什么」可以省略）
    if (!t && pics.length === 0) return false;
    // 双发竞态：streaming 要等 turn_started 事件回来才置上，快速连按两次
    // 回车会双双穿过守卫、在同一对话里并发跑两个轮次。sendInFlight 在
    // invoke 返回前就同步置位，把窗口关死。两个挡回都带提示并返回 false——
    // 静默吞字违背「别把用户打的字弄丢」，调用方（Composer）会把内容放回输入框。
    if (sendInFlight) {
      get().toast("info", "上一条还在发送中，等一下再按");
      return false;
    }
    if (get().streaming) {
      get().toast("info", "上一轮还在进行中，等它跑完再发");
      return false;
    }
    sendInFlight = true;
    // 发到哪按**这条对话自己的归属**算（chatMode/chatSlug 随 loadChat/newChat 走），
    // 不看「现在停在哪个视图」：用户点首页后再发，或者排队消息在切走后自动发出，
    // 都必须落回对话本来的地方——按视图推导会把消息写进工作区目录、
    // 模型丢掉全部历史、侧栏出现同 id 的两行（踩过的坑，别改回去）。
    const studio = get().chatMode === "studio";
    const slug = studio ? null : get().chatSlug;
    // 阶段字段同理：只有环境主题就是这条对话的主题时才带上，不一致就交给后端按主题自己的 meta 取
    const topic = get().topic;
    const stage = studio ? null : topic && topic.slug === slug ? topic.meta.stage : null;
    let chatId = get().chatId;
    try {
      if (!chatId) {
        // 懒建也是一次 IPC：期间用户可能已经点开别的对话/回了首页（loadChat/
        // leaveTopic 都会推进切换序号或改写 chatId）。先记序号，回来发现被人
        // 推进了就**不把新对话的身份写回界面**——写回会让后面的 moved 判断
        // 失真（消息串进别人对话的视图）。消息照发：落库在新对话里，切回去能读到。
        // 正常路径（没切走）照旧写回，界面继续这条新对话。
        const seq = ++chatSwitchSeq;
        chatId = await api.agentNewChat();
        if (seq === chatSwitchSeq && get().chatId === null) {
          set({ chatId, chatSlug: slug });
        }
      }
      const cid: string = chatId;
      const res = await api.agentSend({
        chatId,
        text: t,
        topicSlug: slug,
        stage,
        attachments: attachments ?? [],
        images: pics,
        mode: studio ? "studio" : "study",
      });
      set((s) => {
        // await 期间用户可能已经切走（切对话 / 回首页 / 进工坊——远程桥一个往返
        // 就有几百毫秒）：消息与对话归属都**不能**再写进当前界面。消息已落库到
        // 它自己的对话，切回去 loadChat 会读到；归属写了会把「当前对话属于谁」
        // 污染成发起时那条，下一次发送就会写进错误主题目录下的同名 jsonl
        // （与 syncChatTail / 事件路径的「期间切走」守卫是同一竞态）。
        // streaming 记录的是这一轮自己的归属（chatId = cid），照常设置，
        // turn_started 事件来了也会对齐同一个值。
        const moved = s.chatId !== cid;
        return {
          ...(moved
            ? {}
            : {
                messages: [...s.messages, res.message],
                activities: [],
                chatError: null,
                chatSlug: slug,
                chatMode: studio ? ("studio" as const) : ("study" as const),
              }),
          // 乐观占位：不等 turn_started 事件，输入框立刻进入「停止」态，
          // 也让后续事件的 turn_id 守卫有东西可比
          streaming: { turnId: res.turnId, chatId: cid, text: "", thinking: "" },
        };
      });
      // moved（期间切走）也算成功：消息已落库到它自己的对话、轮次在跑，
      // flushQueued 要据此清队列，不能靠界面消息列表有没有变长来判断
      return true;
    } catch (e) {
      // 期间切走了：错误别写进新对话的 chatError（那是另一条对话界面的状态），toast 提一句就够
      if (get().chatId === chatId) set({ chatError: errText(e) });
      get().toast("error", errText(e));
      return false;
    } finally {
      sendInFlight = false;
    }
  },

  async stop() {
    const s = get().streaming;
    if (!s) return;
    try {
      await api.agentCancel(s.turnId);
    } catch (e) {
      console.warn(e);
    }
  },

  /**
   * 排队发送。两个入口都用它：
   * - **Alt+Enter**：`interrupt=false` —— 让这一轮跑完（当前工具循环结束）再发；
   * - **Ctrl+Enter**：`interrupt=true` —— 先把这一轮打断，它一收尾就发。
   *
   * 打断不是「立刻硬塞进去」：中止那一轮要给还没执行完的工具调用补上
   * 「已被用户中止」的 tool_result 再落库（tool_use/tool_result 必须成对，
   * 否则这条对话之后每次请求都会被服务商 400）。所以这里只负责打断 + 排队，
   * 真正的发送交给 `flushQueued`，在那一轮的 finished/failed 事件里触发——
   * 那时落库已经收尾，顺序天然是对的（中止标记在前、新消息在后）。
   *
   * 返回 false = 没收下。唯一会拒收的情形：槽里已经有一条排队的。排队内容在
   * 输入框清空之后就只剩这一个副本（没落库、没发送），直接覆盖等于把用户
   * 打的字无声丢掉——所以拒收并提示，让调用方把内容留在输入框里。
   */
  async queueSend(text, attachments, images, interrupt) {
    const t = text.trim();
    const pics = images ?? [];
    if (!t && pics.length === 0) return false;
    const st = get();
    // 没在跑：跟普通发送一样（Ctrl+Enter 也就等于发送）。
    // send 被挡回/失败时如实返回 false——Composer 收到 false 会把内容留在输入框
    if (!st.streaming) {
      return await st.send(t, attachments, pics);
    }
    if (st.queued) {
      get().toast("info", "已有一条排队的消息（输入框下面那条），等它发出后再排；点凭证条上的 × 可以取消它");
      return false;
    }
    set({
      queued: {
        text: t,
        attachments: attachments ?? [],
        images: pics,
        chatId: st.streaming.chatId,
        afterTurnId: st.streaming.turnId,
        interrupting: !!interrupt,
      },
    });
    if (interrupt) await st.stop();
    return true;
  },

  cancelQueued() {
    set({ queued: null });
  },

  /**
   * 跟后端对一次「现在到底有没有在跑的轮次」。
   *
   * `streaming` 是前端自己维护的：漏掉一个 `finished`（断线、丢事件）它就会一直挂着——
   * 侧栏那条对话永远转圈、输入框一直是停止键，而其实早跑完了。窗口回前台、手机重连时
   * 各对一次；后端说没有就清掉。
   */
  async reconcileRunning() {
    const cur = get().streaming;
    const bg = get().backgroundTurns;
    // backgroundTurns 也要对账：它是「前端自维护」的名单，手机息屏/切网丢掉
    // finished 事件后幽灵条目会让那条对话在侧栏永久转圈（streaming 最初就是
    // 因为这个才有对账）。后端 agent_running 返回的是**全部**在跑轮次，
    // 对两边是同一个响应。
    if (!cur && bg.length === 0) return;
    try {
      const running = await api.agentRunning();
      const ids = new Set(running.map((r) => r.turnId));
      set((s) => ({
        // 对账是自愈不是权威：后端说没有的才清；后端说在跑但本实例没见过
        // TurnStarted 的**不认领**（那是另一端的轮次，这里没有它的流式缓冲）
        streaming: s.streaming && !ids.has(s.streaming.turnId) ? null : s.streaming,
        backgroundTurns: s.backgroundTurns.filter((t) => ids.has(t.turnId)),
      }));
    } catch {
      /* 对不上就算了，别打扰用户 */
    }
  },

  /**
   * 把排队的消息发出去。三个调用点：那一轮 finished / failed、以及回到那条对话时
   * （`loadChat`）。**发出去了才清队列**——被「上一轮还在进行中」挡回来、或者发送
   * 报错时，排队内容留着，别把用户打的字弄丢。
   */
  async flushQueued(endedTurnId) {
    const q = get().queued;
    if (!q) return;
    // 不是在等这一轮：先看「等的那一轮」还跑不跑。它可能早已收尾（被下面的
    // 守卫挡回来之后，任何后续轮次的收尾都不会再带它的 turn_id）——那之后
    // 再拿 afterTurnId 当闸的话，队列就永久滞留了（凭证条一直承诺「这一轮结束
    // 后自动发送」却永不发送，只能靠再切一次对话解锁）。还在跑就继续等，
    // 已经不在了就落到下面的通用条件（切走/在跑的守卫继续兜底）。
    if (endedTurnId && q.afterTurnId && q.afterTurnId !== endedTurnId) {
      const waiting = get().streaming;
      if (waiting && waiting.turnId === q.afterTurnId) return;
    }
    // 用户切到别的对话去了：留着，回到那条对话时再发（loadChat 会再调一次）
    if (q.chatId && q.chatId !== get().chatId) return;
    // 它那条对话又有一轮在跑（比如用户手动发了）：再等等
    const running = get().streaming;
    if (running && (!q.chatId || running.chatId === q.chatId)) return;
    // 按 send 的「收下了没有」清队列：发出去（含期间切走——消息已落库到
    // 原对话、轮次在跑）才清，被挡回或失败都留着。以前用「messages.length
    // 有没有变长」判断，切走窗口里那是另一条对话的长度：发送成功却留着
    // （回到原对话自动重发一份）、发送失败反而清掉（排队内容无声丢失）。
    const ok = await get().send(q.text, q.attachments, q.images);
    if (ok) set({ queued: null });
  },

  async approve(allow, always) {
    const pending = get().approval;
    if (!pending) return;
    // 出队下一条排队中的审批接着显示（并发轮次各自的申请依次过目）
    const queue = [...get().pendingApprovals];
    const next = queue.shift() ?? null;
    approvalTurn = next?.turnId ?? null;
    set({ approval: next?.approval ?? null, pendingApprovals: queue });
    const requestId = pending.kind === "tool" ? pending.call.requestId : pending.request.requestId;
    try {
      await api.agentApprove(requestId, allow, always);
      if (pending.kind === "sandbox" && allow) {
        get().toast("info", `已允许访问该目录，可在「设置 → Agent」里撤销`);
      }
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  // ============================================================ 配置

  async setReasoning(effort, style) {
    const id = get().config?.activeProfileId;
    if (!id) return;
    try {
      const config = await api.profileSetReasoning(id, effort, style);
      set({ config });
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async setTheme(theme) {
    // 自定义主题自带底色：让它继续生效的话，切「明亮/深色」看起来像没反应。
    // 所以这里明确退回内置配色，并在提示里说清「原来的主题还在，可以再选回来」。
    const custom = get().config?.appearance?.customTheme ?? null;
    if (custom) {
      const name = get().themes?.applied?.name ?? custom;
      await get().patchConfig({ theme });
      await get().setCustomTheme(null);
      get().toast("info", `已切回内置配色（自定义主题「${name}」仍可在设置里选回来）`);
      return;
    }
    applyTheme(theme, null);
    await get().patchConfig({ theme });
  },

  async setUiScale(scale) {
    try {
      const config = await api.uiSetScale(scale);
      set({ config });
      const applied = Math.round((config.appearance.uiScale ?? 1) * 100);
      get().toast("success", applied === 100 ? "界面缩放已恢复 100%" : `界面缩放已调整为 ${applied}%`);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async loadThemes() {
    try {
      set({ themes: await api.themesOverview() });
    } catch (e) {
      console.warn("读取外观主题失败", e);
    }
  },

  async setCustomTheme(id) {
    try {
      const config = await api.themeSetActive(id);
      set({ config });
      await get().loadThemes();
      const applied = get().themes?.applied ?? null;
      applyTheme(config.appearance?.theme ?? "system", applied);
      if (applied) get().toast("success", `已启用主题「${applied.name}」`);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async saveTheme(theme) {
    try {
      set({ themes: await api.themeSave(theme) });
      return true;
    } catch (e) {
      get().toast("error", errText(e));
      return false;
    }
  },

  async deleteTheme(id) {
    try {
      set({ themes: await api.themeDelete(id) });
      // 删掉的正好是当前生效的那份：后端已经把配置清了，这里把配置与变量一起更新，
      // 界面立刻回到内置配色（不这么做会出现「配置里还记着一个不存在的主题」）
      const config = await api.configGet();
      set({ config });
      applyTheme(config.appearance?.theme ?? "system", get().themes?.applied ?? null);
      get().toast("info", "主题已删除");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async patchConfig(patch) {
    const before = get().config?.appearance?.customTheme ?? null;
    try {
      const config = await api.configPatch(patch);
      set({ config });
      // 主题 id 变过（例如在工作区之间切换）→ 变量要重新拉一次
      if ((config.appearance?.customTheme ?? null) !== before) await get().loadThemes();
      applyTheme(config.appearance?.theme ?? "system", get().themes?.applied ?? null);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async upsertProfile(input) {
    try {
      const config = await api.profileUpsert(input);
      set({ config });
      get().toast("success", "模型档案已保存");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async deleteProfile(id) {
    try {
      const config = await api.profileDelete(id);
      set({ config });
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  // ============================================================ 内置浏览器

  async openTab(req) {
    try {
      await api.viewerOpen(req);
      set({ viewer: await api.viewerSnapshot() });
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async closeTab(tabId) {
    try {
      await api.viewerClose(tabId);
      set({ viewer: await api.viewerSnapshot() });
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async activateTab(tabId) {
    try {
      await api.viewerActivate(tabId);
      set({ viewer: await api.viewerSnapshot() });
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async toggleViewer(show) {
    const visible = show ?? !get().viewer.visible;
    try {
      await api.viewerSetVisible(visible);
      set({ viewer: await api.viewerSnapshot() });
      if (visible && get().viewer.tabs.length === 0) {
        await api.viewerOpenHome();
        set({ viewer: await api.viewerSnapshot() });
      }
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async openFile(path, title, page, topicSlug) {
    const { topics, topic } = get();
    // 引用可能写成「主题名/materials/x.pdf」——继承来的资料在提示词里就是这种写法，
    // 认出前缀就切到那个主题打开，否则按当前主题的相对路径解析
    const [head, ...rest] = path.split("/");
    const owner = rest.length > 0 ? topics.find((t) => t.slug === head) : undefined;
    const slug = topicSlug ?? owner?.slug ?? topic?.slug;
    if (!slug) {
      get().toast("warn", "先打开一个主题，才能查看里面的文件");
      return;
    }
    const rel = owner ? rest.join("/") : path;
    await get().openTab({ path: rel, topicSlug: slug, title: title ?? null, page: page ?? null });
  },

  async openUrl(url) {
    await get().openTab({ url });
  },

  setViewerWidth(w) {
    // 拖分界线时给第二栏（对话/工作台）留出最小可用宽度：面板宁可先停住，
    // 也不能把中间挤成一条竖排字（用户报的「拉过来第二栏排版错乱」）。
    // 第二栏的左沿就是侧栏宽度（.app 里侧栏之后是净宽为 0 的 resizer）。
    const MIN_CENTER = 400;
    const max = Math.max(320, window.innerWidth - get().sidebarWidth - MIN_CENTER);
    set({ viewerWidth: Math.max(320, Math.min(1200, max, w)) });
  },

  setSidebarWidth(w) {
    const width = Math.max(200, Math.min(420, Math.round(w)));
    set({ sidebarWidth: width });
    try {
      localStorage.setItem("hub.sidebarWidth", String(width));
    } catch {
      /* 隐私模式下写不了，不影响使用 */
    }
  },

  // ============================================================ 笔记 / 资料

  async createNote(title) {
    const slug = get().topic?.slug;
    if (!slug) return null;
    try {
      const note = await api.noteCreate(slug, title);
      await refreshTopicDetail(set, get);
      return note;
    } catch (e) {
      get().toast("error", errText(e));
      return null;
    }
  },

  async saveNote(path, content) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      await api.noteSave(slug, path, content);
      await refreshTopicDetail(set, get);
      get().toast("success", "已保存");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async deleteNote(path) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      await api.noteDelete(slug, path);
      await refreshTopicDetail(set, get);
      get().toast("info", "笔记已移入回收站");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async importMaterials(sources) {
    const slug = get().topic?.slug;
    if (!slug) return 0;
    try {
      const res = await api.materialImport(slug, sources);
      await refreshTopicDetail(set, get);
      if (res.skipped.length > 0) {
        get().toast("warn", `导入 ${res.imported.length} 个，跳过 ${res.skipped.length} 个`);
      } else {
        get().toast("success", `已导入 ${res.imported.length} 个资料到 ${res.dir}/`);
      }
      return res.imported.length;
    } catch (e) {
      get().toast("error", errText(e));
      return 0;
    }
  },

  // ============================================================ 卡片 / 任务 / 会话

  async loadCards(opts) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      set({ cards: await api.cardList(slug, { dueOnly: opts?.dueOnly }) });
    } catch (e) {
      console.warn(e);
    }
  },

  async createCard(input) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      await api.cardCreate(slug, input);
      await get().loadCards();
      await refreshTopicDetail(set, get);
      await get().refreshBrief();
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async deleteCard(id) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      await api.cardDelete(slug, [id]);
      await get().loadCards();
      await refreshTopicDetail(set, get);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async reviewCard(id, grade) {
    const slug = get().topic?.slug;
    if (!slug) return false;
    const before = get().cards.find((c) => c.id === id);
    const wasDue = before ? new Date(before.srs.due) <= new Date() : false;
    try {
      const card = await api.cardReview(slug, id, grade);
      const stillDue = new Date(card.srs.due) <= new Date();
      set((s) => ({
        cards: s.cards.map((c) => (c.id === id ? card : c)),
        // 顺手把「待复习」计数改掉，省一次全量刷新
        topic:
          s.topic && wasDue && !stillDue
            ? { ...s.topic, stats: { ...s.topic.stats, cardsDue: Math.max(0, s.topic.stats.cardsDue - 1) } }
            : s.topic,
      }));
      return true;
    } catch (e) {
      get().toast("error", errText(e));
      return false;
    }
  },

  async loadTasks() {
    try {
      set({ tasks: await api.taskList({ includeDone: true }) });
    } catch (e) {
      console.warn(e);
    }
  },

  async createTask(input) {
    const slug = get().topic?.slug;
    if (!slug) {
      get().toast("warn", "先在某个主题下建任务");
      return;
    }
    try {
      await api.taskCreate(slug, {
        title: input.title,
        due: input.due ?? null,
        priority: input.priority ?? 2,
      });
      await get().loadTasks();
      await get().refreshAgenda();
      await refreshTopicDetail(set, get);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async updateTask(id, patch) {
    const t = get().tasks.find((x) => x.task.id === id);
    if (!t) return;
    try {
      await api.taskUpdate(t.topicSlug, id, patch);
      await get().loadTasks();
      await get().refreshAgenda();
      await get().refreshBrief();
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async deleteTask(id) {
    const t = get().tasks.find((x) => x.task.id === id);
    if (!t) return;
    try {
      await api.taskDelete(t.topicSlug, id);
      await get().loadTasks();
      await get().refreshAgenda();
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async startSession(title, goals) {
    const slug = get().topic?.slug;
    if (!slug) return;
    try {
      const s = await api.sessionStart(slug, title, {
        stage: get().topic?.meta.stage,
        goals,
        chatId: get().chatId ?? undefined,
      });
      set({ sessions: [s, ...get().sessions] });
      get().toast("success", `已开始【${s.stage}】会话：${s.title}`);
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  async finishSession(summary) {
    try {
      const s = await api.sessionFinish(summary);
      set({ sessions: [s, ...get().sessions.filter((x) => x.id !== s.id)] });
      get().toast("success", "会话已保存");
    } catch (e) {
      get().toast("error", errText(e));
    }
  },

  // ============================================================ UI

  setPaletteOpen(open) {
    set({ paletteOpen: open });
  },

  toast(level, message) {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, level, message }] }));
    setTimeout(() => get().dismissToast(id), level === "error" ? 8000 : 4000);
  },

  dismissToast(id) {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },

  setInspectorOpen(open) {
    set({ inspectorOpen: open });
  },

  bumpOverlay(delta) {
    set((s) => ({ overlayCount: Math.max(0, s.overlayCount + delta) }));
  },

  setNativeWebviewUp(up) {
    // 值没变就不动 state，避免无谓的重渲染（ensure/bounds 成功会反复调）
    set((s) => (s.nativeWebviewUp === up ? {} : { nativeWebviewUp: up }));
  },
}));

// ============================================================ 对话清单的去处

type StoreSet = (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void;

/**
 * 一条对话属于哪份清单。
 *
 * 工坊的对话**没有主题**（它和首页的日常问答共用工作区级的 chats 目录，
 * 后端靠侧车文件里的 mode 分开），所以工坊那份要用单独的 state 字段装，
 * 不能混进按 slug 索引的 `chatIndex` 里。
 */
function chatTarget(
  get: () => AppStore,
  slug?: string | null,
  mode?: AgentMode,
): { slug: string | null; mode: AgentMode } {
  const m: AgentMode = mode ?? (get().view === "studio" ? "studio" : "study");
  if (m === "studio") return { slug: null, mode: m };
  return { slug: slug === undefined ? (get().topic?.slug ?? null) : slug, mode: m };
}

/** 后端返回的新清单放回正确的位置。 */
function putChatList(set: StoreSet, mode: AgentMode, slug: string | null, items: ChatOverviewItem[]): void {
  if (mode === "studio") set({ studioChats: items });
  else set((s) => ({ chatIndex: { ...s.chatIndex, [slug ?? ""]: items } }));
}

// ============================================================ 主题

/**
 * 主题落成 `html[data-theme]`：
 * - light / dark：直接写死，用户要明亮就给他明亮（哪怕系统是深色）
 * - system：跟随系统，并在系统切换时实时跟随
 *
 * 内置编辑器的样式也认这个属性，所以一处生效、全局一致。
 */
let systemThemeListener: ((e: MediaQueryListEvent) => void) | null = null;

/** 上一次应用过的自定义主题变量名（换主题时要把它们撤掉，否则会一直叠着）。 */
let appliedThemeVars: string[] = [];

function applyTheme(theme: ThemeMode, custom?: CustomTheme | null): void {
  const root = document.documentElement;
  const mq = window.matchMedia("(prefers-color-scheme: dark)");

  if (systemThemeListener) {
    mq.removeEventListener("change", systemThemeListener);
    systemThemeListener = null;
  }

  // 自定义主题自带底色（base）：它没覆盖的变量跟随内置的那一套，
  // 所以这里把跟随系统的逻辑让位给主题自己的选择。
  const effective: ThemeMode = custom ? custom.base : theme;

  const set = (mode: "light" | "dark") => {
    root.dataset.theme = mode;
    // InkNote 编辑器认这个属性来切 markdown 主题
    if (!root.dataset.mdTheme) root.dataset.mdTheme = "github";
  };

  if (effective === "system") {
    set(mq.matches ? "dark" : "light");
    systemThemeListener = (e) => set(e.matches ? "dark" : "light");
    mq.addEventListener("change", systemThemeListener);
  } else {
    set(effective);
  }

  // 变量覆盖：用 style.setProperty 逐个设，不注入样式表——
  // 变量名写错了只影响那一条，界面不会变成一片空白。
  for (const name of appliedThemeVars) root.style.removeProperty(name);
  appliedThemeVars = [];
  if (!custom) return;
  for (const [name, value] of Object.entries(custom.vars ?? {})) {
    root.style.setProperty(name, value);
    appliedThemeVars.push(name);
  }
}

// ============================================================ 启动装配

/** 真正干活的启动流程（由 init 保护成只跑一次）。 */
async function bootstrapStore(
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
): Promise<void> {
  // 先按系统主题铺一层，避免启动瞬间闪一下白（配置要等 bootstrap 回来才有）
  applyTheme(get().config?.appearance?.theme ?? "system", null);

  // 把 set/get 交给上面的防抖刷新用
  windowSet = set;
  windowGet = get;
  try {
    const boot = await api.bootstrap();
    set({
      ready: true,
      version: boot.version,
      config: boot.config,
      topics: boot.topics,
      tools: boot.tools,
      permissionModes: boot.permissionModes as AppStore["permissionModes"],
      providerKinds: boot.providerKinds,
      chatId: await api.agentNewChat(),
      viewer: await api.viewerSnapshot(),
    });
    // 配置到手后再按用户存的主题铺一次：上面那次拿不到 config，
    // 少了这一句「明亮模式」冷启动会被系统深色盖掉（看上去像设置没保存）。
    // 自定义主题的变量也在这里铺：先拉列表，再按用户选的那份设上去。
    await get().loadThemes();
    applyTheme(boot.config.appearance?.theme ?? "system", get().themes?.applied ?? null);
    void get().loadStudio();

    // 事件订阅（生命周期与窗口一致）
    await listen<AgentEvent>("hub://agent", (e) => handleAgentEvent(e.payload, set, get));
    await listen<ViewerEvent>("hub://viewer", (e) => handleViewerEvent(e.payload, set));
    // 主题集合变化时：侧栏列表要刷，**同时**如果变化的是当前主题，
    // 工作台里的笔记/资料/统计也要跟着刷——否则 agent 新建的笔记在界面上看不到。
    await listen<TopicsEvent>("hub://topics", (e) => {
      void get().refreshTopics();
      // 对话的增删改也走这个事件（chat_delete / chat_move / chat_rename … 改完都会发）：
      // 对话清单是「读过就缓存」的，不在这里跟着重拉，另一个窗口就会一直显示
      // 已经删掉的对话（用户报的「移动端网页会显示已经删除的对话」）。
      void get().refreshChatLists();
      const cur = get().topic?.slug;
      if (!cur) return;
      const affected =
        e.payload.kind === "refresh" ||
        (e.payload.kind === "created" && e.payload.slug === cur) ||
        (e.payload.kind === "updated" && e.payload.slug === cur);
      if (affected) scheduleTopicRefresh();
    });
    await listen<ToastEvent>("hub://toast", (e) => {
      get().toast(e.payload.level, e.payload.message);
    });
    // 记忆变化（agent 用 memory_write 记下/删掉，或用户在面板里改过）：
    // 这里只推一个计数，界面按需重拉——记忆面板关着时不必付重扫的成本
    await listen<MemoryEvent>("hub://memory", () => {
      set((s) => ({ memoryTick: s.memoryTick + 1 }));
    });

    // 替手机执行命令（桌面端这一侧；手机上不需要——它自己就是发请求的那头）
    if (!isRemote()) void registerRemoteInvoker();

    // 「工作区模样」的持久化（视图/主题/对话/浏览器标签/宽度/zen），下次启动原样恢复。
    //
    // 为什么不只在关窗时存：JS 的 onCloseRequested 与窗口销毁是竞态——Rust 侧
    // 不会等 JS 处理完再关，实测会整份丢失。改成**状态变化后防抖落盘**，
    // 关窗钩子只是兜底（多存一次不吃亏）。快照就几 KB，写入走原子替换。
    //
    // 远程模式下这两件事都不做：手机上不该覆盖电脑的界面快照（接口层也拒了），
    // 关窗钩子更没有意义——手机上的「窗口」是浏览器标签页。
    if (!isRemote()) {
      const uiSaveFields = (s: AppStore) =>
        [s.view, s.topic?.slug, s.chatId, s.topicTab, s.sidebarWidth, s.viewerWidth, s.zen, s.viewer] as const;
      let lastUiSaveKey = "";
      let uiSaveTimer: ReturnType<typeof setTimeout> | null = null;
      useApp.subscribe((s) => {
        const key = JSON.stringify(uiSaveFields(s));
        if (key === lastUiSaveKey) return;
        lastUiSaveKey = key;
        if (uiSaveTimer) clearTimeout(uiSaveTimer);
        // 先等主题详情（openTopic 的多次 set）稳定下来再存，避免存到中间态
        uiSaveTimer = setTimeout(() => void get().saveUiState(), 900);
      });
      const unlistenClose = await getCurrentWindow().onCloseRequested(async () => {
        if (uiSaveTimer) clearTimeout(uiSaveTimer);
        await get().saveUiState();
      });
      void unlistenClose; // 窗口生命周期 = 应用生命周期，不退订
    } else {
      // 手机息屏 / 切网之后 SSE 会自己重连，但**中间那几帧是真的丢了**
      // （可能正好是一轮回复的结尾）。重连时重拉一次当前状态，
      // 免得界面停在一个再也不更新的半截上。
      await listen("hub://reconnect", () => {
        void get().refreshTopics();
        void get().refreshBrief();
        // 断线期间另一个窗口改过的对话（删掉/搬走）也要跟着对齐
        void get().refreshChatLists();
        // 断线期间结束的那一轮：前端这边的 streaming 得清掉，否则一直转圈
        void get().reconcileRunning();
        scheduleTopicRefresh();
        const id = get().chatId;
        if (id && !get().streaming) void get().loadChat(id);
      });

      // 手机上点了侧栏里的东西就把抽屉收起来。放在这里（而不是每个点击处）：
      // 「换了主题 / 换了对话 / 换了视图」是导航的收口点，侧栏里每一个入口
      // 最后都会落到这几个字段上——一处管住，将来加新入口也不会漏。
      useApp.subscribe((s, prev) => {
        if (!s.sidebarOpen) return;
        if (s.view !== prev.view || s.chatId !== prev.chatId || s.topic?.slug !== prev.topic?.slug) {
          useApp.setState({ sidebarOpen: false });
        }
      });
    }

    void get().refreshBrief();
    void get().refreshAgenda();

    // 回到上次关闭时的模样。快照优先于 last_topic（后者只是浏览器标签恢复
    // 之外的兜底：快照缺失/损坏时至少还回得到上次的主题）。
    //
    // 手机上**只取「看到哪」这一半**（视图 / 主题 / 对话 / 半区），
    // 宽度和 zen 都是窗口几何，跟着电脑上那份走反而会让手机上的界面变形。
    let restored = false;
    try {
      const text = await api.uiStateGet();
      const ui = text ? (JSON.parse(text) as SavedUiState) : null;
      if (ui) {
        if (!isRemote()) {
          if (typeof ui.sidebarWidth === "number" && ui.sidebarWidth >= 200 && ui.sidebarWidth <= 420) {
            set({ sidebarWidth: ui.sidebarWidth });
            localStorage.setItem("hub.sidebarWidth", String(ui.sidebarWidth));
          }
          if (typeof ui.viewerWidth === "number" && ui.viewerWidth >= 280) {
            // 走分界线拖动同一套钳制（给中间对话/工作台留 400px、上限 1200）：
            // 窗口尺寸不持久化（tauri.conf 固定 1480×940），上次把浏览器拖宽后
            // 关窗，这次原样恢复会把中间栏挤到几十像素、.viewer 不收缩还把
            // 对话区整个顶出窗口（overflow: clip 连滚动条都不给）。侧栏宽度的
            // 恢复有完整上下限校验，这里漏了是疏忽不是设计。
            get().setViewerWidth(ui.viewerWidth);
          }
        }
        if (ui.topicTab === "workbench" || ui.topicTab === "chat") {
          set({ pendingTopicTab: ui.topicTab });
        }
      }
      if (ui?.view === "topic" && ui.topicSlug && boot.topics.some((t) => t.slug === ui.topicSlug)) {
        const opened = await get().openTopic(ui.topicSlug);
        // 没打开成（磁盘 IO 失败 / 恢复期间用户点了别的主题被作废）就停手：
        // 此刻 topic 为 null 或已是别的主题，接着 loadChat 快照对话会把
        // chatSlug 记错——发送落进工作区级或错误主题目录下的同名 jsonl
        //（与 Sidebar.openChat 的守卫同一族，启动恢复这条路别漏）
        if (opened && ui.chatId && ui.chatId !== get().chatId) {
          await get().loadChat(ui.chatId);
        }
        restored = opened;
      } else if (ui?.view === "studio") {
        await get().openStudio();
        if (ui.chatId && ui.chatId !== get().chatId) {
          await get().loadChat(ui.chatId);
        }
        restored = true;
      } else if (ui?.view === "home") {
        set({ view: "home" });
        if (ui.chatId) await get().loadChat(ui.chatId);
        restored = true;
      }
      // 关在专注模式里就原样回专注模式（几何由 applyZenWindow 自己摆）
      if (ui?.zen && !isRemote()) {
        set({ zen: true });
        void applyZenWindow(true);
      }
    } catch (e) {
      console.warn("恢复界面快照失败，按全新启动处理", e);
    }
    if (!restored) {
      const last = boot.config.lastTopic;
      if (last && boot.topics.some((t) => t.slug === last)) {
        void get().openTopic(last);
      }
    }
  } catch (e) {
    set({ bootError: errText(e), ready: false });
  }
}

/** ui-state.json 的形状（Rust 侧只取 viewer 字段，这里全量用） */
interface SavedUiState {
  savedAt?: string;
  view?: ViewName;
  topicSlug?: string | null;
  chatId?: string | null;
  topicTab?: "chat" | "workbench";
  sidebarWidth?: number;
  viewerWidth?: number;
  zen?: boolean;
}

// ============================================================ 专注模式的窗口几何

/** 进全屏前的窗口几何（退出时还原用） */
let zenGeometry: { x: number; y: number; w: number; h: number } | null = null;

/**
 * 专注模式（F11）的窗口几何：进全屏、退出还原。
 *
 * 为什么不直接 setFullscreen(true)：**多显示器时它会挑错屏**——Windows 自己决定
 * 「哪块屏算全屏的目标」，窗口摆到旁边那块屏上就会突出去一部分（用户报过：
 * 一块屏上全屏，右边那部分跑到另一块屏上）。所以：
 *
 * 1. 按**窗口当前所在的那块屏**全屏（setFullscreenOnMonitor，Tauri 2.12 起有）；
 * 2. 再显式对齐一次位置与尺寸——个别情况下系统只铺满「工作区」，任务栏那条还露在外面；
 * 3. 退出时把进来之前的几何摆回去，免得还原到别处或大小漂移。
 *
 * 另外 TitleBar 的「贴合工作区」逻辑在全屏期间必须让路（见那里的守卫），
 * 否则它会在系统刚铺好全屏之后把窗口缩成工作区大小。
 */
async function applyZenWindow(on: boolean): Promise<void> {
  // 远程（手机浏览器）里没有「窗口」这回事：样式上的 zen 照常生效，
  // 几何一律不动——`getCurrentWindow()` 那边根本没有可操作的窗口。
  if (isRemote()) return;
  const win = getCurrentWindow();
  try {
    if (!on) {
      await win.setFullscreen(false);
      const back = zenGeometry;
      // 退出时按进来之前记下的外框几何摆回去（fitOuter 内部换算成内容区尺寸）
      if (back) await fitOuter(win, back.x, back.y, back.w, back.h);
      return;
    }

    // 几何一律按**外框**记（fitOuter 内部会换算成 setSize 要的内容区尺寸）
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    zenGeometry = { x: pos.x, y: pos.y, w: size.width, h: size.height };

    const mon = (await currentMonitor()) ?? (await primaryMonitor());
    if (!mon) {
      await win.setFullscreen(true);
      return;
    }
    await win.setFullscreenOnMonitor(mon.position);
    await fitOuter(win, mon.position.x, mon.position.y, mon.size.width, mon.size.height);
  } catch (e) {
    // 权限漏了（capabilities）、或者系统拒绝：退回最简单的全屏，界面样式不受影响
    console.warn("切换全屏失败", e);
    try {
      await win.setFullscreen(on);
    } catch {
      /* 连这个都不行就只能当它没有 */
    }
  }
}

// ============================================================ 事件处理

/** 这轮事件是不是「当前正在看的这一轮」。
 *
 * 后端每个事件都带 turn_id，但光比 turn_id 还不够：轮次进行中切到同主题的
 * 另一条对话（loadChat 不清 streaming，轮次还在后台跑），turn_id 依然相等，
 * 旧对话的消息就会打进新对话的消息列表。所以还要比 chatId。 */
function isCurrentTurn(s: AppStore, turnId: string): boolean {
  return s.streaming?.turnId === turnId && s.streaming.chatId === s.chatId;
}

/** 最近一轮是「哪条对话的哪一轮」：`finished` 靠它判断该不该去盘上补一次尾巴
 *  （`Finished` 事件只有 turn_id，没有 chat_id）。 */
let lastTurn: { turnId: string; chatId: string } | null = null;

function handleAgentEvent(
  ev: AgentEvent,
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
): void {
  switch (ev.kind) {
    case "turn_started":
      // 并发轮次（另一端——比如手机——发起的）不许顶掉正在看的这一轮：
      // 顶掉的后果是流式气泡、活动卡片、停止按钮全部消失，正在跑的那轮既
      // 看不到输出也点不了停止（AGENTS.md：切视图都不许清 streaming，这里
      // 更不能拿别人的 turn_started 来清）。新轮记进后台名单：侧栏照样转圈，
      // 它收尾时按 turn_id 摘掉。没在跑的、或同对话的新一轮照常接管。
      set((s) => {
        if (s.streaming && s.streaming.chatId !== ev.chat_id) {
          if (s.backgroundTurns.some((t) => t.turnId === ev.turn_id)) return {};
          return {
            backgroundTurns: [...s.backgroundTurns, { turnId: ev.turn_id, chatId: ev.chat_id }],
          };
        }
        return {
          streaming: { turnId: ev.turn_id, chatId: ev.chat_id, text: "", thinking: "" },
          activities: [],
          iteration: null,
          chatError: null,
        };
      });
      lastTurn = { turnId: ev.turn_id, chatId: ev.chat_id };
      break;
    case "delta":
      set((s) => {
        if (!isCurrentTurn(s, ev.turn_id)) return {};
        return {
          streaming: ev.thinking
            ? { ...s.streaming!, thinking: s.streaming!.thinking + ev.text }
            : { ...s.streaming!, text: s.streaming!.text + ev.text },
        };
      });
      break;
    case "message":
      set((s) => {
        // 只认「当前正在看的就是这条对话」的消息（事件带 chat_id）。别的对话的消息
        // 不追加进来——它们落在各自的 jsonl 里，切回去时 loadChat 会从盘上读。
        //
        // 这里**不能**再看 `streaming` 还在不在：用户中途切走一下（切对话/切主题都会
        // 把 streaming 清掉）之后，这一轮剩下的回复就整条进不了界面了——模型生成了、
        // jsonl 里也写了、下一轮它自己还引用得到，界面上却永远是断的
        //（用户报的「模型认为它输出了讲解和提问，我却没读到」）。判据改成 chat_id。
        if (ev.chat_id !== s.chatId) return {};
        // 重连/重试会把同一条消息再推一次：按 id 去重，别渲染两遍
        if (s.messages.some((m) => m.id === ev.message.id)) return {};
        return {
          messages: [...s.messages, ev.message],
          // 一轮里模型可能回复多次（工具调用之间），每次都从空缓冲重新开始
          streaming: s.streaming ? { ...s.streaming, text: "", thinking: "" } : null,
        };
      });
      break;
    case "stream_reset":
      // 自动重连：断线前吐的半截话擦掉重来，重连后的增量才能接上正确的前文
      set((s) => (isCurrentTurn(s, ev.turn_id) ? { streaming: { ...s.streaming!, text: "", thinking: "" } } : {}));
      break;
    case "iteration":
      set((s) => (isCurrentTurn(s, ev.turn_id) ? { iteration: { index: ev.index, max: ev.max } } : {}));
      break;
    case "tool_approval":
      // 审批是全局浮层：哪怕用户切到了别的对话，后台那轮也得等这个决定。
      // 但它不是「后来者顶掉先来者」：并发轮次各自的审批在这里排队依次显示
      // （后端按 requestId 本来就是多槽表），显示槽空着才直接上。
      set((s) => {
        const approval: ApprovalRequest = { kind: "tool", call: ev.call };
        if (s.approval) {
          return { pendingApprovals: [...s.pendingApprovals, { approval, turnId: ev.turn_id }] };
        }
        approvalTurn = ev.turn_id;
        return { approval };
      });
      break;
    case "sandbox_request": {
      // 同 tool_approval：显示槽被占就排队（并发轮次各自的申请依次过目）
      set((s) => {
        const approval: ApprovalRequest = {
          kind: "sandbox",
          request: {
            requestId: ev.request_id,
            path: ev.path,
            root: ev.root,
            mode: ev.mode,
            reason: ev.reason,
            preApproved: ev.pre_approved,
          },
        };
        if (s.approval) {
          return { pendingApprovals: [...s.pendingApprovals, { approval, turnId: ev.turn_id }] };
        }
        approvalTurn = ev.turn_id;
        return { approval };
      });
      break;
    }
    case "tool_started":
      set((s) => {
        if (!isCurrentTurn(s, ev.turn_id)) return {};
        return {
          activities: [
            ...s.activities,
            {
              callId: ev.call_id,
              name: ev.name,
              summary: ev.summary,
              risk: ev.risk,
              ok: true,
              preview: "",
              durationMs: 0,
              denied: false,
              startedAt: Date.now(),
              running: true,
            },
          ],
        };
      });
      break;
    case "tool_finished":
      set((s) => {
        if (!isCurrentTurn(s, ev.turn_id)) return {};
        const exists = s.activities.some((a) => a.callId === ev.outcome.callId);
        const next = exists
          ? s.activities.map((a) =>
              a.callId === ev.outcome.callId ? { ...a, ...ev.outcome, running: false } : a,
            )
          : [
              ...s.activities,
              { ...ev.outcome, startedAt: Date.now(), running: false },
            ];
        return { activities: next };
      });
      break;
    case "usage":
      set((s) =>
        isCurrentTurn(s, ev.turn_id)
          ? {
              usage: {
                input: ev.input_tokens,
                output: ev.output_tokens,
                cached: ev.cached_tokens ?? 0,
                cacheWrite: ev.cache_write_tokens ?? 0,
                context: ev.context ?? [],
              },
            }
          : {},
      );
      break;
    case "finished":
      set((s) => {
        // 后台轮次收尾：从名单里摘掉
        const backgroundTurns = s.backgroundTurns.filter((t) => t.turnId !== ev.turn_id);
        // 迟到的旧轮次收尾不许把新一轮的 streaming 清掉
        if (s.streaming && s.streaming.turnId !== ev.turn_id) {
          return { backgroundTurns, lessonTick: s.lessonTick + 1 };
        }
        // 审批只撤「属于这一轮」的（显示中的与排队中的都算——那一轮已经收尾，
        // 它排着的审批再显示出来就是让人批准一个已经死掉的请求）；显示槽被撤
        // 后把队列里还活着的第一条顶上来，不然它永远上不了屏
        const alive = s.pendingApprovals.filter((p) => p.turnId !== ev.turn_id);
        const clearApproval = approvalTurn === ev.turn_id;
        const next = clearApproval ? (alive.shift() ?? null) : null;
        approvalTurn = next?.turnId ?? (clearApproval ? null : approvalTurn);
        return {
          streaming: null,
          iteration: null,
          approval: next?.approval ?? (clearApproval ? null : s.approval),
          pendingApprovals: alive,
          backgroundTurns,
          lessonTick: s.lessonTick + 1,
        };
      });
      void get().refreshTopics();
      void get().refreshBrief();
      // 侧栏的对话清单：标题取自第一句话，所以第一轮结束后要刷一次才会出现
      void get().loadChats();
      // 兜底：这一轮如果就是当前这条对话的，结束后跟盘上对一次尾巴——
      // 中途切走 / 掉过事件时丢掉的回复都能补回来（只追加不覆盖，见 syncChatTail）
      if (lastTurn?.turnId === ev.turn_id && lastTurn.chatId === get().chatId) {
        void get().syncChatTail(lastTurn.chatId);
      }
      // 有排在队里的消息（Alt+Enter / Ctrl+Enter）：这一轮收尾了，该它上了
      void get().flushQueued(ev.turn_id);
      break;
    case "failed":
      set((s) => {
        const backgroundTurns = s.backgroundTurns.filter((t) => t.turnId !== ev.turn_id);
        // 迟到的旧轮次失败不许清掉新一轮的 streaming，错误也只属于它的对话
        if (s.streaming && s.streaming.turnId !== ev.turn_id) return { backgroundTurns };
        // 与 finished 同一套：撤这一轮的排队审批、顶起还活着的第一条
        const alive = s.pendingApprovals.filter((p) => p.turnId !== ev.turn_id);
        const clearApproval = approvalTurn === ev.turn_id;
        const next = clearApproval ? (alive.shift() ?? null) : null;
        approvalTurn = next?.turnId ?? (clearApproval ? null : approvalTurn);
        return {
          streaming: null,
          iteration: null,
          approval: next?.approval ?? (clearApproval ? null : s.approval),
          pendingApprovals: alive,
          backgroundTurns,
          chatError: ev.message,
        };
      });
      get().toast("error", ev.message);
      // 失败也一样：排队的消息别一直卡着
      void get().flushQueued(ev.turn_id);
      break;
  }
}

function handleViewerEvent(
  ev: ViewerEvent,
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
): void {
  switch (ev.kind) {
    case "sync":
      set({ viewer: ev.snapshot });
      break;
    case "updated":
      set((s) => ({
        viewer: {
          ...s.viewer,
          tabs: s.viewer.tabs.map((t) => (t.id === ev.tab.id ? ev.tab : t)),
        },
      }));
      break;
    case "snapshot_request": {
      const provider = snapshotProviders.get(ev.tab_id);
      const data = provider?.() ?? null;
      if (data) {
        void api
          .viewerReportSnapshot(ev.tab_id, {
            content: data.content,
            pages: data.pages,
            totalPages: data.totalPages,
          })
          .catch((e) => console.warn("上报快照失败", e));
      } else {
        // 前端拿不到（例如 PDF 还没加载完），让后端自己想办法
        void api.viewerReportSnapshot(ev.tab_id, { error: "前端无法提供内容快照" }).catch(() => {});
      }
      break;
    }
    case "render_request": {
      // agent 要看某一页的图（pdf_screenshot）：渲染完必须回一次话，
      // 无论成功还是失败——不回的话工具那边会一直等到超时。
      void renderPdfPage(ev.tab_id, ev.page, ev.scale)
        .then((r) => api.viewerReportRender(ev.request_id, { data: r.data, width: r.width, height: r.height }))
        .catch((e) =>
          api.viewerReportRender(ev.request_id, { error: errText(e) }).catch(() => {}),
        );
      break;
    }
    case "goto":
      set({
        goto: {
          seq: gotoSeq++,
          tabId: ev.tab_id,
          page: ev.page,
          scroll: ev.scroll,
          anchor: ev.anchor,
          highlight: ev.highlight,
        },
      });
      break;
    case "reload":
      set((s) => ({ reloadSeq: { ...s.reloadSeq, [ev.tab_id]: (s.reloadSeq[ev.tab_id] ?? 0) + 1 } }));
      break;
  }
}

/** 同一个主题的连续刷新请求合并成一次，避免 agent 写文件时反复全量扫描 */
let topicRefreshTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleTopicRefresh() {
  if (topicRefreshTimer) clearTimeout(topicRefreshTimer);
  topicRefreshTimer = setTimeout(() => {
    topicRefreshTimer = null;
    if (windowSet && windowGet) void refreshTopicDetail(windowSet, windowGet);
  }, 260);
}
// 由 bootstrapStore 注入，避免模块间循环依赖
let windowSet: ((p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void) | null = null;
let windowGet: (() => AppStore) | null = null;

// 开发模式下把 store 挂到 window，方便用 scripts/cdp.mjs 直接读状态
// （`node scripts/cdp.mjs eval "window.__hub.getState().topic.slug"`）。
// 只在 dev 构建里存在，打包版没有这个入口。
if (import.meta.env.DEV) {
  (window as unknown as { __hub?: typeof useApp }).__hub = useApp;
}

async function refreshTopicDetail(
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
): Promise<void> {
  const cur = get().topic;
  if (!cur) return;
  try {
    const detail = await api.topicGet(cur.slug);
    set({ topic: detail, sessions: detail.sessions });
  } catch (e) {
    console.warn("刷新主题详情失败", e);
  }
}

/** 供组件使用的小选择器 */
export const selectActiveTab = (s: AppStore): TabView | null => {
  const id = s.viewer.activeId;
  if (!id) return null;
  return s.viewer.tabs.find((t) => t.id === id) ?? null;
};

export type { NoteSummary, PlanTask, ChatMessage, TopicSummary };
