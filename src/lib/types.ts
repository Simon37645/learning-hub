// 与 Rust 端 serde 输出一一对应的类型定义。
//
// 注意几个坑：
// - 结构体基本是 camelCase，但**带 tag 的枚举内部字段仍是 snake_case**
//   （serde 的 rename_all 只作用于变体名，不作用于变体里的字段）
// - 枚举值统一用 snake_case 字符串

// ---------------------------------------------------------------- 基础

export type StudyStage = "preview" | "learn" | "review" | "test";
export type PermissionMode = "ask" | "auto_edit" | "full";
export type ProviderKind = "open_ai" | "anthropic";
export type Risk = "read" | "write" | "destructive";
export type Role = "system" | "user" | "assistant" | "tool";

export const STAGE_LABEL: Record<StudyStage, string> = {
  preview: "预习",
  learn: "学习",
  review: "复习",
  test: "测验",
};

export const PERMISSION_LABEL: Record<PermissionMode, string> = {
  ask: "每次确认",
  auto_edit: "自动编辑",
  full: "完全访问",
};

export const RISK_LABEL: Record<Risk, string> = {
  read: "读取",
  write: "写入",
  destructive: "删除/覆盖",
};

// ---------------------------------------------------------------- 消息

export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean };

export interface MessageMeta {
  model?: string | null;
  durationMs?: number | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  /** 输入里由缓存提供的部分。服务商没报就是 null/undefined——这一轮不计入命中率 */
  cachedTokens?: number | null;
  /** 这一轮写进缓存的部分（Anthropic 才有） */
  cacheWriteTokens?: number | null;
  interrupted?: boolean;
}

/** 「上下文构成」里的一块：标签 + 估算 token 数（后端按字符估的，只用来画比例） */
export interface ContextPart {
  label: string;
  tokens: number;
}

export interface ChatMessage {
  id: string;
  role: Role;
  blocks: ContentBlock[];
  createdAt: string;
  meta: MessageMeta;
}

// ---------------------------------------------------------------- 主题

export interface TopicMeta {
  id: string;
  name: string;
  emoji?: string | null;
  description: string;
  tags: string[];
  stage: StudyStage;
  createdAt: string;
  updatedAt: string;
  lastOpenedAt?: string | null;
  /** 父主题的 id（不是目录名）。没有就是顶层主题。子主题=这门课的一章，能读父主题的资料 */
  parent?: string | null;
}

export interface TopicStats {
  notes: number;
  materials: number;
  cards: number;
  cardsDue: number;
  tasksOpen: number;
  tasksDone: number;
  sessions: number;
  /** 对话条数（侧栏把对话挂在主题下面，用它决定有没有折叠箭头） */
  chats?: number;
  nextDue?: string | null;
}

export interface TopicSummary {
  meta: TopicMeta;
  slug: string;
  path: string;
  stats: TopicStats;
}

export interface NoteSummary {
  path: string;
  title: string;
  tags: string[];
  updatedAt: string;
  size: number;
  excerpt: string;
}

export interface Note extends NoteSummaryShape {
  content: string;
}
interface NoteSummaryShape {
  path: string;
  title: string;
  tags: string[];
  updatedAt: string;
}

export interface MaterialItem {
  /** 相对**所属主题**的路径（继承来的资料就是相对父主题） */
  path: string;
  name: string;
  size: number;
  sizeText: string;
  kind: string;
  modifiedAt: string;
  /** 所属主题的 slug：打开它要用这个主题，而不是当前主题 */
  topic: string;
  /** 来源主题名；本主题自己的资料没有这个字段 */
  origin?: string | null;
  /** 继承自父主题（只读） */
  inherited: boolean;
}

export interface StudySession {
  id: string;
  topicId: string;
  topicSlug: string;
  title: string;
  stage: StudyStage;
  goals: string[];
  materials: string[];
  chatId: string;
  startedAt: string;
  endedAt?: string | null;
  summary: string;
  highlights: string[];
  openQuestions: string[];
  cardsCreated: number;
  notesCreated: number;
}

export interface TopicDetail {
  meta: TopicMeta;
  slug: string;
  path: string;
  stats: TopicStats;
  notes: NoteSummary[];
  materials: MaterialItem[];
  sessions: StudySession[];
  chats: string[];
  currentSession?: StudySession | null;
  readme?: string | null;
}

export interface TopicMatch {
  summary: TopicSummary;
  hits: string[];
  score: number;
}

