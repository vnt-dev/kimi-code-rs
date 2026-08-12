import type { ProtocolMessage, TokenUsage } from "./types";

export type SubagentRunStatus =
  | "queued"
  | "running"
  | "suspended"
  | "completed"
  | "failed";

export interface SubagentRun {
  subagentId: string;
  subagentName: string;
  description?: string;
  swarmIndex?: number;
  runInBackground: boolean;
  status: SubagentRunStatus;
  resultSummary?: string;
  error?: string;
  usage?: TokenUsage;
  contextTokens?: number;
  historyPrompt?: string;
  historyOccurrenceFromEnd?: number;
  historyAvailable?: boolean;
  historyNextPrompt?: string;
}

export type SubagentEvent =
  | {
      type: "subagent.spawned";
      subagentId: string;
      subagentName: string;
      parentToolCallId: string;
      parentToolCallUuid?: string;
      parentAgentId?: string;
      callerAgentId?: string;
      description?: string;
      swarmIndex?: number;
      runInBackground: boolean;
    }
  | {
      type: "subagent.started";
      subagentId: string;
    }
  | {
      type: "subagent.suspended";
      subagentId: string;
      reason: string;
    }
  | {
      type: "subagent.completed";
      subagentId: string;
      resultSummary: string;
      usage?: TokenUsage;
      contextTokens?: number;
    }
  | {
      type: "subagent.failed";
      subagentId: string;
      error: string;
    };

export type SubagentRunsByTool = Record<string, SubagentRun[]>;
export type SessionSubagentRuns = Record<string, SubagentRunsByTool>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object";
}

function decodeXmlAttribute(value: string): string {
  return value.replace(
    /&(amp|quot|lt|gt);/g,
    (encoded, entity: string) => {
      switch (entity) {
        case "amp":
          return "&";
        case "quot":
          return '"';
        case "lt":
          return "<";
        case "gt":
          return ">";
        default:
          return encoded;
      }
    },
  );
}

function agentSwarmSubagentName(
  input: unknown,
  mode: string | undefined,
): string {
  if (mode === "resume") return "agent";
  if (!isRecord(input)) return "coder";
  const subagentType = input.subagent_type;
  return typeof subagentType === "string" && subagentType.trim()
    ? subagentType.trim()
    : "coder";
}

function normalizedText(value: string): string {
  return value.replace(/\r\n/g, "\n").trim();
}

function matchesHistoricalPrompt(messageText: string, prompt: string): boolean {
  const normalizedMessage = normalizedText(messageText);
  const normalizedPrompt = normalizedText(prompt);
  return (
    normalizedMessage === normalizedPrompt ||
    normalizedMessage.endsWith(`\n\n${normalizedPrompt}`)
  );
}

function protocolMessageText(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<(typeof message.content)[number], { type: "text" }> =>
        part.type === "text",
    )
    .map((part) => part.text)
    .join("");
}

function agentSwarmPrompts(input: unknown): string[] {
  if (!isRecord(input)) return [];
  const prompts: string[] = [];
  const resumeAgentIds = input.resume_agent_ids;
  if (isRecord(resumeAgentIds)) {
    for (const prompt of Object.values(resumeAgentIds)) {
      if (typeof prompt === "string") prompts.push(prompt.trim());
    }
  }
  const template = input.prompt_template;
  const items = input.items;
  if (typeof template === "string" && Array.isArray(items)) {
    for (const item of items) {
      if (typeof item === "string") {
        prompts.push(template.trim().replaceAll("{{item}}", item.trim()));
      }
    }
  }
  return prompts;
}

function agentToolInputData(input: unknown): {
  prompt: string;
  description?: string;
  subagentName: string;
  runInBackground: boolean;
} | undefined {
  if (!isRecord(input) || typeof input.prompt !== "string") return undefined;
  const resume = typeof input.resume === "string" && input.resume.trim();
  const subagentType =
    typeof input.subagent_type === "string" && input.subagent_type.trim();
  return {
    prompt: input.prompt,
    description:
      typeof input.description === "string" ? input.description : undefined,
    subagentName: resume ? "agent" : subagentType || "coder",
    runInBackground: input.run_in_background === true,
  };
}

