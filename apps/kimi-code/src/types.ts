export type Role = "user" | "assistant" | "tool" | "system";
export type PermissionMode = "manual" | "auto" | "yolo";

export type MessageMediaSource =
  | { kind: "url"; url: string }
  | { kind: "base64"; media_type: string; data: string }
  | { kind: "file"; file_id: string };

export type MessageContent =
  | { type: "text"; text: string }
  | {
      type: "tool_use";
      tool_call_id: string;
      tool_name: string;
      input: unknown;
    }
  | {
      type: "tool_result";
      tool_call_id: string;
      output: unknown;
      is_error?: boolean;
    }
  | { type: "image"; source: MessageMediaSource }
  | { type: "audio"; source: MessageMediaSource }
  | { type: "video"; source: MessageMediaSource }
  | {
      type: "file";
      file_id: string;
      name: string;
      media_type: string;
      size: number;
    }
  | { type: "thinking"; thinking: string; signature?: string };

export interface ProtocolMessage {
  id: string;
  role: Role;
  session_id: string;
  content: MessageContent[];
  created_at: string;
  prompt_id?: string;
  parent_message_id?: string;
  metadata?: Record<string, unknown>;
}

export interface MessagePage {
  items: ProtocolMessage[];
  has_more: boolean;
}

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  modelId?: string;
  thinkingLevel?: string;
  permissionMode?: PermissionMode;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  accent: string;
  expanded: boolean;
  conversations: Conversation[];
}

export interface DesktopState {
  projects: Project[];
  activeProjectId?: string;
  activeConversationId?: string;
}

export interface Workspace {
  id: string;
  root: string;
  name: string;
  createdAt: number;
  lastOpenedAt: number;
}

export interface SessionSummary {
  id: string;
  workspaceId: string;
  cwd?: string;
  title?: string;
  lastPrompt?: string;
  createdAt: number;
  updatedAt: number;
  archived: boolean;
  custom?: Record<string, unknown>;
}

export interface PreparedSession {
  sessionId: string;
  agentId: string;
  model: string;
  thinkingLevel: string;
  permissionMode: PermissionMode;
}

export interface SkillDescriptor {
  name: string;
  description: string;
  source: "project" | "user" | "extra" | "builtin";
}

export interface SkillContent extends SkillDescriptor {
  path: string;
  content: string;
}

export interface AgentConfig {
  modelAlias?: string;
  thinkingLevel: string;
}

export interface PlanData {
  id: string;
  content: string;
  path: string;
}

export type GoalStatus = "active" | "paused" | "blocked" | "complete";

export interface GoalBudgetReport {
  tokenBudget: number | null;
  turnBudget: number | null;
  wallClockBudgetMs: number | null;
  remainingTokens: number | null;
  remainingTurns: number | null;
  remainingWallClockMs: number | null;
  tokenBudgetReached: boolean;
  turnBudgetReached: boolean;
  wallClockBudgetReached: boolean;
  overBudget: boolean;
}

export interface GoalSnapshot {
  goalId: string;
  objective: string;
  completionCriterion?: string;
  status: GoalStatus;
  turnsUsed: number;
  tokensUsed: number;
  wallClockMs: number;
  budget: GoalBudgetReport;
  terminalReason?: string;
}

export interface GoalToolResult {
  goal: GoalSnapshot | null;
}

export type TodoStatus = "pending" | "in_progress" | "done";

export interface TodoItem {
  title: string;
  status: TodoStatus;
}

export type AgentTaskStatus =
  | "running"
  | "completed"
  | "failed"
  | "timed_out"
  | "killed"
  | "lost";

export interface AgentTaskInfo {
  taskId: string;
  description: string;
  status: AgentTaskStatus;
  kind: string;
  detached?: boolean;
  startedAt: number;
  endedAt?: number | null;
  stopReason?: string;
  timeoutMs?: number;
  command?: string;
  pid?: number;
  exitCode?: number | null;
  agentId?: string;
  subagentType?: string;
  questionCount?: number;
  toolCallId?: string;
}

