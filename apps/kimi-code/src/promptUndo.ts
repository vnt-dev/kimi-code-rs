const MAX_PROMPT_UNDO_ENTRIES = 50;
const PROMPT_EDIT_MERGE_WINDOW_MS = 1_000;

type PromptEditKind = "insert" | "delete" | "replace";

interface PromptEdit {
  kind: PromptEditKind;
  at: number;
}

export interface PromptUndoHistory {
  current: string;
  past: string[];
  lastEdit?: PromptEdit;
}

export function createPromptUndoHistory(
  current = "",
): PromptUndoHistory {
  return { current, past: [] };
}

function promptEditKind(current: string, next: string): PromptEditKind {
  if (next.length > current.length) return "insert";
  if (next.length < current.length) return "delete";
  return "replace";
}

function rememberCurrent(history: PromptUndoHistory): string[] {
  const { current, past } = history;
  if (past.at(-1) === current) return past;
  return [...past, current].slice(-MAX_PROMPT_UNDO_ENTRIES);
}

export function recordPromptEdit(
  history: PromptUndoHistory,
  next: string,
  at = Date.now(),
): PromptUndoHistory {
  if (next === history.current) return history;

  const kind = promptEditKind(history.current, next);
  const mergesWithLastEdit =
    history.lastEdit?.kind === kind &&
    at - history.lastEdit.at <= PROMPT_EDIT_MERGE_WINDOW_MS;

  return {
    current: next,
    past: mergesWithLastEdit ? history.past : rememberCurrent(history),
    lastEdit: { kind, at },
  };
}

export function recordPromptInput(
  history: PromptUndoHistory,
  next: string,
  options: { isComposing?: boolean; at?: number } = {},
): PromptUndoHistory {
  if (options.isComposing) return history;
  return recordPromptEdit(history, next, options.at);
}

export function undoPromptEdit(
  history: PromptUndoHistory,
): PromptUndoHistory {
  const previous = history.past.at(-1);
  if (previous === undefined) return history;
  return {
    current: previous,
    past: history.past.slice(0, -1),
  };
}

export function canUndoPromptEdit(history: PromptUndoHistory): boolean {
  return history.past.length > 0;
}
