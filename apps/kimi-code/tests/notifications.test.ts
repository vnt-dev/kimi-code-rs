import assert from "node:assert/strict";
import test from "node:test";

import {
  compactNotificationText,
  conversationNotificationText,
  finalLiveResponseText,
  shouldNotifyConversation,
} from "../src/notificationUtils.ts";
import {
  conversationStatus,
  userAttentionForInteraction,
} from "../src/conversationStatus.ts";
import type { AgentInteraction } from "../src/types.ts";
import type { InFlightTurn } from "../src/chat/liveTurns.ts";

test("notifies when the window is unfocused or another conversation is active", () => {
  assert.equal(shouldNotifyConversation("session-a", "session-a", true), false);
  assert.equal(shouldNotifyConversation("session-a", "session-a", false), true);
  assert.equal(shouldNotifyConversation("session-a", "session-b", true), true);
  assert.equal(shouldNotifyConversation("session-a", undefined, true), true);
});

test("compacts notification text and keeps it within the requested limit", () => {
  assert.equal(compactNotificationText("  hello\n   world  "), "hello world");
  assert.equal(compactNotificationText("abcdefgh", 6), "abcde…");
  assert.equal(compactNotificationText("   "), undefined);
});

test("uses the conversation title and event content as notification text", () => {
  assert.deepEqual(
    conversationNotificationText("Build notifications", "The task is done."),
    {
      title: "Build notifications",
      body: "The task is done.",
    },
  );
});

test("conversation status prioritizes user attention over running and unread", () => {
  const approval: AgentInteraction = {
    id: "approval-1",
    kind: "approval",
    createdAt: 1,
    payload: {
      toolName: "Bash",
      action: "Run tests",
      display: { kind: "command", command: "pnpm test" },
    },
  };
  assert.equal(
    conversationStatus({
      interactions: [approval],
      running: true,
      completedUnread: true,
    }),
    "attention",
  );
  assert.equal(
    conversationStatus({ interactions: [], running: true, completedUnread: true }),
    "running",
  );
  assert.equal(
    conversationStatus({ interactions: [], running: false, completedUnread: true }),
    "completed",
  );
});

test("classifies questions, tool approvals, and plan reviews as user attention", () => {
  const question: AgentInteraction = {
    id: "question-1",
    kind: "question",
    createdAt: 1,
    payload: {
      questions: [{ question: "Choose a database", options: [] }],
    },
  };
  const planReview: AgentInteraction = {
    id: "plan-1",
    kind: "approval",
    createdAt: 1,
    payload: {
      toolName: "ExitPlanMode",
      action: "Review plan",
      display: { kind: "plan_review", plan: "The plan" },
    },
  };
  const userTool: AgentInteraction = {
    id: "host-tool-1",
    kind: "user_tool",
    createdAt: 1,
    payload: {},
  };

  assert.deepEqual(userAttentionForInteraction(question), {
    kind: "question",
    content: "Choose a database",
  });
  assert.deepEqual(userAttentionForInteraction(planReview), {
    kind: "planReview",
    content: "The plan",
  });
  assert.equal(userAttentionForInteraction(userTool), undefined);
});

test("uses the final model response from the last step with assistant text", () => {
  const turn: InFlightTurn = {
    prompt: "Implement it",
    attachments: [],
    skills: [],
    steeredPrompts: [],
    createdAt: "2026-08-06T00:00:00.000Z",
    status: "completed",
    steps: [
      {
        step: 1,
        status: "completed",
        blocks: [{ kind: "text", content: "Earlier tool preamble" }],
      },
      {
        step: 2,
        status: "completed",
        blocks: [
          { kind: "thinking", content: "internal" },
          { kind: "text", content: "Final " },
          { kind: "content", content: { type: "text", text: "answer" } },
        ],
      },
    ],
  };

  assert.equal(finalLiveResponseText(turn), "Final answer");
});