export interface BackgroundTaskView extends AgentTaskInfo {
  output?: string;
  outputLoading?: boolean;
  outputError?: string;
}

export interface AuthStatus {
  loggedIn: boolean;
  provider: string;
}

export interface ManagedUsageRow {
  label: string;
  used: number;
  limit: number;
  resetHint?: string;
}

export interface BoosterWalletInfo {
  balanceCents: number;
  totalCents: number;
  monthlyChargeLimitEnabled: boolean;
  monthlyChargeLimitCents: number;
  monthlyUsedCents: number;
  currency: string;
}

export interface AccountUsage {
  summary: ManagedUsageRow | null;
  limits: ManagedUsageRow[];
  extraUsage: BoosterWalletInfo | null;
}

export interface ManagedUserInfoPhone {
  countryCode: string;
  number: string;
}

export interface ManagedUserInfo {
  userId: string;
  nickname: string;
  status: string;
  region: string;
  userLevel: number;
  userLevelName: string;
  domain: number;
  domainName: string;
  globalId?: string;
  bio?: string;
  avatar?: string;
  username?: string;
  email?: string;
  phone?: ManagedUserInfoPhone;
  createdTime?: string;
  lastLoginTime?: string;
}

export type AccountProfile = Pick<
  ManagedUserInfo,
  | "userId"
  | "nickname"
  | "userLevel"
  | "userLevelName"
  | "avatar"
  | "username"
>;

export interface DeviceCode {
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn?: number;
}

export interface Model {
  id: string;
  model: string;
  providerId: string;
  isDefault: boolean;
  displayName: string;
  contextLength: number;
  supportsReasoning: boolean;
  supportsImage: boolean;
  supportsVideo: boolean;
  supportsTools: boolean;
  protocol: string;
  supportEfforts: string[];
  defaultEffort?: string;
}

export interface ToolUpdate {
  kind: "stdout" | "stderr" | "progress" | "status" | "custom";
  text?: string;
  percent?: number;
  customKind?: string;
  customData?: unknown;
}

export type AgentContentPart =
  | { type: "text"; text: string }
  | { type: "think"; think: string; encrypted?: string }
  | { type: "image_url"; imageUrl: { url: string; id?: string } }
  | { type: "audio_url"; audioUrl: { url: string; id?: string } }
  | { type: "video_url"; videoUrl: { url: string; id?: string } };

export type AgentPromptPart = Extract<
  AgentContentPart,
  { type: "text" | "image_url" | "audio_url" | "video_url" }
> | {
  type: "file";
  file_id: string;
  name: string;
  media_type: string;
  size: number;
};

export interface TurnFileChange {
  path: string;
  change: "created" | "modified" | "deleted";
  additions?: number;
  deletions?: number;
}

