export type Role = "user" | "assistant";

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
