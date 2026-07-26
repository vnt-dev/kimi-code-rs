export type Role = "user" | "assistant";
export type PermissionMode = "manual" | "auto" | "yolo";

export interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  thinking?: string;
  createdAt: number;
  status?: "streaming" | "done" | "error";
}

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  modelId?: string;
  permissionMode?: PermissionMode;
  messages: ChatMessage[];
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

export interface AuthStatus {
  loggedIn: boolean;
  provider: string;
}

export interface DeviceCode {
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn?: number;
}

export interface Model {
  id: string;
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

export interface ChatStreamEvent {
  conversationId: string;
  kind: "text" | "thinking";
  content: string;
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

export interface ApprovalPayload {
  toolName: string;
  action: string;
  display: CommandDisplay | GenericToolDisplay;
}

export interface AgentInteraction {
  id: string;
  kind: "approval" | "question" | "user_tool";
  payload: ApprovalPayload | Record<string, unknown>;
  createdAt: number;
}

export interface AgentInteractionsEvent {
  conversationId: string;
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

export interface AgentContextUsageEvent {
  conversationId: string;
  usage: ContextUsage;
}