/** 侧栏「对话」列表里的一项 */
export interface ChatOverviewItem {
  id: string;
  /** 显示标题：用户起过名就用它，否则是第一句用户消息的前 40 字 */
  title: string;
  /** 用户是否自己改过名（可以「恢复自动标题」） */
  customTitle: boolean;
  messages: number;
  updatedAt: string;
  /** 置顶：排在该主题对话列表最前面 */
  pinned: boolean;
  /** 归档：默认收进侧栏的「已归档」分组 */
  archived: boolean;
  /** 分叉血缘（从哪条对话、第几条消息分出来的） */
  forkedFrom?: ChatForkInfo | null;
}

/** 分叉血缘：只记「从哪来」，不记「分出去哪些」 */
export interface ChatForkInfo {
  chatId: string;
  /** 分叉点在原对话里的消息序号（0 基） */
  atMessage: number;
  title: string;
}

// ---------------------------------------------------------------- 卡片 / 任务

export interface SrsState {
  ease: number;
  intervalDays: number;
  repetitions: number;
  lapses: number;
  due: string;
  lastReview?: string | null;
  reviews: number;
}

/** 卡片类型：基础 / 反向 / 完形填空 */
export type CardKind = "basic" | "reversed" | "cloze";

export const CARD_KIND_LABEL: Record<CardKind, string> = {
  basic: "基础",
  reversed: "反向",
  cloze: "完形",
};

export interface Card {
  id: string;
  kind: CardKind;
  front: string;
  back: string;
  source?: string | null;
  tags: string[];
  module?: string | null;
  createdAt: string;
  srs: SrsState;
  /** 四档评分各自会把下次复习推到多久之后（秒）。后端用同一套调度算法算好，界面直接印在按钮上 */
  preview?: { again: number; hard: number; good: number; easy: number };
}

export type Grade = "again" | "hard" | "good" | "easy";

export const GRADE_LABEL: Record<Grade, string> = {
  again: "忘了",
  hard: "吃力",
  good: "记得",
  easy: "太简单",
};

export type TaskStatus = "todo" | "doing" | "done" | "archived";

export const TASK_STATUS_LABEL: Record<TaskStatus, string> = {
  todo: "待办",
  doing: "进行中",
  done: "已完成",
  archived: "已归档",
};

export interface PlanTask {
  id: string;
  title: string;
  detail: string;
  status: TaskStatus;
  priority: number;
  due?: string | null;
  estimateMin?: number | null;
  stage?: StudyStage | null;
  module?: string | null;
  createdAt: string;
  updatedAt: string;
  doneAt?: string | null;
}

export interface TaskWithTopic {
  topicSlug: string;
  topicName: string;
  task: PlanTask;
  overdue: boolean;
}

export interface AgendaItem {
  topicSlug: string;
  topicName: string;
  task: PlanTask;
}

export interface AgendaBucket {
  key: string;
  label: string;
  tasks: AgendaItem[];
}

export interface HeatCell {
  date: string;
  count: number;
  level: number;
}

export interface DailyBrief {
  date: string;
  dueCards: number;
  openTasks: number;
  overdueTasks: number;
  topicsTouchedToday: number;
  heatmap: HeatCell[];
}

// ---------------------------------------------------------------- 配置

export interface AgentConfig {
  permissionMode: PermissionMode;
  maxIterations: number;
  systemPromptExtra: string;
  allowWeb: boolean;
  requestTimeoutSecs: number;
  contextBudgetChars: number;
  /** 沙箱：agent 默认只能碰工作区里的文件 */
  sandbox: boolean;
  /** 用户批准过的「工作区之外」的目录 */
  approvedRoots: string[];
  /** 长期记忆：记下用户的特点与注意事项，每轮注入系统提示词 */
  memoryEnabled: boolean;
}

export interface ViewerConfig {
  homeUrl: string;
  searchEngine: string;
}

/** 思考强度：让模型想多久 */
export type ReasoningEffort = "off" | "low" | "medium" | "high" | "max";

export const EFFORT_LABEL: Record<ReasoningEffort, string> = {
  off: "关闭",
  low: "低",
  medium: "中",
  high: "高",
  max: "最高",
};

export const EFFORT_HINT: Record<ReasoningEffort, string> = {
  off: "不发送思考参数（兼容性最好）",
  low: "适合改写、格式化这类轻任务",
  medium: "日常问答与讲解",
  high: "复杂推导、长材料分析",
  max: "最费 token、最慢，留给硬骨头",
};

/** 发送方式：各家扩展不统一，认不出来时可以关掉 */
export type ReasoningStyle = "auto" | "openai_effort" | "anthropic_thinking" | "qwen_thinking" | "none";

