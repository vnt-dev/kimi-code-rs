import type { InFlightTurn } from "./chat/liveTurns";

const NOTIFICATION_PREVIEW_LIMIT = 160;

export function shouldNotifyConversation(
  sessionId: string,
  activeSessionId: string | undefined,
  windowFocused: boolean,
): boolean {
  return !windowFocused || sessionId !== activeSessionId;
}

export function compactNotificationText(
  value: string | undefined,
  limit = NOTIFICATION_PREVIEW_LIMIT,
): string | undefined {
  const compact = value?.replace(/\s+/g, " ").trim();
  if (!compact) return undefined;
  if (compact.length <= limit) return compact;
  return `${compact.slice(0, Math.max(1, limit - 1)).trimEnd()}…`;
}

export function conversationNotificationText(
  conversationTitle: string | undefined,
  content: string | undefined,
): { title: string; body?: string } {
  return {
    title: compactNotificationText(conversationTitle, 72) ?? "Kimi Code",
    body: compactNotificationText(content),
  };
}

export function finalLiveResponseText(
  turn: InFlightTurn | undefined,
): string | undefined {
  if (!turn) return undefined;
  for (let index = turn.steps.length - 1; index >= 0; index -= 1) {
    const text = turn.steps[index].blocks
      .flatMap((block) => {
        if (block.kind === "text") return [block.content];
        if (block.kind === "content" && block.content.type === "text") {
          return [block.content.text];
        }
        return [];
      })
      .join("")
      .trim();
    if (text) return text;
  }
  return undefined;
}
