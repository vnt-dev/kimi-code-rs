export function conciseError(error: unknown): string {
  const message =
    error instanceof Error
      ? error.message
      : error &&
          typeof error === "object" &&
          "message" in error &&
          typeof error.message === "string"
        ? error.message
        : String(error);
  const summary =
    message
      .split(/\r?\n/)
      .map((line) => line.trim())
      .find(Boolean) ?? "Unknown error";
  const cleaned = summary.replace(/^Error:\s*/i, "");
  return cleaned.length > 300 ? `${cleaned.slice(0, 297)}...` : cleaned;
}
