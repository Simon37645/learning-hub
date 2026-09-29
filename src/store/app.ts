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
import { api, errText } from "../lib/api";
import type {
  ThemeMode,
  ReasoningEffort,
  ReasoningStyle,
  AgentEvent,
  AgendaBucket,
  Card,
  ChatMessage,
  ChatOverviewItem,
  ConfigPatch,
  DailyBrief,
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
} from "../lib/types";

export type ViewName = "home" | "topic" | "agenda" | "settings";

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
  messages: ChatMessage[];
  streaming: { turnId: string; text: string; thinking: string } | null;
  activities: ToolActivity[];
  iteration: { index: number; max: number } | null;
  usage: { input: number; output: number } | null;
  chatError: string | null;
  approval: ApprovalRequest | null;
  /** 每跑完一轮 +1：讲解步骤面板靠它刷新（agent 会在这一轮里改方案） */
  lessonTick: number;

  // --- 内置浏览器 ---
  viewer: ViewerSnapshot;
  viewerWidth: number;
  /** 侧栏宽度（分界线可拖） */
  sidebarWidth: number;
  goto: GotoRequest | null;
  reloadSeq: Record<string, number>;

  // --- UI ---
  view: ViewName;
  paletteOpen: boolean;
  toasts: Toast[];
  inspectorOpen: boolean;

  // --- 动作 ---
  init: () => Promise<void>;
  refreshTopics: () => Promise<void>;
  refreshBrief: () => Promise<void>;
  refreshAgenda: () => Promise<void>;

  setView: (v: ViewName) => void;
  openTopic: (slug: string) => Promise<void>;
  leaveTopic: () => void;
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
  loadChats: (slug?: string | null) => Promise<void>;
  send: (text: string, attachments?: string[]) => Promise<void>;
  stop: () => Promise<void>;
  approve: (allow: boolean, always: boolean) => Promise<void>;

  patchConfig: (patch: ConfigPatch) => Promise<void>;
  setTheme: (theme: ThemeMode) => Promise<void>;
  setReasoning: (effort: ReasoningEffort, style?: ReasoningStyle) => Promise<void>;
  upsertProfile: (input: ProfileInput) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;

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
  messages: [],
  streaming: null,
  activities: [],
  iteration: null,
  usage: null,
  chatError: null,
  approval: null,
  lessonTick: 0,

  viewer: { tabs: [], activeId: null, visible: false },
  viewerWidth: 460,
  sidebarWidth: (() => {
    const saved = Number(localStorage.getItem("hub.sidebarWidth"));
    return saved >= 200 && saved <= 420 ? saved : 248;
  })(),
  goto: null,
  reloadSeq: {},

  view: "home",
  paletteOpen: false,
  toasts: [],
  inspectorOpen: false,

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
  async loadChats(slug) {
    const target = slug === undefined ? (get().topic?.slug ?? null) : slug;
    try {
      const items = await api.chatOverview(target);
      set((s) => ({ chatIndex: { ...s.chatIndex, [target ?? ""]: items } }));
    } catch (e) {
      console.warn("读取对话清单失败", e);
    }
  },

  async send(text, attachments) {
    const t = text.trim();
    if (!t || get().streaming) return;
    let chatId = get().chatId;
    if (!chatId) {
      chatId = await api.agentNewChat();
      set({ chatId });
    }
    const slug = get().topic?.slug ?? null;
    try {
      const res = await api.agentSend({
        chatId,
        text: t,
        topicSlug: slug,
        stage: get().topic?.meta.stage ?? null,
        attachments: attachments ?? [],
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
    applyTheme(theme);
    await get().patchConfig({ theme });
  },

  async patchConfig(patch) {
    try {
      const config = await api.configPatch(patch);
      set({ config });
      // 主题跟着配置走，改完立刻生效
      applyTheme(config.appearance?.theme ?? "system");
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
}));

// ============================================================ 主题

/**
 * 主题落成 `html[data-theme]`：
 * - light / dark：直接写死，用户要明亮就给他明亮（哪怕系统是深色）
 * - system：跟随系统，并在系统切换时实时跟随
 *
 * 内置编辑器的样式也认这个属性，所以一处生效、全局一致。
 */
let systemThemeListener: ((e: MediaQueryListEvent) => void) | null = null;

function applyTheme(theme: ThemeMode): void {
  const root = document.documentElement;
  const mq = window.matchMedia("(prefers-color-scheme: dark)");

  if (systemThemeListener) {
    mq.removeEventListener("change", systemThemeListener);
    systemThemeListener = null;
  }

  const set = (mode: "light" | "dark") => {
    root.dataset.theme = mode;
    // InkNote 编辑器认这个属性来切 markdown 主题
    if (!root.dataset.mdTheme) root.dataset.mdTheme = "github";
  };

  if (theme === "system") {
    set(mq.matches ? "dark" : "light");
    systemThemeListener = (e) => set(e.matches ? "dark" : "light");
    mq.addEventListener("change", systemThemeListener);
  } else {
    set(theme);
  }
}

// ============================================================ 启动装配

/** 真正干活的启动流程（由 init 保护成只跑一次）。 */
async function bootstrapStore(
  set: (p: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
): Promise<void> {
  // 先按系统主题铺一层，避免启动瞬间闪一下白（配置要等 bootstrap 回来才有）
  applyTheme(get().config?.appearance?.theme ?? "system");

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
    applyTheme(boot.config.appearance?.theme ?? "system");

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
      set({ usage: { input: ev.input_tokens, output: ev.output_tokens } });
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
