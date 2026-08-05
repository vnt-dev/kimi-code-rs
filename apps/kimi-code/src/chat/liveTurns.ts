import type { AgentPromptSubmitStatus } from "../agentRpc";
import { projectLiveUserMessage } from "../liveUserMessage";
import { parseSkillPromptDisplay } from "../prompt/skills";
import type { PluginCommandDisplay } from "./messages";
import { conciseError } from "../utils/errors";
import type {
  AgentChatEvent,
  AgentContentPart,
  LiveUserMessage,
  PromptSubmittedEvent,
  SkillDescriptor,
  ToolUpdate,
  TurnFileChange,
} from "../types";

const MAX_LIVE_TOOL_UPDATES = 50;
export const MAIN_AGENT_ID = "main";

export type LiveTurnStatus =
  | "queued"
  | "running"
  | "completed"
  | "cancelled"
  | "failed"
  | "blocked";

export type LiveStepStatus = "running" | "completed" | "interrupted";

export type LiveBlock =
  | { kind: "text"; content: string }
  | { kind: "thinking"; content: string }
  | { kind: "content"; content: AgentContentPart }
  | {
      kind: "tool";
      toolCallId: string;
      name?: string;
      argumentsText: string;
      input?: unknown;
      description?: string;
      display?: unknown;
      status: "streaming" | "running" | "completed" | "error";
      updates: ToolUpdate[];
      output?: unknown;
      isError?: boolean;
    };

export interface LiveStep {
  step: number;
  stepId?: string;
  status: LiveStepStatus;
  blocks: LiveBlock[];
  finishReason?: string;
  interruption?: string;
}

export interface InFlightTurn {
  promptId?: string;
  userMessageId?: string;
  prompt: string;
  attachments: readonly PromptAttachment[];
  skills: readonly string[];
  steeredPrompts: readonly LiveSteeredPrompt[];
  createdAt: string;
  turnId?: number;
  status: LiveTurnStatus;
  durationMs?: number;
  steps: LiveStep[];
  fileChanges?: readonly TurnFileChange[];
  error?: string;
  historyBoundaryId?: string;
  pluginCommand?: PluginCommandDisplay;
  pluginCommandContent?: string;
}

export interface QueuedAgentChatEvent {
  sessionId: string;
  agentId: string;
  event: AgentChatEvent;
}

export type SubagentLiveTurns = Record<string, Record<string, InFlightTurn>>;

export type PromptAttachmentKind = "image" | "audio" | "video" | "file";

export interface PromptAttachment {
  id: string;
  name: string;
  dataUrl?: string;
  kind: PromptAttachmentKind;
  fileId?: string;
  mediaType: string;
  size: number;
}

export interface QueuedPrompt {
  id: string;
  text: string;
  attachments: readonly PromptAttachment[];
  skills: readonly SkillDescriptor[];
  createdAt: string;
  goalMode?: boolean;
  steering?: boolean;
}

export interface RemoteQueuedPrompt {
  promptId: string;
  userMessageId: string;
  text: string;
  attachments: readonly PromptAttachment[];
  skills: readonly string[];
  createdAt: string;
}

export interface GoalModeChangedEvent {
  sessionId: string;
  enabled: boolean;
}

export interface LiveSteeredPrompt {
  promptId: string;
  message?: QueuedPrompt;
  anchorStepKey?: string;
  afterBlockIndex?: number;
}


export function newInFlightTurn(
  prompt: string,
  attachments: readonly PromptAttachment[],
  historyBoundaryId?: string,
  skills: readonly string[] = [],
): InFlightTurn {
  return {
    prompt,
    attachments,
    skills,
    steeredPrompts: [],
    createdAt: new Date().toISOString(),
    status: "queued",
    steps: [],
    historyBoundaryId,
  };
}

