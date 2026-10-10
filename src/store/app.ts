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
  /** 每个主题的对话清单（侧栏把对话挂在对应主题下面）。键是主题 slug，首页对话用空串 */
  chatIndex: Record<string, ChatOverviewItem[]>;
  /** 工坊的对话清单。工坊没有主题，所以单独一份，键不是 slug */
  studioChats: ChatOverviewItem[];
  messages: ChatMessage[];
  streaming: { turnId: string; text: string; thinking: string } | null;
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
  /** 专注/全屏模式（F11）：窗口真全屏，界面上只留当前内容（见 App.tsx 与 .app-shell.zen） */
  zen: boolean;
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
  refreshTopics: () => Promise<void>;
  refreshBrief: () => Promise<void>;
  refreshAgenda: () => Promise<void>;

  setView: (v: ViewName) => void;
  /** 切换专注模式：F11 与界面上的按钮都走这里（窗口全屏与样式在同一个动作里改，避免两边不同步） */
  setZen: (on: boolean) => void;
  openTopic: (slug: string) => Promise<void>;
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
  loadChat: (chatId: string) => Promise<void>;
  /** 读某个主题的对话清单（不传就是当前主题）；侧栏树展开时按需调用 */
  loadChats: (slug?: string | null, mode?: AgentMode) => Promise<void>;
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
  send: (text: string, attachments?: string[], images?: ImageUpload[]) => Promise<void>;
  stop: () => Promise<void>;
  approve: (allow: boolean, always: boolean) => Promise<void>;

  patchConfig: (patch: ConfigPatch) => Promise<void>;
  setTheme: (theme: ThemeMode) => Promise<void>;
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
  reviewCard: (id: string, grade: "again" | "hard" | "good" | "easy") => Promise<void>;
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

/**
 * 初始化只允许跑一次。
 *
 * React 严格模式会把 effect 执行两遍（挂载 → 卸载 → 再挂载），
 * 如果这里不做保护，`listen()` 会注册两套监听，之后每个事件都被处理两次
 * ——症状是回复内容、工具卡片整齐地翻倍。
 */
