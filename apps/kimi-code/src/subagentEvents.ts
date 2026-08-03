import type { TokenUsage } from "./types";

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
      });
    }
    cursor = closingTag.lastIndex;
  }

  return runs;
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