export function inFlightTurnFromUserMessage(message: LiveUserMessage): InFlightTurn {
  const projected = projectLiveUserMessage(message);
  const display = parseSkillPromptDisplay(projected.text);
  return {
    ...newInFlightTurn(
      display.text,
      projected.attachments,
      undefined,
      display.skills,
    ),
    promptId: message.promptId,
    userMessageId: message.userMessageId,
    createdAt: message.createdAt,
    pluginCommand: projected.pluginCommand,
    pluginCommandContent: projected.pluginCommandContent,
  };
}

export function readPromptSubmittedEvent(
  event: { type: string; [key: string]: unknown },
): PromptSubmittedEvent | undefined {
  if (
    event.type !== "prompt.submitted" ||
    event.status !== "queued" ||
    typeof event.promptId !== "string" ||
    typeof event.userMessageId !== "string" ||
    typeof event.createdAt !== "string" ||
    !Array.isArray(event.content)
  ) {
    return undefined;
  }
  return event as unknown as PromptSubmittedEvent;
}

export function isTurnRunning(turn?: InFlightTurn): boolean {
  return turn?.status === "queued" || turn?.status === "running";
}

export function liveStepKey(step: number, stepId?: string): string {
  return stepId ?? `step-${step}`;
}

export function liveTurnStatusFromSubmit(
  status: AgentPromptSubmitStatus,
): LiveTurnStatus {
  switch (status) {
    case "queued":
      return "queued";
    case "running":
    case "steered":
      return "running";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "blocked":
      return "blocked";
  }
}

export function withCurrentStep(
  turn: InFlightTurn,
  update: (step: LiveStep) => LiveStep,
): InFlightTurn {
  const steps =
    turn.steps.length > 0
      ? [...turn.steps]
      : [{ step: 0, status: "running" as const, blocks: [] }];
  const index = steps.length - 1;
  steps[index] = update(steps[index]);
  return { ...turn, steps };
}

export function appendLiveContent(
  turn: InFlightTurn,
  kind: "text" | "thinking",
  content: string,
): InFlightTurn {
  return withCurrentStep(turn, (step) => {
    const blocks = [...step.blocks];
    const last = blocks.at(-1);
    const lastIndex = blocks.length - 1;
    const stepKey = liveStepKey(step.step, step.stepId);
    const hasSteeredBoundary = turn.steeredPrompts.some(
      (item) =>
        item.anchorStepKey === stepKey &&
        item.afterBlockIndex === lastIndex,
    );
    if (last?.kind === kind && !hasSteeredBoundary) {
      blocks[blocks.length - 1] = {
        ...last,
        content: last.content + content,
      };
    } else {
      blocks.push({ kind, content });
    }
    return { ...step, blocks };
  });
}

export function updateLiveTool(
  turn: InFlightTurn,
  toolCallId: string,
  update: (tool: Extract<LiveBlock, { kind: "tool" }>) => LiveBlock,
): InFlightTurn {
  for (
    let stepIndex = turn.steps.length - 1;
    stepIndex >= 0;
    stepIndex -= 1
  ) {
    const step = turn.steps[stepIndex];
    const blockIndex = step.blocks.findIndex(
      (block) => block.kind === "tool" && block.toolCallId === toolCallId,
    );
    if (blockIndex >= 0) {
      const block = step.blocks[blockIndex];
      if (block.kind === "tool") {
        const blocks = [...step.blocks];
        blocks[blockIndex] = update(block);
        const steps = [...turn.steps];
        steps[stepIndex] = { ...step, blocks };
        return { ...turn, steps };
      }
    }
  }

  return withCurrentStep(turn, (step) => ({
    ...step,
    blocks: [
      ...step.blocks,
      update({
        kind: "tool",
        toolCallId,
        argumentsText: "",
        status: "streaming",
        updates: [],
      }),
    ],
  }));
}

