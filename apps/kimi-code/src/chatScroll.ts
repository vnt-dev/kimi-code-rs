export type ChatFollowDecision = {
  currentlyFollowing: boolean;
  distanceFromBottom: number;
  contentHeightChanged: boolean;
  scrollingUp: boolean;
  userScrollingUp: boolean;
  userTogglingDisclosure: boolean;
};

export function resolveChatFollowState({
  currentlyFollowing,
  distanceFromBottom,
  contentHeightChanged,
  scrollingUp,
  userScrollingUp,
  userTogglingDisclosure,
}: ChatFollowDecision): boolean {
  if (userTogglingDisclosure) return false;
  if (scrollingUp && (userScrollingUp || !contentHeightChanged)) return false;
  if (distanceFromBottom <= 48) return true;
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
