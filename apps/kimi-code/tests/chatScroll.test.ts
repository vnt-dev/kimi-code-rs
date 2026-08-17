import assert from "node:assert/strict";
import test from "node:test";

import {
  conversationTurnScrollTarget,
  isChatAtBottom,
  isUpwardChatScrollKey,
  resolveChatFollowState,
} from "../src/chatScroll.ts";

test("explicit upward input leaves follow mode after moving away from the bottom", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 120,
      scrollingUp: true,
      userScrollingUp: true,
    }),
    false,
  );
});

test("automatic upward reflow does not look like user scrolling", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 120,
      scrollingUp: true,
      userScrollingUp: false,
    }),
    true,
  );
});

test("upward input within the bottom threshold keeps follow mode", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 12,
      scrollingUp: true,
      userScrollingUp: true,
    }),
    true,
  );
});

test("scrolling back to the bottom resumes follow mode", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: false,
      distanceFromBottom: 24,
      scrollingUp: false,
      userScrollingUp: false,
    }),
    true,
  );
});

test("remaining away from the bottom keeps follow mode paused", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: false,
      distanceFromBottom: 120,
      scrollingUp: false,
      userScrollingUp: false,
    }),
    false,
  );
});

test("bottom detection uses the shared follow threshold", () => {
  assert.equal(isChatAtBottom(48), true);
  assert.equal(isChatAtBottom(49), false);
});

test("upward keyboard scrolling is recognized", () => {
  assert.equal(isUpwardChatScrollKey("ArrowUp", false), true);
  assert.equal(isUpwardChatScrollKey("PageUp", false), true);
  assert.equal(isUpwardChatScrollKey("Home", false), true);
  assert.equal(isUpwardChatScrollKey(" ", true), true);
  assert.equal(isUpwardChatScrollKey(" ", false), false);
  assert.equal(isUpwardChatScrollKey("PageDown", false), false);
});

test("conversation outline targets the user message inside a turn", () => {
  const userMessage = {} as HTMLElement;
  const turn = {
    querySelector(selector: string) {
      assert.equal(selector, "[data-conversation-user-message]");
      return userMessage;
    },
  } as unknown as HTMLElement;

  assert.equal(conversationTurnScrollTarget(turn), userMessage);
});

test("conversation outline falls back to the turn when no user message is visible", () => {
  const turn = {
    querySelector() {
      return null;
    },
  } as unknown as HTMLElement;

  assert.equal(conversationTurnScrollTarget(turn), turn);
});