export function parseAgentResult(
  output: unknown,
  toolInput: unknown,
): SubagentRun[] {
  if (typeof output !== "string") return [];
  const input = agentToolInputData(toolInput);
  if (!input) return [];
  const agentId = output.match(/^agent_id:\s*(\S+)\s*$/m)?.[1];
  if (!agentId) return [];
  const actualType = output.match(/^actual_subagent_type:\s*(.+?)\s*$/m)?.[1];
  const statusText = output.match(/^status:\s*(\S+)\s*$/m)?.[1];
  const summaryMarker = "[summary]";
  const summaryIndex = output.indexOf(summaryMarker);
  const errorMarker = "subagent error:";
  const errorIndex = output.indexOf(errorMarker);
  const completed = statusText === "completed";
  const failed = statusText === "failed";
  const resultSummary = completed && summaryIndex >= 0
    ? output.slice(summaryIndex + summaryMarker.length).replace(/^\r?\n/, "")
    : undefined;
  const error = failed && errorIndex >= 0
    ? output.slice(errorIndex + errorMarker.length).trim()
    : undefined;
  return [{
    subagentId: agentId,
    subagentName: actualType?.trim() || input.subagentName,
    description: input.description,
    runInBackground: input.runInBackground,
    status: failed ? "failed" : completed ? "completed" : "running",
    resultSummary,
    error,
    historyPrompt: input.prompt,
    historyAvailable: true,
  }];
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isOptionalString(value: unknown): value is string | undefined {
  return value === undefined || isString(value);
}

function isOptionalNumber(value: unknown): value is number | undefined {
  return value === undefined || typeof value === "number";
}

function isTokenUsage(value: unknown): value is TokenUsage {
  return (
    isRecord(value) &&
    typeof value.inputOther === "number" &&
    typeof value.output === "number" &&
    typeof value.inputCacheRead === "number" &&
    typeof value.inputCacheCreation === "number"
  );
}

export function isSubagentEvent(
  value: { type: string; [key: string]: unknown },
): value is SubagentEvent {
  switch (value.type) {
    case "subagent.spawned":
      return (
        isString(value.subagentId) &&
        isString(value.subagentName) &&
        isString(value.parentToolCallId) &&
        isOptionalString(value.parentToolCallUuid) &&
        isOptionalString(value.parentAgentId) &&
        isOptionalString(value.callerAgentId) &&
        isOptionalString(value.description) &&
        isOptionalNumber(value.swarmIndex) &&
        typeof value.runInBackground === "boolean"
      );
    case "subagent.started":
      return isString(value.subagentId);
    case "subagent.suspended":
      return isString(value.subagentId) && isString(value.reason);
    case "subagent.completed":
      return (
        isString(value.subagentId) &&
        isString(value.resultSummary) &&
        (value.usage === undefined || isTokenUsage(value.usage)) &&
        isOptionalNumber(value.contextTokens)
      );
    case "subagent.failed":
      return isString(value.subagentId) && isString(value.error);
    default:
      return false;
  }
}

function sortRuns(runs: SubagentRun[]): SubagentRun[] {
  return runs
    .map((run, position) => ({ run, position }))
    .sort(
      (left, right) =>
        (left.run.swarmIndex ?? Number.MAX_SAFE_INTEGER) -
          (right.run.swarmIndex ?? Number.MAX_SAFE_INTEGER) ||
        left.position - right.position,
    )
    .map(({ run }) => run);
}

function mergeRun(
  run: SubagentRun,
  event: Exclude<SubagentEvent, { type: "subagent.spawned" }>,
): SubagentRun {
  switch (event.type) {
    case "subagent.started":
      return {
        ...run,
        status: "running",
        error: undefined,
      };
    case "subagent.suspended":
      return {
        ...run,
        status: "suspended",
        error: event.reason,
      };
    case "subagent.completed":
      return {
        ...run,
        status: "completed",
        resultSummary: event.resultSummary,
        usage: event.usage,
        contextTokens: event.contextTokens,
        error: undefined,
      };
    case "subagent.failed":
      return {
        ...run,
        status: "failed",
        error: event.error,
      };
  }
}

export function mergeSubagentEvent(
  current: SubagentRunsByTool,
  event: SubagentEvent,
): SubagentRunsByTool {
  if (event.type === "subagent.spawned") {
    const toolCallId =
      event.parentToolCallId || event.parentToolCallUuid || "";
    if (!toolCallId) return current;
    const runs = current[toolCallId] ?? [];
    const nextRun: SubagentRun = {
      subagentId: event.subagentId,
      subagentName: event.subagentName,
      description: event.description,
      swarmIndex: event.swarmIndex,
      runInBackground: event.runInBackground,
      status: "queued",
    };
    const index = runs.findIndex(
      (run) => run.subagentId === event.subagentId,
    );
    const nextRuns =
      index < 0
        ? [...runs, nextRun]
        : runs.map((run, runIndex) =>
            runIndex === index ? { ...run, ...nextRun } : run,
          );
    return {
      ...current,
      [toolCallId]: sortRuns(nextRuns),
    };
  }

  for (const [toolCallId, runs] of Object.entries(current)) {
    const index = runs.findIndex(
      (run) => run.subagentId === event.subagentId,
    );
    if (index < 0) continue;
    return {
      ...current,
      [toolCallId]: runs.map((run, runIndex) =>
        runIndex === index ? mergeRun(run, event) : run,
      ),
    };
  }
  return current;
}

export function mergeSessionSubagentEvent(
  current: SessionSubagentRuns,
  sessionId: string,
  event: SubagentEvent,
): SessionSubagentRuns {
  const runs = current[sessionId] ?? {};
  const nextRuns = mergeSubagentEvent(runs, event);
  if (nextRuns === runs) return current;
  return {
    ...current,
    [sessionId]: nextRuns,
  };
}

export function parseAgentSwarmResult(
  output: unknown,
  toolCallId: string,
  toolInput: unknown,
): SubagentRun[] {
  if (typeof output !== "string") return [];
  const envelopeStart = output.indexOf("<agent_swarm_result>");
  const envelopeEnd = output.lastIndexOf("</agent_swarm_result>");
  if (envelopeStart < 0 || envelopeEnd <= envelopeStart) return [];

  const runs: SubagentRun[] = [];
  const prompts = agentSwarmPrompts(toolInput);
  const openingTag = /<subagent\b([^>]*)>/g;
  const closingTag = /<\/subagent>(?=\r?\n(?:<subagent\b|<\/agent_swarm_result>))/g;
  let cursor = envelopeStart + "<agent_swarm_result>".length;

  while (cursor < envelopeEnd) {
    openingTag.lastIndex = cursor;
    const opening = openingTag.exec(output);
    if (!opening || opening.index >= envelopeEnd) break;

    closingTag.lastIndex = openingTag.lastIndex;
    const closing = closingTag.exec(output);
    if (!closing || closing.index > envelopeEnd) break;

    const attributes = new Map<string, string>();
    for (const match of opening[1].matchAll(/([a-z_]+)="([^"]*)"/g)) {
      attributes.set(match[1], decodeXmlAttribute(match[2]));
    }
    const outcome = attributes.get("outcome");
    if (outcome === "completed" || outcome === "failed" || outcome === "aborted") {
      const swarmIndex = runs.length + 1;
      const body = output.slice(openingTag.lastIndex, closing.index);
      const mode = attributes.get("mode");
      const subagentId =
        attributes.get("agent_id") || `${toolCallId}:history:${swarmIndex}`;
      runs.push({
        subagentId,
        subagentName: agentSwarmSubagentName(toolInput, mode),
        description: attributes.get("item"),
        swarmIndex,
        runInBackground: false,
        status: outcome === "completed" ? "completed" : "failed",
        resultSummary: outcome === "completed" ? body : undefined,
        error: outcome === "completed" ? undefined : body,
        ...(prompts[swarmIndex - 1]
          ? { historyPrompt: prompts[swarmIndex - 1] }
          : {}),
        historyAvailable: attributes.has("agent_id"),
      });
    }
    cursor = closingTag.lastIndex;
  }

  return runs;
}

