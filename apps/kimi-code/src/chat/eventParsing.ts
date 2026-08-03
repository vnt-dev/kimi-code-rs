import type { AgentChatEvent, AgentTaskInfo, TodoItem } from "../types";

const CHAT_EVENT_TYPES = new Set([
  "prompt.steered",
  "turn.started",
  "turn.files.changed",
  "turn.ended",
  "turn.step.started",
  "turn.step.completed",
  "turn.step.interrupted",
  "assistant.delta",
  "assistant.content",
  "thinking.delta",
  "tool.call.delta",
  "tool.call.started",
  "tool.progress",
  "tool.result",
]);

export function isAgentChatEvent(event: { type: string }): event is AgentChatEvent {
  return CHAT_EVENT_TYPES.has(event.type);
}

export function readTodoItems(value: unknown): TodoItem[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const todos: TodoItem[] = [];
  for (const item of value) {
    if (
      !item ||
      typeof item !== "object" ||
      !("title" in item) ||
      typeof item.title !== "string" ||
      !("status" in item) ||
      !["pending", "in_progress", "done"].includes(String(item.status))
    ) {
      return undefined;
    }
    todos.push({
      title: item.title,
      status: item.status as TodoItem["status"],
    });
  }
  return todos;
}

const AGENT_TASK_STATUSES = new Set([
  "running",
  "completed",
  "failed",
  "timed_out",
  "killed",
  "lost",
]);

export function readAgentTaskInfo(
  value: unknown,
  fallbackStatus?: AgentTaskInfo["status"],
): AgentTaskInfo | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  if (
    typeof record.taskId !== "string" ||
    typeof record.kind !== "string"
  ) {
    return undefined;
  }
  const status = AGENT_TASK_STATUSES.has(String(record.status))
    ? (record.status as AgentTaskInfo["status"])
    : fallbackStatus;
  if (!status) return undefined;

  return {
    ...(record as unknown as AgentTaskInfo),
    taskId: record.taskId,
    kind: record.kind,
    status,
    description:
      typeof record.description === "string"
        ? record.description
        : typeof record.command === "string"
          ? record.command
          : record.taskId,
    startedAt:
      typeof record.startedAt === "number" ? record.startedAt : Date.now(),
  };
}

export function isTaskLifecycleEventType(type: string): boolean {
  return (
    type === "task.started" ||
    type === "task.terminated" ||
    type === "background.task.started" ||
    type === "background.task.terminated"
  );
}
