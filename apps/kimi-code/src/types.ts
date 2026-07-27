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

export type AgentChatEvent =
  | {
      type: "turn_started";
      turnId: number;
      origin: unknown;
      prompt?: string;
    }
  | {
      type: "turn_ended";
      turnId: number;
      reason: "completed" | "cancelled" | "failed" | "blocked";
      error?: unknown;
      durationMs?: number;
    }
  | {
      type: "step_started";
      turnId: number;
      step: number;
      stepId?: string;
    }
  | {
      type: "step_completed";
      turnId: number;
      step: number;
      stepId?: string;
      usage?: unknown;
      finishReason?: string;
      providerFinishReason?: unknown;
      rawFinishReason?: string;
    }
  | {
      type: "step_interrupted";
      turnId: number;
      step: number;
      stepId?: string;
      reason: string;
      message?: string;
    }
  | { type: "assistant_delta"; turnId: number; delta: string }
  | {
      type: "assistant_content";
      turnId: number;
      content: AgentContentPart;
    }
  | { type: "thinking_delta"; turnId: number; delta: string }
  | {
      type: "tool_call_delta";
      turnId: number;
      toolCallId: string;
      name?: string;
      argumentsPart?: string;
    }
  | {
      type: "tool_call_started";
      turnId: number;
      toolCallId: string;
      name: string;
      args: unknown;
      description?: string;
      display?: unknown;
    }
  | {
      type: "tool_progress";
      turnId: number;
      toolCallId: string;
      update: ToolUpdate;
    }
  | {
      type: "tool_result";
      turnId: number;
      toolCallId: string;
      output: unknown;
      isError?: boolean;
      synthetic?: boolean;
    };

export interface AgentChatEventEnvelope {
  conversationId: string;
  event: AgentChatEvent;
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