export function collectHistoricalSubagentRuns(
  messages: readonly ProtocolMessage[],
): SubagentRunsByTool {
  const results = new Map<string, unknown>();
  for (const message of messages) {
    for (const part of message.content) {
      if (part.type === "tool_result") {
        results.set(part.tool_call_id, part.output);
      }
    }
  }

  const entries: Array<{ toolCallId: string; runs: SubagentRun[] }> = [];
  for (const message of messages) {
    for (const part of message.content) {
      if (part.type !== "tool_use") continue;
      const output = results.get(part.tool_call_id);
      const runs = part.tool_name === "Agent"
        ? parseAgentResult(output, part.input)
        : part.tool_name === "AgentSwarm"
          ? parseAgentSwarmResult(output, part.tool_call_id, part.input)
          : [];
      if (runs.length > 0) entries.push({ toolCallId: part.tool_call_id, runs });
    }
  }

  const occurrences = new Map<string, number>();
  const previousByAgent = new Map<string, { entryIndex: number; runIndex: number }>();
  for (let entryIndex = 0; entryIndex < entries.length; entryIndex += 1) {
    const entry = entries[entryIndex];
    for (let runIndex = 0; runIndex < entry.runs.length; runIndex += 1) {
      const run = entry.runs[runIndex];
      if (!run.historyPrompt) continue;
      const previous = previousByAgent.get(run.subagentId);
      if (previous) {
        const previousRun = entries[previous.entryIndex].runs[previous.runIndex];
        entries[previous.entryIndex].runs[previous.runIndex] = {
          ...previousRun,
          historyNextPrompt: run.historyPrompt,
        };
      }
      previousByAgent.set(run.subagentId, { entryIndex, runIndex });
    }
  }
  for (let entryIndex = entries.length - 1; entryIndex >= 0; entryIndex -= 1) {
    const entry = entries[entryIndex];
    for (let runIndex = entry.runs.length - 1; runIndex >= 0; runIndex -= 1) {
      const run = entry.runs[runIndex];
      if (!run.historyPrompt) continue;
      const key = `${run.subagentId}\u0000${normalizedText(run.historyPrompt)}`;
      const occurrence = occurrences.get(key) ?? 0;
      entry.runs[runIndex] = {
        ...run,
        historyOccurrenceFromEnd: occurrence,
      };
      occurrences.set(key, occurrence + 1);
    }
  }

  return Object.fromEntries(entries.map(({ toolCallId, runs }) => [toolCallId, runs]));
}

