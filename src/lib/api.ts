// Tauri 命令的类型化封装。所有与后端的交互都必须经过这里，
// 方便将来替换传输层（例如换成本地 HTTP 服务）。

import { invoke } from "@tauri-apps/api/core";
import type {
  Bootstrap,
  Card,
  ChatMessage,
  ChatOverviewItem,
  ConfigPatch,
  DailyBrief,
  Note,
  NoteSummary,
  PlanTask,
  ProfileInput,
  ProfileTestResult,
  PublicConfig,
  SendResult,
  StudySession,
  TabView,
  TaskWithTopic,
  TopicDetail,
  TopicMatch,
  TopicSummary,
  TurnRequest,
  ViewerSnapshot,
  WorkspaceInfo,
  AgendaBucket,
  MaterialItem,
  OpenRequest,
  PageText,
  StudyStage,
  Grade,
  CardKind,
  ReasoningEffort,
  ReasoningStyle,
  SkillDirs,
  SkillsOverview,
  Scope,
  McpOverview,
  McpStatusEntry,
  McpServerConfig,
  MemoryInput,
  MemoryKindInfo,
  MemoryOverview,
  MemoryPaths,
  MemoryScope,
  LessonPlan,
  LessonStep,
  StepStatus,
  Quiz,
  QuizSummary,
  QuizAnswer,
  Attempt,
  WrongItem,
  Question,
  QuizKind,
  AgentMode,
  CustomTheme,
  StudioInfo,
  ThemesOverview,
} from "./types";