export type AgentChatEvent =
  | {
      type: "prompt.steered";
      activePromptId: string;
      promptIds: string[];
      userMessages?: LiveUserMessage[];
    }
  | {
      type: "turn.started";
      turnId: number;
      origin: unknown;
      prompt?: string;
      userMessage?: LiveUserMessage;
    }
  | {
      type: "turn.ended";
      turnId: number;
      reason: "completed" | "cancelled" | "failed" | "blocked";
      error?: unknown;
      durationMs?: number;
    }
  | {
      type: "turn.files.changed";
      turnId: number;
      files: TurnFileChange[];
    }
  | {
      type: "turn.step.started";
      turnId: number;
      step: number;
      stepId?: string;
    }
  | {
      type: "turn.step.completed";
      turnId: number;
      step: number;
      stepId?: string;
      usage?: unknown;
      finishReason?: string;
      providerFinishReason?: unknown;
      rawFinishReason?: string;
    }
  | {
      type: "turn.step.interrupted";
      turnId: number;
      step: number;
      stepId?: string;
      reason: string;
      message?: string;
    }
  | {
      type: "turn.step.retrying";
      turnId: number;
      step: number;
      stepId?: string;
      failedAttempt: number;
      nextAttempt: number;
      maxAttempts: number;
      delayMs: number;
      errorName?: string;
      errorMessage?: string;
      statusCode?: number;
    }
  | { type: "assistant.delta"; turnId: number; delta: string }
  | {
      type: "assistant.content";
      turnId: number;
      content: AgentContentPart;
    }
  | { type: "thinking.delta"; turnId: number; delta: string }
  | {
      type: "tool.call.delta";
      turnId: number;
      toolCallId: string;
      name?: string;
      argumentsPart?: string;
    }
  | {
      type: "tool.call.started";
      turnId: number;
      toolCallId: string;
      name: string;
      args: unknown;
      description?: string;
      display?: unknown;
    }
  | {
      type: "tool.progress";
      turnId: number;
      toolCallId: string;
      update: ToolUpdate;
    }
  | {
      type: "tool.result";
      turnId: number;
      toolCallId: string;
      output: unknown;
      isError?: boolean;
      synthetic?: boolean;
    };

export interface LiveUserMessage {
  promptId: string;
  userMessageId: string;
  createdAt: string;
  content: MessageContent[];
  origin?: unknown;
}

export interface PromptSubmittedEvent extends LiveUserMessage {
  type: "prompt.submitted";
  status: "queued";
}

export interface AgentChatEventEnvelope {
  sessionId: string;
  agentId: string;
  event: { type: string; [key: string]: unknown };
}

export interface CommandDisplay {
  kind: "command";
  command: string;
  cwd?: string;
  description?: string;
  language?: string;
}

export interface GenericToolDisplay {
  kind: string;
  [key: string]: unknown;
}

export interface PlanReviewOption {
  label: string;
  description: string;
}

export interface PlanReviewDisplay {
  kind: "plan_review";
  plan: string;
  path?: string;
  options?: PlanReviewOption[];
}

export interface ApprovalPayload {
  toolName: string;
  action: string;
  display: CommandDisplay | PlanReviewDisplay | GenericToolDisplay;
}

export interface QuestionOption {
  label: string;
  description?: string;
}

export interface QuestionItem {
  question: string;
  header?: string;
  body?: string;
  options: QuestionOption[];
  multiSelect?: boolean;
  otherLabel?: string;
  otherDescription?: string;
}

export interface QuestionPayload {
  id?: string;
  turnId?: number;
  toolCallId?: string;
  presentation?: "retry_confirmation";
  questions: QuestionItem[];
}

export interface QuestionResponse {
  answers: Record<string, string>;
  method?: "enter" | "space" | "number_key";
}

export interface AgentInteraction {
  id: string;
  kind: "approval" | "question" | "user_tool";
  payload: ApprovalPayload | QuestionPayload | Record<string, unknown>;
  createdAt: number;
}

export interface AgentInteractionsEvent {
  sessionId: string;
  interactions: AgentInteraction[];
}

export interface CompactionEvent {
  phase: "started" | "completed" | "cancelled";
  trigger?: "manual" | "auto";
  compactedCount?: number;
  tokensBefore?: number;
  tokensAfter?: number;
}

export interface AgentCompactionEvent {
  conversationId: string;
  event: CompactionEvent;
}

export interface ContextUsage {
  contextTokens: number;
  measuredTokens: number;
  estimatedTokens: number;
  maxContextTokens: number;
  usageRatio: number;
}

export interface TokenUsage {
  inputOther: number;
  output: number;
  inputCacheRead: number;
  inputCacheCreation: number;
}

export interface AgentUsageStatus {
  byModel?: Record<string, TokenUsage>;
  total?: TokenUsage;
  currentTurn?: TokenUsage;
}

export interface AgentContextUsageEvent {
  conversationId: string;
  usage: ContextUsage;
}