export function reduceAgentChatEvent(
  turn: InFlightTurn,
  event: AgentChatEvent,
): InFlightTurn {
  if (event.type === "prompt.steered") {
    const known = new Set(turn.steeredPrompts.map((item) => item.promptId));
    const currentStep = turn.steps.at(-1);
    const placement = currentStep
      ? {
          anchorStepKey: liveStepKey(currentStep.step, currentStep.stepId),
          afterBlockIndex: currentStep.blocks.length - 1,
        }
      : {};
    const additions = event.promptIds
      .filter((promptId) => !known.has(promptId))
      .map((promptId) => ({ promptId, ...placement }));
    return additions.length > 0
      ? {
          ...turn,
          steeredPrompts: [...turn.steeredPrompts, ...additions],
        }
      : turn;
  }
  if (turn.turnId !== undefined && turn.turnId !== event.turnId) return turn;
  const next =
    turn.turnId === undefined ? { ...turn, turnId: event.turnId } : turn;

  switch (event.type) {
    case "turn.started": {
      const projected = event.userMessage
        ? inFlightTurnFromUserMessage(event.userMessage)
        : undefined;
      return {
        ...next,
        ...(projected
          ? {
              promptId: projected.promptId,
              userMessageId: projected.userMessageId,
              prompt: projected.prompt,
              attachments: projected.attachments,
              skills: projected.skills,
              createdAt: projected.createdAt,
              pluginCommand: projected.pluginCommand,
              pluginCommandContent: projected.pluginCommandContent,
            }
          : {}),
        status: "running",
      };
    }
    case "turn.ended":
      return {
        ...next,
        status: event.reason,
        durationMs:
          event.durationMs ??
          Math.max(0, Date.now() - Date.parse(next.createdAt)),
        error:
          event.reason === "failed" && event.error !== undefined
            ? conciseError(
                typeof event.error === "string"
                  ? event.error
                  : JSON.stringify(event.error),
              )
            : next.error,
      };
    case "turn.files.changed":
      return {
        ...next,
        fileChanges: event.files,
      };
    case "turn.step.started": {
      const anchorStepKey = liveStepKey(event.step, event.stepId);
      const steeredPrompts = next.steeredPrompts.map((item) =>
        item.anchorStepKey === undefined
          ? { ...item, anchorStepKey, afterBlockIndex: -1 }
          : item,
      );
      const index = next.steps.findIndex(
        (step) =>
          (event.stepId && step.stepId === event.stepId) ||
          step.step === event.step,
      );
      if (index < 0) {
        return {
          ...next,
          status: "running",
          steeredPrompts,
          steps: [
            ...next.steps,
            {
              step: event.step,
              stepId: event.stepId,
              status: "running",
              blocks: [],
            },
          ],
        };
      }
      const steps = [...next.steps];
      steps[index] = { ...steps[index], status: "running" };
      return { ...next, status: "running", steeredPrompts, steps };
    }
    case "turn.step.completed": {
      const steps = next.steps.map((step) =>
        step.step === event.step
          ? {
              ...step,
              status: "completed" as const,
              finishReason: event.finishReason,
            }
          : step,
      );
      return { ...next, steps };
    }
    case "turn.step.interrupted": {
      const steps = next.steps.map((step) =>
        step.step === event.step
          ? {
              ...step,
              status: "interrupted" as const,
              interruption: event.message ?? event.reason,
            }
          : step,
      );
      return { ...next, steps };
    }
    case "assistant.delta":
      return appendLiveContent(next, "text", event.delta);
    case "assistant.content":
      return withCurrentStep(next, (step) => ({
        ...step,
        blocks: [...step.blocks, { kind: "content", content: event.content }],
      }));
    case "thinking.delta":
      return appendLiveContent(next, "thinking", event.delta);
    case "tool.call.delta":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        name: event.name ?? tool.name,
        argumentsText: tool.argumentsText + (event.argumentsPart ?? ""),
      }));
    case "tool.call.started":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        name: event.name,
        input: event.args,
        description: event.description,
        display: event.display,
        status: "running",
      }));
    case "tool.progress":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        status: "running",
        updates: [
          ...tool.updates.slice(-(MAX_LIVE_TOOL_UPDATES - 1)),
          event.update,
        ],
      }));
    case "tool.result":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        status: event.isError ? "error" : "completed",
        output: event.output,
        isError: event.isError,
      }));
  }
}