/** 后端返回的错误是纯字符串 */
export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const api = {
  // ---------------- 应用 ----------------
  bootstrap: () => invoke<Bootstrap>("app_bootstrap"),
  configGet: () => invoke<PublicConfig>("config_get"),
  configPatch: (patch: ConfigPatch) => invoke<PublicConfig>("config_patch", { patch }),
  profileUpsert: (input: ProfileInput) => invoke<PublicConfig>("profile_upsert", { input }),
  profileDelete: (id: string) => invoke<PublicConfig>("profile_delete", { id }),
  profileTest: (id: string) => invoke<ProfileTestResult>("profile_test", { id }),
  profileSetReasoning: (profileId: string, effort: ReasoningEffort, style?: ReasoningStyle) =>
    invoke<PublicConfig>("profile_set_reasoning", { profileId, effort, style: style ?? null }),
  promptPreview: (topicSlug?: string | null, mode?: AgentMode | null) =>
    invoke<string>("prompt_preview", { topicSlug: topicSlug ?? null, mode: mode ?? null }),
  workspaceInfo: () => invoke<WorkspaceInfo>("workspace_info"),
  revealInExplorer: (topicSlug?: string | null) =>
    invoke<string>("reveal_in_explorer", { topicSlug: topicSlug ?? null }),
  openWithSystem: (topicSlug: string, path: string) =>
    invoke<void>("open_with_system", { topicSlug, path }),

  // ---------------- 主题 ----------------
  topicList: () => invoke<TopicSummary[]>("topic_list"),
  topicSearch: (query: string, limit?: number) =>
    invoke<TopicMatch[]>("topic_search", { query, limit: limit ?? 20 }),
  topicCreate: (name: string, description?: string, emoji?: string, parent?: string | null) =>
    invoke<TopicDetail>("topic_create", {
      name,
      description: description ?? null,
      emoji: emoji ?? null,
      parent: parent ?? null,
    }),
  topicGet: (slug: string) => invoke<TopicDetail>("topic_get", { slug }),
  topicOpen: (slug: string) => invoke<TopicDetail>("topic_open", { slug }),
  topicSetParent: (slug: string, parent: string | null) =>
    invoke<TopicDetail>("topic_set_parent", { slug, parent }),
  topicUpdate: (
    slug: string,
    patch: { name?: string; description?: string; emoji?: string; tags?: string[]; stage?: StudyStage },
  ) => invoke<TopicDetail>("topic_update", { slug, patch }),
  topicDelete: (slug: string) => invoke<string>("topic_delete", { slug }),

  // ---------------- 笔记 ----------------
  noteList: (slug: string, query?: string) =>
    invoke<NoteSummary[]>("note_list", { slug, query: query ?? null }),
  noteGet: (slug: string, path: string) => invoke<Note>("note_get", { slug, path }),
  noteSave: (slug: string, path: string, content: string) =>
    invoke<NoteSummary>("note_save", { slug, path, content }),
  noteCreate: (slug: string, title: string, dir?: string) =>
    invoke<Note>("note_create", { slug, title, dir: dir ?? null }),
  noteDelete: (slug: string, path: string) => invoke<string>("note_delete", { slug, path }),

  // ---------------- 资料 ----------------
  materialList: (slug: string) => invoke<MaterialItem[]>("material_list", { slug }),
  kbRebuild: (slug: string) =>
    invoke<{ files: number; chunks: number; paged: number }>("kb_rebuild", { slug }),
  materialImport: (slug: string, sources: string[], subdir?: string) =>
    invoke<{ imported: MaterialItem[]; skipped: string[]; dir: string }>("material_import", {
      slug,
      sources,
      subdir: subdir ?? null,
    }),

  // ---------------- 对话 ----------------
  agentSend: (req: TurnRequest) => invoke<SendResult>("agent_send", { req }),
  agentCancel: (turnId: string) => invoke<boolean>("agent_cancel", { turnId }),
  agentApprove: (requestId: string, allow: boolean, always?: boolean) =>
    invoke<boolean>("agent_approve", { requestId, allow, always: always ?? false }),
  agentTranscript: (chatId: string, topicSlug?: string | null) =>
    invoke<ChatMessage[]>("agent_transcript", { chatId, topicSlug: topicSlug ?? null }),
  agentChats: (topicSlug?: string | null) =>
    invoke<string[]>("agent_chats", { topicSlug: topicSlug ?? null }),
  /** 对话清单。`mode` 决定要哪一份：工坊的对话和工作区级的日常问答共用同一个目录 */
  chatOverview: (topicSlug?: string | null, includeArchived?: boolean, mode?: AgentMode | null) =>
    invoke<ChatOverviewItem[]>("chat_overview", {
      topicSlug: topicSlug ?? null,
      includeArchived: includeArchived ?? false,
      mode: mode ?? null,
    }),
  chatMove: (chatId: string, fromSlug: string, toSlug: string) =>
    invoke<void>("chat_move", { chatId, fromSlug, toSlug }),
  chatRename: (chatId: string, title: string, topicSlug?: string | null, mode?: AgentMode | null) =>
    invoke<ChatOverviewItem[]>("chat_rename", {
      chatId,
      title,
      topicSlug: topicSlug ?? null,
      mode: mode ?? null,
    }),
  chatPin: (chatId: string, pinned: boolean, topicSlug?: string | null, mode?: AgentMode | null) =>
    invoke<ChatOverviewItem[]>("chat_pin", {
      chatId,
      pinned,
      topicSlug: topicSlug ?? null,
      mode: mode ?? null,
    }),
  chatArchive: (chatId: string, archived: boolean, topicSlug?: string | null, mode?: AgentMode | null) =>
    invoke<ChatOverviewItem[]>("chat_archive", {
      chatId,
      archived,
      topicSlug: topicSlug ?? null,
      mode: mode ?? null,
    }),
  /** 分叉：在「最后一个完整回合」处截断复制成新对话，返回新对话 id */
  chatFork: (
    chatId: string,
    topicSlug?: string | null,
    toSlug?: string | null,
    mode?: AgentMode | null,
  ) =>
    invoke<string>("chat_fork", {
      chatId,
      topicSlug: topicSlug ?? null,
      toSlug: toSlug ?? null,
      mode: mode ?? null,
    }),
  chatDelete: (chatId: string, topicSlug?: string | null) =>
    invoke<void>("chat_delete", { chatId, topicSlug: topicSlug ?? null }),
  agentNewChat: () => invoke<string>("agent_new_chat"),
  /** 读一张本机图片转成可上传的载荷（用户在输入框里选了图片时用） */
  agentImageLoad: (path: string) =>
    invoke<{ name: string; mediaType: string; data: string }>("agent_image_load", { path }),

  // ---------------- 内置浏览器 ----------------
  viewerSnapshot: () => invoke<ViewerSnapshot>("viewer_snapshot"),
  viewerOpen: (req: OpenRequest) => invoke<TabView>("viewer_open", { req }),
  /** 网页能否内嵌（站点可能用 X-Frame-Options / CSP 拒绝） */
  webFrameCheck: (url: string) =>
    invoke<{ embeddable: boolean; reason: string }>("web_frame_check", { url }),
  viewerClose: (tabId: string) => invoke<void>("viewer_close", { tabId }),
  viewerActivate: (tabId: string) => invoke<TabView>("viewer_activate", { tabId }),
  viewerSetVisible: (visible: boolean) => invoke<void>("viewer_set_visible", { visible }),
  viewerReportState: (tabId: string, page?: number, scroll?: number, totalPages?: number) =>
    invoke<void>("viewer_report_state", {
      tabId,
      page: page ?? null,
      scroll: scroll ?? null,
      totalPages: totalPages ?? null,
    }),
  viewerReportSnapshot: (
    tabId: string,
    payload: { content?: string; pages?: PageText[]; totalPages?: number; error?: string },
  ) =>
    invoke<void>("viewer_report_snapshot", {
      tabId,
      content: payload.content ?? null,
      pages: payload.pages ?? null,
      totalPages: payload.totalPages ?? null,
      error: payload.error ?? null,
    }),
  viewerLoadText: (tabId: string) => invoke<string>("viewer_load_text", { tabId }),
  /**
   * 把 agent 要的那一页位图交回后端（`pdf_screenshot` 用）。
   * 渲染不出来时也必须回一次（带 `error`），否则工具那边会一直等到超时。
   */
  viewerReportRender: (
    requestId: string,
    payload: { data?: string; width?: number; height?: number; error?: string },
  ) =>
    invoke<void>("viewer_report_render", {
      requestId,
      data: payload.data ?? null,
      width: payload.width ?? null,
      height: payload.height ?? null,
      error: payload.error ?? null,
    }),
  viewerLoadBytes: (tabId: string) => invoke<ArrayBuffer>("viewer_load_bytes", { tabId }),
  viewerGetContent: (tabId: string) => invoke<string>("viewer_get_content", { tabId }),
  viewerReload: (tabId: string) => invoke<void>("viewer_reload", { tabId }),
  viewerOpenHome: () => invoke<TabView>("viewer_open_home"),

  // ---------------- 学习资产 ----------------
  cardList: (slug: string, opts?: { dueOnly?: boolean; query?: string }) =>
    invoke<Card[]>("card_list", {
      slug,
      dueOnly: opts?.dueOnly ?? null,
      query: opts?.query ?? null,
    }),
  cardCreate: (
    slug: string,
    input: { front: string; back: string; kind?: CardKind; tags?: string[]; module?: string; source?: string },
  ) =>
    invoke<Card>("card_create", {
      slug,
      input: {
        front: input.front,
        back: input.back,
        kind: input.kind ?? "basic",
        tags: input.tags ?? [],
        module: input.module ?? null,
        source: input.source ?? null,
      },
    }),
  cardUpdate: (
    slug: string,
    id: string,
    input: { front: string; back: string; kind?: CardKind; tags?: string[]; module?: string; source?: string },
  ) =>
    invoke<Card>("card_update", {
      slug,
      id,
      input: {
        front: input.front,
        back: input.back,
        kind: input.kind ?? "basic",
        tags: input.tags ?? [],
        module: input.module ?? null,
        source: input.source ?? null,
      },
    }),
  cardDelete: (slug: string, ids: string[]) => invoke<number>("card_delete", { slug, ids }),
  cardReview: (slug: string, id: string, grade: Grade) =>
    invoke<Card>("card_review", { slug, id, grade }),


  taskList: (opts?: { slug?: string; status?: string; includeDone?: boolean }) =>
    invoke<TaskWithTopic[]>("task_list", {
      slug: opts?.slug ?? null,
      status: opts?.status ?? null,
      includeDone: opts?.includeDone ?? null,
    }),
  taskCreate: (
    slug: string,
    input: {
      title: string;
      detail?: string;
      due?: string | null;
      priority?: number;
      estimateMin?: number | null;
      stage?: StudyStage | null;
      module?: string | null;
    },
  ) =>
    invoke<PlanTask>("task_create", {
      slug,
      input: {
        title: input.title,
        detail: input.detail ?? "",
        due: input.due ?? null,
        priority: input.priority ?? null,
        estimateMin: input.estimateMin ?? null,
        stage: input.stage ?? null,
        module: input.module ?? null,
      },
    }),
  taskUpdate: (
    slug: string,
    id: string,
    patch: {
      title?: string;
      detail?: string;
      status?: string;
      due?: string;
      priority?: number;
      estimateMin?: number;
    },
  ) => invoke<PlanTask>("task_update", { slug, id, patch }),
  taskDelete: (slug: string, id: string) => invoke<boolean>("task_delete", { slug, id }),
  agenda: (horizonDays?: number) => invoke<AgendaBucket[]>("agenda", { horizonDays: horizonDays ?? 7 }),
  dailyBrief: () => invoke<DailyBrief>("daily_brief"),

  // ---------------- 技能与 MCP ----------------
  skillsOverview: (topicSlug?: string | null) =>
    invoke<SkillsOverview>("skills_overview", { topicSlug: topicSlug ?? null }),
  skillsReload: () => invoke<number>("skills_reload"),
  skillsSetEnabled: (enabled: boolean) => invoke<void>("skills_set_enabled", { enabled }),
  /** 一键开关全部技能（不区分来源，扫到什么管什么） */
  skillsSetAll: (enabled: boolean) => invoke<number>("skills_set_all", { enabled }),
  skillSetEnabled: (skillId: string, scope: Scope, enabled: boolean, topicSlug?: string | null) =>
    invoke<void>("skill_set_enabled", { skillId, scope, enabled, topicSlug: topicSlug ?? null }),
  skillsDirs: (topicSlug?: string | null) =>
    invoke<SkillDirs>("skills_dirs", { topicSlug: topicSlug ?? null }),
  skillSave: (
    input: { id: string; name: string; description: string; body: string },
    scope: Scope,
    topicSlug?: string | null,
    overwrite?: boolean,
  ) => invoke<string>("skill_save", { input, scope, topicSlug: topicSlug ?? null, overwrite: overwrite ?? false }),

  mcpOverview: (topicSlug?: string | null) =>
    invoke<McpOverview>("mcp_overview", { topicSlug: topicSlug ?? null }),
  mcpStatus: () => invoke<McpStatusEntry[]>("mcp_status"),
  mcpReload: () => invoke<McpStatusEntry[]>("mcp_reload"),
  mcpUpsert: (server: McpServerConfig, scope: Scope, topicSlug?: string | null, originalName?: string) =>
    invoke<McpStatusEntry[]>("mcp_upsert", {
      server,
      scope,
      topicSlug: topicSlug ?? null,
      originalName: originalName ?? null,
    }),
  mcpDelete: (name: string, scope: Scope, topicSlug?: string | null) =>
    invoke<McpStatusEntry[]>("mcp_delete", { name, scope, topicSlug: topicSlug ?? null }),
  mcpSetEnabled: (name: string, enabled: boolean, scope: Scope, topicSlug?: string | null) =>
    invoke<McpStatusEntry[]>("mcp_set_enabled", { name, enabled, scope, topicSlug: topicSlug ?? null }),
  /** 一键开关全部 MCP 服务器 */
  mcpSetAll: (enabled: boolean) => invoke<McpStatusEntry[]>("mcp_set_all", { enabled }),

  // ---------------- 工坊（独立于学习：造技能与 MCP 服务器）----------------
  studioInfo: () => invoke<StudioInfo>("studio_info"),
  /** 读一份内置规范文档（skill / mcp） */
  studioSpec: (doc: string) => invoke<string>("studio_spec", { doc }),
  studioOpenDir: (ensure?: boolean) =>
    invoke<string>("studio_open_dir", { ensure: ensure ?? true }),

  // ---------------- 自定义外观主题 ----------------
  themesOverview: () => invoke<ThemesOverview>("themes_overview"),
  /** 切换自定义主题；传 null 回到内置配色 */
  themeSetActive: (id: string | null) =>
    invoke<PublicConfig>("theme_set_active", { id: id ?? null }),
  themeSave: (theme: CustomTheme) => invoke<ThemesOverview>("theme_save", { theme }),
  themeDelete: (id: string) => invoke<ThemesOverview>("theme_delete", { id }),
  themeOpenDir: (user?: boolean) => invoke<string>("theme_open_dir", { user: user ?? false }),

  // ---------------- 长期记忆 ----------------
  memoryOverview: (topicSlug?: string | null) =>
    invoke<MemoryOverview>("memory_overview", { topicSlug: topicSlug ?? null }),
  memoryUpsert: (input: MemoryInput, scope: MemoryScope, topicSlug?: string | null) =>
    invoke<MemoryOverview>("memory_upsert", { input, scope, topicSlug: topicSlug ?? null }),
  memoryDelete: (id: string, scope: MemoryScope, topicSlug?: string | null) =>
    invoke<MemoryOverview>("memory_delete", { id, scope, topicSlug: topicSlug ?? null }),
  memorySetPinned: (id: string, pinned: boolean, scope: MemoryScope, topicSlug?: string | null) =>
    invoke<MemoryOverview>("memory_set_pinned", { id, pinned, scope, topicSlug: topicSlug ?? null }),
  memoryClear: (scope: MemoryScope, topicSlug?: string | null) =>
    invoke<number>("memory_clear", { scope, topicSlug: topicSlug ?? null }),
  memorySetEnabled: (enabled: boolean, topicSlug?: string | null) =>
    invoke<MemoryOverview>("memory_set_enabled", { enabled, topicSlug: topicSlug ?? null }),
  /** 分类的中文名与说明由后端给，前端不重复维护一份 */
  memoryKinds: () => invoke<MemoryKindInfo[]>("memory_kinds"),
  memoryPaths: (topicSlug?: string | null) =>
    invoke<MemoryPaths>("memory_paths", { topicSlug: topicSlug ?? null }),

  // ---------------- 讲解方案 ----------------
  lessonList: (slug: string) => invoke<LessonPlan[]>("lesson_list", { slug }),
  lessonGet: (slug: string, id: string) => invoke<LessonPlan>("lesson_get", { slug, id }),
  lessonSave: (
    slug: string,
    input: {
      id?: string | null;
      title: string;
      goal?: string;
      prereqs?: string[];
      steps?: LessonStep[];
      finished?: boolean;
    },
  ) => invoke<LessonPlan>("lesson_save", { slug, input }),
  lessonSetStep: (slug: string, id: string, index: number, status: StepStatus) =>
    invoke<LessonPlan>("lesson_set_step", { slug, id, index, status }),
  lessonDelete: (slug: string, id: string) => invoke<void>("lesson_delete", { slug, id }),

  // ---------------- 测验 ----------------
  quizList: (slug: string) => invoke<QuizSummary[]>("quiz_list", { slug }),
  quizGet: (slug: string, id: string) => invoke<Quiz>("quiz_get", { slug, id }),
  quizSave: (
    slug: string,
    input: { id?: string | null; title: string; kind?: QuizKind; scope?: string; questions: Question[] },
  ) => invoke<Quiz>("quiz_save", { slug, input }),
  quizDelete: (slug: string, id: string) => invoke<void>("quiz_delete", { slug, id }),
  quizSubmit: (slug: string, id: string, answers: QuizAnswer[]) =>
    invoke<Attempt>("quiz_submit", { slug, id, answers }),
  quizAttempts: (slug: string, id: string) => invoke<Attempt[]>("quiz_attempts", { slug, id }),
  quizGradeSubjective: (slug: string, quizId: string, attemptId: string) =>
    invoke<Attempt>("quiz_grade_subjective", { slug, quizId, attemptId }),
  quizWrongItems: (slug: string, quizId: string, attemptId: string) =>
    invoke<WrongItem[]>("quiz_wrong_items", { slug, quizId, attemptId }),

  sessionList: (slug: string) => invoke<StudySession[]>("session_list", { slug }),
  sessionCurrent: () => invoke<StudySession | null>("session_current"),
  sessionStart: (
    slug: string,
    title: string,
    opts?: { stage?: StudyStage; goals?: string[]; chatId?: string },
  ) =>
    invoke<StudySession>("session_start", {
      slug,
      title,
      stage: opts?.stage ?? null,
      goals: opts?.goals ?? null,
      chatId: opts?.chatId ?? null,
    }),
  sessionFinish: (summary: string, highlights?: string[], openQuestions?: string[]) =>
    invoke<StudySession>("session_finish", {
      summary,
      highlights: highlights ?? null,
      openQuestions: openQuestions ?? null,
    }),
};