export const STYLE_LABEL: Record<ReasoningStyle, string> = {
  auto: "自动（按协议）",
  openai_effort: "reasoning_effort",
  anthropic_thinking: "thinking",
  qwen_thinking: "enable_thinking",
  none: "不发",
};

export interface ReasoningConfig {
  effort: ReasoningEffort;
  style: ReasoningStyle;
}

export interface PublicProfile {
  id: string;
  name: string;
  kind: ProviderKind;
  kindLabel: string;
  baseUrl: string;
  model: string;
  temperature: number;
  maxTokens: number;
  supportsTools: boolean;
  reasoning: ReasoningConfig;
  hasApiKey: boolean;
  keyHint: string;
}

export type ThemeMode = "system" | "light" | "dark";

export const THEME_LABEL: Record<ThemeMode, string> = {
  system: "跟随系统",
  light: "明亮",
  dark: "深色",
};

export interface AppearanceConfig {
  theme: ThemeMode;
}

export interface PublicConfig {
  version: number;
  userName: string;
  workspaceRoot: string;
  activeProfileId: string;
  profiles: PublicProfile[];
  agent: AgentConfig;
  viewer: ViewerConfig;
  appearance: AppearanceConfig;
  lastTopic?: string | null;
}

export interface ToolSpec {
  name: string;
  description: string;
  inputSchema: unknown;
}

export interface Bootstrap {
  version: string;
  config: PublicConfig;
  topics: TopicSummary[];
  tools: ToolSpec[];
  toolNames: string[];
  permissionModes: { id: PermissionMode; label: string; hint: string }[];
  providerKinds: { id: ProviderKind; label: string; defaultBaseUrl: string; defaultModel: string }[];
}

export interface ConfigPatch {
  userName?: string;
  workspaceRoot?: string;
  activeProfileId?: string;
  permissionMode?: PermissionMode;
  allowWeb?: boolean;
  sandbox?: boolean;
  maxIterations?: number;
  systemPromptExtra?: string;
  requestTimeoutSecs?: number;
  contextBudgetChars?: number;
  homeUrl?: string;
  searchEngine?: string;
  theme?: ThemeMode;
  /** 长期记忆总开关 */
  memoryEnabled?: boolean;
  /** 撤销某个已授权目录 */
  revokeRoot?: string;
  /** 清空全部越权白名单 */
  clearApprovedRoots?: boolean;
}

export interface ProfileInput {
  id?: string | null;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey?: string | null;
  temperature: number;
  maxTokens: number;
  supportsTools: boolean;
  headers: Record<string, string>;
  reasoning?: ReasoningConfig;
}

export interface ProfileTestResult {
  ok: boolean;
  message: string;
  latencyMs: number;
}

export interface WorkspaceInfo {
  root: string;
  exists: boolean;
  topicCount: number;
  diskUsageText: string;
}

// ---------------------------------------------------------------- agent 事件

export interface PendingCall {
  requestId: string;
  callId: string;
  name: string;
  summary: string;
  risk: Risk;
  riskLabel: string;
  input: unknown;
}

export interface ToolOutcomeView {
  callId: string;
  name: string;
  summary: string;
  risk: Risk;
  ok: boolean;
  preview: string;
  durationMs: number;
  denied: boolean;
}

/** 越权申请：agent 想访问工作区之外的文件 */
export interface SandboxRequest {
  requestId: string;
  /** agent 想访问的具体路径 */
  path: string;
  /** 拟授权的上级目录（批准一次，整个目录长期有效） */
  root: string;
  /** read / write / delete */
  mode: string;
  reason: string;
  preApproved: boolean;
}

export const SANDBOX_MODE_LABEL: Record<string, string> = {
  read: "读取",
  write: "写入",
  delete: "删除",
};

/** 需要用户点头的两类请求：工具执行、越权访问工作区之外 */
export type ApprovalRequest =
  | { kind: "tool"; call: PendingCall }
  | { kind: "sandbox"; request: SandboxRequest };

