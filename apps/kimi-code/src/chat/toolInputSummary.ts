const TOOL_INPUT_SUMMARY_KEYS = [
  "description",
  "command",
  "pattern",
  "path",
  "query",
  "url",
] as const;

/** Returns the first non-empty, supported input value for a tool card summary. */
export function toolInputSummary(input: unknown): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;

  const values = input as Record<string, unknown>;
  for (const key of TOOL_INPUT_SUMMARY_KEYS) {
    const value = values[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}
