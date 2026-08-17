export type ChatFollowDecision = {
  currentlyFollowing: boolean;
  distanceFromBottom: number;
  scrollingUp: boolean;
  userScrollingUp: boolean;
};

export function isChatAtBottom(distanceFromBottom: number): boolean {
  return distanceFromBottom <= 48;
}

export function resolveChatFollowState({
  currentlyFollowing,
  distanceFromBottom,
  scrollingUp,
  userScrollingUp,
}: ChatFollowDecision): boolean {
  if (isChatAtBottom(distanceFromBottom)) return true;
  if (scrollingUp && userScrollingUp) return false;
  return currentlyFollowing;
}

export function isUpwardChatScrollKey(
  key: string,
  shiftKey: boolean,
): boolean {
  return (
    key === "ArrowUp" ||
    key === "PageUp" ||
    key === "Home" ||
    (key === " " && shiftKey)
  );
}

export function conversationTurnScrollTarget(turn: HTMLElement): HTMLElement {
  return (
    turn.querySelector<HTMLElement>("[data-conversation-user-message]") ?? turn
  );
}