export function appendQueuedAgentChatEvent(
  events: AgentChatEvent[],
  event: AgentChatEvent,
): void {
  const previous = events.at(-1);
  if (
    previous?.type === "assistant.delta" &&
    event.type === "assistant.delta" &&
    previous.turnId === event.turnId
  ) {
    events[events.length - 1] = {
      ...event,
      delta: previous.delta + event.delta,
    };
    return;
  }
  if (
    previous?.type === "thinking.delta" &&
    event.type === "thinking.delta" &&
    previous.turnId === event.turnId
  ) {
    events[events.length - 1] = {
      ...event,
      delta: previous.delta + event.delta,
    };
    return;
  }
  if (
    previous?.type === "tool.call.delta" &&
    event.type === "tool.call.delta" &&
    previous.turnId === event.turnId &&
    previous.toolCallId === event.toolCallId
  ) {
    events[events.length - 1] = {
      ...event,
      name: event.name ?? previous.name,
      argumentsPart:
        (previous.argumentsPart ?? "") + (event.argumentsPart ?? ""),
    };
    return;
  }
  events.push(event);
}

export function reduceQueuedAgentChatEvents(
  current: Record<string, InFlightTurn>,
  queue: QueuedAgentChatEvent[],
): Record<string, InFlightTurn> {
  const eventsBySession = new Map<string, AgentChatEvent[]>();
  for (const queued of queue) {
    const events = eventsBySession.get(queued.sessionId) ?? [];
    appendQueuedAgentChatEvent(events, queued.event);
    eventsBySession.set(queued.sessionId, events);
  }

  let next = current;
  for (const [sessionId, events] of eventsBySession) {
    const turn = current[sessionId];
    let reduced = turn;
    for (const event of events) {
      if (!reduced) {
        if (event.type !== "turn.started") continue;
        reduced = newSubagentTurn(event);
      }
      if (
        event.type === "turn.started" &&
        reduced.turnId !== undefined &&
        reduced.turnId !== event.turnId &&
        !isTurnRunning(reduced)
      ) {
        reduced = newSubagentTurn(event);
      }
      reduced = reduceAgentChatEvent(reduced, event);
    }
    if (!reduced) continue;
    if (reduced === turn) continue;
    if (next === current) next = { ...current };
    next[sessionId] = reduced;
  }
  return next;
}

export function newSubagentTurn(event: AgentChatEvent): InFlightTurn {
  if (event.type === "turn.started" && event.userMessage) {
    return inFlightTurnFromUserMessage(event.userMessage);
  }
  return newInFlightTurn(
    event.type === "turn.started" ? event.prompt ?? "" : "",
    [],
  );
}

export function reduceQueuedSubagentChatEvents(
  current: SubagentLiveTurns,
  queue: QueuedAgentChatEvent[],
): SubagentLiveTurns {
  const grouped = new Map<
    string,
    {
      sessionId: string;
      agentId: string;
      events: AgentChatEvent[];
    }
  >();
  for (const queued of queue) {
    const key = `${queued.sessionId}\u0000${queued.agentId}`;
    const entry = grouped.get(key) ?? {
      sessionId: queued.sessionId,
      agentId: queued.agentId,
      events: [],
    };
    appendQueuedAgentChatEvent(entry.events, queued.event);
    grouped.set(key, entry);
  }

  let next = current;
  for (const { sessionId, agentId, events } of grouped.values()) {
    const sessionTurns = next[sessionId] ?? {};
    const previous = sessionTurns[agentId];
    let reduced = previous;
    for (const event of events) {
      if (
        !reduced ||
        (event.type === "turn.started" &&
          reduced.turnId !== undefined &&
          reduced.turnId !== event.turnId)
      ) {
        reduced = newSubagentTurn(event);
      }
      reduced = reduceAgentChatEvent(reduced, event);
    }
    if (!reduced || reduced === previous) continue;
    if (next === current) next = { ...current };
    next[sessionId] = {
      ...sessionTurns,
      [agentId]: reduced,
    };
  }
  return next;
}
