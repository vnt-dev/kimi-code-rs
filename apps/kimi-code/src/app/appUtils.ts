import { invoke } from "../transport";
import type { MessagePage, ProtocolMessage } from "../types";

export const MAX_PROMPT_SKILLS = 8;
export const LIVE_TURN_HANDOFF_MS = 200;
export const BACKGROUND_TASK_LIST_LIMIT = 50;
export const BACKGROUND_TASK_OUTPUT_TAIL = 16_384;
export const BACKGROUND_TASK_DETAIL_TAIL = 65_536;

export interface ConversationHistory {
  conversationId: string;
  items: ProtocolMessage[];
  loading: boolean;
  error?: string;
}

export interface AgentSubscription {
  agentId: string;
  subscriptionId: string;
}

export interface PendingAgentSubscription {
  agentId: string;
  promise: Promise<string>;
}

export function newQueuedPromptId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function fetchConversationHistory(
  conversationId: string,
): Promise<MessagePage> {
  return invoke<MessagePage>("list_conversation_messages", {
    sessionId: conversationId,
  });
}

export function omitSessionKeys<T>(
  current: Record<string, T>,
  sessionIds: ReadonlySet<string>,
): Record<string, T> {
  let changed = false;
  const next = { ...current };
  for (const sessionId of sessionIds) {
    if (!(sessionId in next)) continue;
    delete next[sessionId];
    changed = true;
  }
  return changed ? next : current;
}
