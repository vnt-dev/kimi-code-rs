import type { ProtocolMessage } from "../types";
import type { InFlightTurn } from "./liveTurns";

const MAX_LCS_CELLS = 250_000;

export type ToolFileOperation =
  | {
      kind: "edit";
      before: string;
      after: string;
      replaceAll: boolean;
      additions: number;
      deletions: number;
      lines: readonly ToolDiffLine[];
    }
  | {
      kind: "write";
      content: string;
      mode: "overwrite" | "append";
      additions: number;
      deletions: 0;
    };

export interface ToolFileChange {
  path: string;
  additions: number;
  deletions: number;
  operations: readonly ToolFileOperation[];
}

export type ToolDiffLine =
  | {
      kind: "context";
      text: string;
      oldLine: number;
      newLine: number;
    }
  | {
      kind: "removed";
      text: string;
      oldLine: number;
    }
  | {
      kind: "added";
      text: string;
      newLine: number;
    };

interface SuccessfulToolCall {
  name: string;
  input: unknown;
}

interface ToolResultLike {
  is_error?: boolean;
}

function textLines(text: string): string[] {
  if (text.length === 0) return [];
  const lines = text.replaceAll("\r\n", "\n").split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

export function normalizeToolFilePath(path: string): string {
  let normalized = path.replaceAll("\\", "/");
  while (normalized.startsWith("./")) normalized = normalized.slice(2);
  return normalized;
}

function fallbackLineDiff(before: string[], after: string[]): ToolDiffLine[] {
  let prefix = 0;
  while (
    prefix < before.length &&
    prefix < after.length &&
    before[prefix] === after[prefix]
  ) {
    prefix += 1;
  }

  let suffix = 0;
  while (
    suffix < before.length - prefix &&
    suffix < after.length - prefix &&
    before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
  ) {
    suffix += 1;
  }

  const lines: ToolDiffLine[] = [];
  for (let index = 0; index < prefix; index += 1) {
    lines.push({
      kind: "context",
      text: before[index],
      oldLine: index + 1,
      newLine: index + 1,
    });
  }
  for (let index = prefix; index < before.length - suffix; index += 1) {
    lines.push({ kind: "removed", text: before[index], oldLine: index + 1 });
  }
  for (let index = prefix; index < after.length - suffix; index += 1) {
    lines.push({ kind: "added", text: after[index], newLine: index + 1 });
  }
  for (let index = suffix; index > 0; index -= 1) {
    const oldIndex = before.length - index;
    const newIndex = after.length - index;
    lines.push({
      kind: "context",
      text: before[oldIndex],
      oldLine: oldIndex + 1,
      newLine: newIndex + 1,
    });
  }
  return lines;
}

export function buildToolLineDiff(
  beforeText: string,
  afterText: string,
): ToolDiffLine[] {
  const before = textLines(beforeText);
  const after = textLines(afterText);
  if (before.length * after.length > MAX_LCS_CELLS) {
    return fallbackLineDiff(before, after);
  }

  const table = Array.from(
    { length: before.length + 1 },
    () => new Uint32Array(after.length + 1),
  );
  for (let oldIndex = before.length - 1; oldIndex >= 0; oldIndex -= 1) {
    for (let newIndex = after.length - 1; newIndex >= 0; newIndex -= 1) {
      table[oldIndex][newIndex] =
        before[oldIndex] === after[newIndex]
          ? table[oldIndex + 1][newIndex + 1] + 1
          : Math.max(
              table[oldIndex + 1][newIndex],
              table[oldIndex][newIndex + 1],
            );
    }
  }

  const lines: ToolDiffLine[] = [];
  let oldIndex = 0;
  let newIndex = 0;
  while (oldIndex < before.length || newIndex < after.length) {
    if (
      oldIndex < before.length &&
      newIndex < after.length &&
      before[oldIndex] === after[newIndex]
    ) {
      lines.push({
        kind: "context",
        text: before[oldIndex],
        oldLine: oldIndex + 1,
        newLine: newIndex + 1,
      });
      oldIndex += 1;
      newIndex += 1;
      continue;
    }
    if (
      oldIndex < before.length &&
      (newIndex >= after.length ||
        table[oldIndex + 1][newIndex] >= table[oldIndex][newIndex + 1])
    ) {
      lines.push({
        kind: "removed",
        text: before[oldIndex],
        oldLine: oldIndex + 1,
      });
      oldIndex += 1;
      continue;
    }
    lines.push({
      kind: "added",
      text: after[newIndex],
      newLine: newIndex + 1,
    });
    newIndex += 1;
  }
  return lines;
}

function parseToolFileOperation(call: SuccessfulToolCall):
  | { path: string; operation: ToolFileOperation }
  | undefined {
  if (
    !call.input ||
    typeof call.input !== "object" ||
    Array.isArray(call.input)
  ) {
    return undefined;
  }
  const input = call.input as Record<string, unknown>;
  if (typeof input.path !== "string" || input.path.trim().length === 0) {
    return undefined;
  }
  const path = normalizeToolFilePath(input.path);
  if (path.trim().length === 0) return undefined;

  if (call.name === "Edit") {
    if (
      typeof input.old_string !== "string" ||
      typeof input.new_string !== "string" ||
      input.old_string === input.new_string
    ) {
      return undefined;
    }
    const lines = buildToolLineDiff(input.old_string, input.new_string);
    return {
      path,
      operation: {
        kind: "edit",
        before: input.old_string,
        after: input.new_string,
        replaceAll: input.replace_all === true,
        additions: lines.filter((line) => line.kind === "added").length,
        deletions: lines.filter((line) => line.kind === "removed").length,
        lines,
      },
    };
  }

  if (call.name === "Write" && typeof input.content === "string") {
    const mode = input.mode === "append" ? "append" : "overwrite";
    return {
      path,
      operation: {
        kind: "write",
        content: input.content,
        mode,
        additions: textLines(input.content).length,
        deletions: 0,
      },
    };
  }
  return undefined;
}

export function toolFileChangesFromSuccessfulCalls(
  calls: readonly SuccessfulToolCall[],
): ToolFileChange[] {
  const grouped = new Map<
    string,
    {
      path: string;
      additions: number;
      deletions: number;
      operations: ToolFileOperation[];
    }
  >();
  for (const call of calls) {
    const parsed = parseToolFileOperation(call);
    if (!parsed) continue;
    const current = grouped.get(parsed.path) ?? {
      path: parsed.path,
      additions: 0,
      deletions: 0,
      operations: [],
    };
    current.additions += parsed.operation.additions;
    current.deletions += parsed.operation.deletions;
    current.operations.push(parsed.operation);
    grouped.set(parsed.path, current);
  }
  return [...grouped.values()];
}

export function historyTurnFileChanges(
  messages: readonly ProtocolMessage[],
  results: ReadonlyMap<string, ToolResultLike>,
): ToolFileChange[] {
  const calls: SuccessfulToolCall[] = [];
  for (const message of messages) {
    for (const part of message.content) {
      if (part.type !== "tool_use") continue;
      const result = results.get(part.tool_call_id);
      if (!result || result.is_error === true) continue;
      calls.push({ name: part.tool_name, input: part.input });
    }
  }
  return toolFileChangesFromSuccessfulCalls(calls);
}

export function liveTurnFileChanges(turn: InFlightTurn): ToolFileChange[] {
  const calls: SuccessfulToolCall[] = [];
  for (const step of turn.steps) {
    for (const block of step.blocks) {
      if (
        block.kind !== "tool" ||
        block.status !== "completed" ||
        block.isError === true
      ) {
        continue;
      }
      calls.push({ name: block.name ?? "", input: block.input });
    }
  }
  return toolFileChangesFromSuccessfulCalls(calls);
}

export function liveTurnFileChangeRevision(turn: InFlightTurn): string {
  const completedCalls: string[] = [];
  for (const step of turn.steps) {
    for (const block of step.blocks) {
      if (
        block.kind === "tool" &&
        (block.name === "Edit" || block.name === "Write") &&
        (block.status === "completed" || block.status === "error")
      ) {
        completedCalls.push(`${block.toolCallId}:${block.status}`);
      }
    }
  }
  return completedCalls.join("\u0000");
}
