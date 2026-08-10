export interface LiveUserMessageVisibilityInput {
  prompt: string;
  attachments: readonly unknown[];
  pluginCommand?: unknown;
}

export function hasVisibleLiveUserMessage(
  turn: LiveUserMessageVisibilityInput,
): boolean {
  return (
    turn.prompt.trim().length > 0 ||
    turn.attachments.length > 0 ||
    turn.pluginCommand !== undefined
  );
}