export type AgentEvent =
  | { kind: "turn_started"; turn_id: string; chat_id: string; topic_slug: string | null; message_id: string; mode: PermissionMode }
  | { kind: "delta"; turn_id: string; message_id: string; text: string; thinking: boolean }
  | { kind: "message"; turn_id: string; message: ChatMessage }
  | { kind: "iteration"; turn_id: string; index: number; max: number }
  | { kind: "tool_approval"; turn_id: string; call: PendingCall }
  | {
      kind: "sandbox_request";
      turn_id: string;
      request_id: string;
      path: string;
      root: string;
      mode: string;
      reason: string;
      pre_approved: boolean;
    }
  | { kind: "tool_started"; turn_id: string; call_id: string; name: string; summary: string; risk: Risk }
  | { kind: "tool_finished"; turn_id: string; outcome: ToolOutcomeView }
  | {
      kind: "usage";
      turn_id: string;
      input_tokens: number;
      output_tokens: number;
      /** 服务商报的真实用量；不报就是 0（界面把这轮算作「无数据」） */
      cached_tokens?: number;
      cache_write_tokens?: number;
      /** 最后一次请求的上下文构成 */
      context?: ContextPart[];
    }
  | { kind: "finished"; turn_id: string; reason: string; duration_ms: number; interrupts: number }
  | { kind: "failed"; turn_id: string; message: string };

export interface SendResult {
  turnId: string;
  chatId: string;
  message: ChatMessage;
}

export interface TurnRequest {
  topicSlug?: string | null;
  chatId: string;
  text: string;
  stage?: string | null;
  attachments?: string[];
}

// ---------------------------------------------------------------- 查看器

export type ViewerKind = "markdown" | "pdf" | "web" | "text" | "image" | "blank";

export interface TabView {
  id: string;
  kind: ViewerKind;
  title: string;
  url?: string | null;
  topicSlug?: string | null;
  path?: string | null;
  page: number;
  totalPages: number;
  scroll: number;
  zoom: number;
  loading: boolean;
  error?: string | null;
  snapshotChars: number;
  updatedAt: string;
}

export interface ViewerSnapshot {
  tabs: TabView[];
  activeId?: string | null;
  visible: boolean;
}

export interface PageText {
  page: number;
  text: string;
}

export interface OpenRequest {
  url?: string | null;
  path?: string | null;
  topicSlug?: string | null;
  title?: string | null;
  kind?: ViewerKind | null;
  page?: number | null;
  newTab?: boolean;
}

export type ViewerEvent =
  | { kind: "sync"; snapshot: ViewerSnapshot }
  | { kind: "snapshot_request"; tab_id: string }
  | { kind: "goto"; tab_id: string; page: number | null; scroll: number | null; anchor: string | null; highlight: string | null }
  | { kind: "reload"; tab_id: string }
  | { kind: "updated"; tab: TabView };

export type TopicsEvent =
  | { kind: "created"; slug: string; name: string }
  | { kind: "updated"; slug: string }
  | { kind: "deleted"; slug: string }
  | { kind: "refresh" };

export interface ToastEvent {
  level: "info" | "warn" | "error" | "success";
  message: string;
}

// ---------------------------------------------------------------- 测验

export type QuestionType = "a1" | "a2" | "b" | "x" | "term" | "short";

export const QUESTION_TYPE_LABEL: Record<QuestionType, string> = {
  a1: "A1 型",
  a2: "A2 型",
  b: "B 型",
  x: "X 型",
  term: "名词解释",
  short: "简答",
};

/** 主观题：由 agent 按采分点评分 */
export const SUBJECTIVE_TYPES: QuestionType[] = ["term", "short"];

export interface QuizOptionItem {
  key: string;
  text: string;
}

export interface Question {
  id: string;
  type: QuestionType;
  stem: string;
  caseText?: string | null;
  options: QuizOptionItem[];
  answer: string[];
  keyPoints: string[];
  explanation: string;
  source?: string | null;
  score: number;
  group?: string | null;
}

export type QuizKind = "practice" | "exam";

export interface Quiz {
  id: string;
  topicId: string;
  title: string;
  kind: QuizKind;
  scope: string;
  questions: Question[];
  createdAt: string;
  createdBy: string;
}

export interface QuizSummary {
  id: string;
  title: string;
  kind: QuizKind;
  scope: string;
  questionCount: number;
  totalScore: number;
  typeSummary: string;
  createdAt: string;
  lastPercent?: number | null;
  attemptCount: number;
}

export interface QuizAnswer {
  questionId: string;
  value: string;
}

export interface QuestionResult {
  questionId: string;
  correct?: boolean | null;
  score: number;
  maxScore: number;
  comment: string;
  missing: string[];
  gradedByAgent: boolean;
}

export interface Attempt {
  id: string;
  quizId: string;
  startedAt: string;
  finishedAt?: string | null;
  answers: QuizAnswer[];
  results: QuestionResult[];
  score: number;
  total: number;
  pendingSubjective: string[];
}

export interface WrongItem {
  questionId: string;
  kind: string;
  stem: string;
  yourAnswer: string;
  correctAnswer: string;
  explanation: string;
  missing: string[];
}

// ---------------------------------------------------------------- 讲解方案（讲解模式）

