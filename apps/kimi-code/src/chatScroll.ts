export type ChatFollowDecision = {
  currentlyFollowing: boolean;
  distanceFromBottom: number;
  contentHeightChanged: boolean;
  scrollingUp: boolean;
  userScrollingUp: boolean;
};

export function resolveChatFollowState({
  currentlyFollowing,
  distanceFromBottom,
  contentHeightChanged,
  scrollingUp,
  userScrollingUp,
}: ChatFollowDecision): boolean {
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
