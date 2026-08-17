import assert from "node:assert/strict";
import test from "node:test";

import {
  conversationTurnScrollTarget,
  isUpwardChatScrollKey,
  resolveChatFollowState,
} from "../src/chatScroll.ts";

test("explicit upward input leaves follow mode while content is changing", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 12,
      contentHeightChanged: true,
      scrollingUp: true,
      userScrollingUp: true,
      userTogglingDisclosure: false,
    }),
    false,
  );
});

test("content reflow does not leave follow mode without upward input", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 120,
      contentHeightChanged: true,
      scrollingUp: true,
      userScrollingUp: false,
      userTogglingDisclosure: false,
    }),
    true,
  );
});

test("scrolling back to the bottom resumes follow mode", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: false,
      distanceFromBottom: 24,
      contentHeightChanged: false,
      scrollingUp: false,
      userScrollingUp: false,
      userTogglingDisclosure: false,
    }),
    true,
  );
});

test("toggling conversation details pauses follow mode during reflow", () => {
  assert.equal(
    resolveChatFollowState({
      currentlyFollowing: true,
      distanceFromBottom: 0,
      contentHeightChanged: true,
      scrollingUp: false,
      userScrollingUp: false,
      userTogglingDisclosure: true,
    }),
    false,
  );
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
