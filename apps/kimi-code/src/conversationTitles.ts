const AUTO_CONVERSATION_TITLES_STORAGE_KEY =
  "kimi-code.auto-conversation-titles";
const CONVERSATION_TITLE_MODEL_STORAGE_KEY =
  "kimi-code.conversation-title-model";

export const DEFAULT_AUTO_CONVERSATION_TITLES = true;

export function loadAutoConversationTitlesEnabled(): boolean {
  if (typeof window === "undefined") return DEFAULT_AUTO_CONVERSATION_TITLES;
  try {
    const stored = window.localStorage.getItem(
      AUTO_CONVERSATION_TITLES_STORAGE_KEY,
    );
    return stored === null
      ? DEFAULT_AUTO_CONVERSATION_TITLES
      : stored === "true";
  } catch {
    return DEFAULT_AUTO_CONVERSATION_TITLES;
  }
}

export function saveAutoConversationTitlesEnabled(enabled: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      AUTO_CONVERSATION_TITLES_STORAGE_KEY,
      String(enabled),
    );
  } catch {
    // Storage failures should not prevent the in-memory setting from changing.
  }
}

export function loadConversationTitleModel(): string | undefined {
  if (typeof window === "undefined") return undefined;
  try {
    return (
      window.localStorage
        .getItem(CONVERSATION_TITLE_MODEL_STORAGE_KEY)
        ?.trim() || undefined
    );
  } catch {
    return undefined;
  }
}

export function saveConversationTitleModel(modelId?: string): void {
  if (typeof window === "undefined") return;
  try {
    if (modelId) {
      window.localStorage.setItem(
        CONVERSATION_TITLE_MODEL_STORAGE_KEY,
        modelId,
      );
    } else {
      window.localStorage.removeItem(CONVERSATION_TITLE_MODEL_STORAGE_KEY);
    }
  } catch {
    // Storage failures should not prevent the in-memory setting from changing.
  }
}