export function subagentInvocationMessages(
  messages: readonly ProtocolMessage[],
  run: SubagentRun,
): ProtocolMessage[] {
  if (!run.historyPrompt) return [];
  const prompt = normalizedText(run.historyPrompt);
  const matches: number[] = [];
  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    if (
      message.role === "user" &&
      matchesHistoricalPrompt(protocolMessageText(message), prompt)
    ) {
      matches.push(index);
    }
  }
  const occurrence = run.historyOccurrenceFromEnd ?? 0;
  const start = matches[matches.length - 1 - occurrence];
  if (start === undefined) return [];

  let end = messages.length;
  const result = run.resultSummary && normalizedText(run.resultSummary);
  if (result) {
    for (let index = start + 1; index < messages.length; index += 1) {
      const message = messages[index];
      if (
        message.role === "assistant" &&
        normalizedText(protocolMessageText(message)) === result
      ) {
        end = index + 1;
        break;
      }
    }
  }
  if (end === messages.length) {
    const nextPrompt = run.historyNextPrompt
      ? normalizedText(run.historyNextPrompt)
      : prompt;
    for (let index = start + 1; index < messages.length; index += 1) {
      const message = messages[index];
      if (
        message.role === "user" &&
        matchesHistoricalPrompt(protocolMessageText(message), nextPrompt)
      ) {
        end = index;
        break;
      }
    }
  }
  return messages.slice(start, end);
}

export function mergeSubagentRuns(
  historicalRuns: readonly SubagentRun[],
  liveRuns: readonly SubagentRun[],
): readonly SubagentRun[] {
  if (historicalRuns.length === 0) return liveRuns;
  if (liveRuns.length === 0) return historicalRuns;

  const liveById = new Map(liveRuns.map((run) => [run.subagentId, run]));
  const merged = historicalRuns.map((historicalRun) => {
    const liveRun = liveById.get(historicalRun.subagentId);
    if (!liveRun) return historicalRun;
    liveById.delete(historicalRun.subagentId);
    return {
      ...historicalRun,
      ...liveRun,
      description: historicalRun.description ?? liveRun.description,
    };
  });
  merged.push(...liveById.values());
  return sortRuns(merged);
}

export function subagentRunsWithSwarmItems(
  runs: readonly SubagentRun[],
  toolInput: unknown,
): readonly SubagentRun[] {
  if (!isRecord(toolInput)) return runs;
  const items = toolInput.items;
  if (!Array.isArray(items)) return runs;
  let changed = false;
  const described = runs.map((run) => {
    if (run.swarmIndex === undefined || run.swarmIndex < 1) return run;
    const item = items[run.swarmIndex - 1];
    if (typeof item !== "string" || !item.trim()) return run;
    changed = true;
    return { ...run, description: item };
  });
  return changed ? described : runs;
}