export type StepStatus = "todo" | "doing" | "done" | "skipped";

export const STEP_STATUS_LABEL: Record<StepStatus, string> = {
  todo: "待讲",
  doing: "讲解中",
  done: "已讲完",
  skipped: "跳过",
};

export interface LessonStep {
  index: number;
  title: string;
  focus: string;
  check: string;
  sources: string[];
  status: StepStatus;
  demoHtml?: string | null;
}

export interface LessonPlan {
  id: string;
  topicId: string;
  title: string;
  goal: string;
  prereqs: string[];
  steps: LessonStep[];
  createdAt: string;
  updatedAt: string;
  finished: boolean;
}

// ---------------------------------------------------------------- 技能与 MCP

/** 作用域：全局配置，或只属于当前主题 */
export type Scope = "global" | "topic";

export const SCOPE_LABEL: Record<Scope, string> = {
  global: "全局",
  topic: "本主题",
};

export interface SkillEntry {
  id: string;
  name: string;
  description: string;
  dir: string;
  source: string;
  scope: Scope;
  /** 在当前上下文里是否生效 */
  enabled: boolean;
  /** 没生效的原因：global / topic / parent / 总开关 */
  disabledBy?: string | null;
  files: string[];
}

export interface SkillsOverview {
  enabled: boolean;
  global: SkillEntry[];
  topic: SkillEntry[];
  topicName?: string | null;
}

export interface SkillDirs {
  workspace: string;
  topic?: string | null;
  user?: string | null;
  extra: string[];
}

export interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  cwd?: string | null;
  enabled: boolean;
}

export interface McpEntryView {
  name: string;
  command: string;
  args: string[];
  scope: Scope;
  /** 全局启用状态 */
  enabled: boolean;
  /** 在**当前主题**下是否生效 */
  enabledHere: boolean;
  connected: boolean;
  serverInfo: string;
  toolCount: number;
  tools: string[];
  error?: string | null;
}

/** 连接状态的原始视图（mcp_status / mcp_reload 返回） */
export interface McpStatusEntry {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  connected: boolean;
  serverInfo: string;
  toolCount: number;
  tools: string[];
  error?: string | null;
}

export interface McpOverview {
  global: McpEntryView[];
  topic: McpEntryView[];
  topicName?: string | null;
}

// ---------------------------------------------------------------- 长期记忆

/**
 * 记忆的分类。**不是装饰**：提示词里按类分组注入，模型据此决定怎么用这条记忆。
 * 中文名由后端给（`memory_kinds`），前端不再写一份。
 */
export type MemoryKind = "fact" | "preference" | "goal" | "pitfall" | "style" | "gap";

/** 记忆的作用域：全局（所有主题）或某个主题 */
export type MemoryScope = "global" | "topic";

export const MEMORY_SCOPE_LABEL: Record<MemoryScope, string> = {
  global: "全局",
  topic: "本主题",
};

export interface MemoryItem {
  id: string;
  kind: MemoryKind;
  content: string;
  note: string;
  source?: string | null;
  /** 钉住的永远优先注入（称呼、总目标这类） */
  pinned: boolean;
  scope: MemoryScope;
  topicSlug?: string | null;
  topicName?: string | null;
  /** 从父主题继承来的：本主题里可见，但要改得去父主题 */
  inherited: boolean;
  createdAt: string;
  updatedAt?: string | null;
  lastUsed?: string | null;
  /** 被写进提示词多少次——用来发现「记了从来没起过作用」的条目 */
  useCount: number;
}

export interface MemoryOverview {
  /** 记忆总开关 */
  enabled: boolean;
  topicName?: string | null;
  /** 真正会写进系统提示词的条数 */
  activeCount: number;
  /** 注入提示词的字符数（提示词预览里能看到同样内容） */
  digestChars: number;
  /** 单个作用域的容量上限 */
  limit: number;
  global: MemoryItem[];
  topic: MemoryItem[];
  /** 从父主题继承的（只读展示） */
  inherited: MemoryItem[];
}

export interface MemoryKindInfo {
  id: MemoryKind;
  label: string;
  hint: string;
}

export interface MemoryPaths {
  global: string;
  topic?: string | null;
  limit: number;
}

export interface MemoryInput {
  /** 有 id 是修改，没有是新增 */
  id?: string | null;
  kind?: MemoryKind;
  content: string;
  note?: string;
  source?: string;
  pinned?: boolean;
}

/** hub://memory 事件：记忆被 agent 或别处改动过 */
export interface MemoryEvent {
  scope: string;
  action: string;
}