let initPromise: Promise<void> | null = null;

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
  chatIndex: {},
  studioChats: [],
  messages: [],
  streaming: null,
  activities: [],
  iteration: null,
  usage: null,
  chatError: null,
  approval: null,
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
  zen: false,
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

  async refreshTopics() {
    try {
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
    set({ view: v });
    if (v !== "topic") {
      void api.viewerSetVisible(get().viewer.visible);
    }
  },

  setZen(on) {
    set({ zen: on });
    // 窗口几何在 applyZenWindow 里自己管（见那里的说明：多显示器时系统的全屏会挑错屏）。
    // 失败只记一行：样式那边已经切好了，功能不会因此断掉。
    void applyZenWindow(on);
  },

  async openTopic(slug) {
    set({ topicLoading: true });
    try {
      const detail = await api.topicOpen(slug);
      const chats = detail.chats ?? [];
      set({
        topic: detail,
        view: "topic",
        topicLoading: false,
        sessions: detail.sessions,
        cards: [],
        tasks: [],
        activities: [],
        streaming: null,
      });
      // 优先续上最近一次对话，没有就新开
      if (chats.length > 0) await get().loadChat(chats[0]);
      else await get().newChat();
      void get().loadChats(detail.slug);
      void get().loadCards();
      void get().loadTasks();
    } catch (e) {
      set({ topicLoading: false });
      get().toast("error", errText(e));
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
    set({ view: "home", topic: null, messages: [], activities: [], streaming: null });
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
    const chatId = await api.agentNewChat();
    set({ chatId, messages: [], activities: [], streaming: null, chatError: null, usage: null });
    void get().loadChats();
  },

  async loadChat(chatId) {
    const slug = get().topic?.slug ?? null;
    try {
      const messages = await api.agentTranscript(chatId, slug);
      set({ chatId, messages, activities: [], chatError: null });
    } catch (e) {
      set({ chatId, messages: [] });
      console.warn("读取对话失败", e);
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
    set({ view: "studio", topic: null, cards: [], tasks: [], activities: [], streaming: null, chatError: null });
    void get().loadStudioChats();
    void get().loadStudio();
    try {
      const items = await api.chatOverview(null, false, "studio");
      if (items.length > 0) await get().loadChat(items[0].id);
      else await get().newStudioChat();
    } catch (e) {
      console.warn("打开工坊失败", e);
    }
  },

  async newStudioChat() {
    const chatId = await api.agentNewChat();
    set({ chatId, messages: [], activities: [], streaming: null, chatError: null, usage: null });
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
      const st = get();
      await st.refreshTopics();
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
    if ((!t && pics.length === 0) || get().streaming) return;
    let chatId = get().chatId;
    if (!chatId) {
      chatId = await api.agentNewChat();
      set({ chatId });
    }
    // 在哪个视图里说话就发给哪个模式：工坊里没有主题，
    // 传 topicSlug 会让后端把对话写进主题目录（那就跑到学习那边去了）
    const studio = get().view === "studio";
    const slug = studio ? null : (get().topic?.slug ?? null);
    try {
      const res = await api.agentSend({
        chatId,
        text: t,
        topicSlug: slug,
        stage: studio ? null : (get().topic?.meta.stage ?? null),
        attachments: attachments ?? [],
        images: pics,
        mode: studio ? "studio" : "study",
      });
      set((s) => ({
        messages: [...s.messages, res.message],
        chatError: null,
        activities: [],
      }));
    } catch (e) {
      set({ chatError: errText(e) });
      get().toast("error", errText(e));
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

  async approve(allow, always) {
    const pending = get().approval;
    if (!pending) return;
    set({ approval: null });
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
    set({ viewerWidth: Math.max(320, Math.min(1200, w)) });
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
    if (!slug) return;
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
    } catch (e) {
      get().toast("error", errText(e));
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

    void get().refreshBrief();
    void get().refreshAgenda();

    // 回到上次打开的主题
    const last = boot.config.lastTopic;
    if (last && boot.topics.some((t) => t.slug === last)) {
      void get().openTopic(last);
    }
  } catch (e) {
    set({ bootError: errText(e), ready: false });
  }
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

function handleAgentEvent(
  ev: AgentEvent,
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
): void {
  switch (ev.kind) {
    case "turn_started":
      set({ streaming: { turnId: ev.turn_id, text: "", thinking: "" }, activities: [], iteration: null, chatError: null });
      break;
    case "delta":
      set((s) => {
        if (!s.streaming) return {};
        return {
          streaming: ev.thinking
            ? { ...s.streaming, thinking: s.streaming.thinking + ev.text }
            : { ...s.streaming, text: s.streaming.text + ev.text },
        };
      });
      break;
    case "message":
      set((s) => ({
        messages: [...s.messages, ev.message],
        // 一轮里模型可能回复多次（工具调用之间），每次都从空缓冲重新开始
        streaming: s.streaming ? { ...s.streaming, text: "", thinking: "" } : null,
      }));
      break;
    case "iteration":
      set({ iteration: { index: ev.index, max: ev.max } });
      break;
    case "tool_approval":
      set({ approval: { kind: "tool", call: ev.call } });
      break;
    case "sandbox_request":
      set({
        approval: {
          kind: "sandbox",
          request: {
            requestId: ev.request_id,
            path: ev.path,
            root: ev.root,
            mode: ev.mode,
            reason: ev.reason,
            preApproved: ev.pre_approved,
          },
        },
      });
      break;
    case "tool_started":
      set((s) => ({
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
      }));
      break;
    case "tool_finished":
      set((s) => {
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
      set({
        usage: {
          input: ev.input_tokens,
          output: ev.output_tokens,
          cached: ev.cached_tokens ?? 0,
          cacheWrite: ev.cache_write_tokens ?? 0,
          context: ev.context ?? [],
        },
      });
      break;
    case "finished":
      set((s) => ({ streaming: null, iteration: null, approval: null, lessonTick: s.lessonTick + 1 }));
      void get().refreshTopics();
      void get().refreshBrief();
      // 侧栏的对话清单：标题取自第一句话，所以第一轮结束后要刷一次才会出现
      void get().loadChats();
      break;
    case "failed":
      set({ streaming: null, iteration: null, approval: null, chatError: ev.message });
      get().toast("error", ev.message);
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
