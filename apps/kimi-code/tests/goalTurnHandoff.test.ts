import assert from "node:assert/strict";
import test from "node:test";

import {
  reduceQueuedAgentChatEvents,
  type InFlightTurn,
  type QueuedAgentChatEvent,
} from "../src/chat/liveTurns.ts";

const sessionId = "session-1";

function firstTurn(): InFlightTurn {
  return {
    promptId: "prompt-1",
    userMessageId: "user-1",
    prompt: "test goal",
    attachments: [],
    skills: [],
    steeredPrompts: [],
    createdAt: "2026-08-18T00:00:00Z",
    turnId: 1,
    status: "running",
    steps: [
      {
        step: 1,
        status: "completed",
        blocks: [{ kind: "text", content: "first response" }],
      },
    ],
  };
}

test("keeps the completed goal turn visible while the next turn starts", () => {
  const events: QueuedAgentChatEvent[] = [
    {
      sessionId,
      agentId: "main",
      event: { type: "turn.ended", turnId: 1, reason: "completed" },
    },
    {
      sessionId,
      agentId: "main",
      event: {
        type: "turn.started",
        turnId: 2,
        origin: { kind: "system_trigger", name: "goal_continuation" },
        prompt: "Continue working toward the active goal.",
      },
    },
    {
      sessionId,
      agentId: "main",
      event: { type: "assistant.delta", turnId: 2, delta: "second response" },
    },
  ];

  const reduced = reduceQueuedAgentChatEvents(
    { [sessionId]: firstTurn() },
    events,
  )[sessionId];

  assert.equal(reduced.turnId, 2);
  assert.equal(reduced.status, "running");
  assert.equal(reduced.steps[0].blocks[0].kind, "text");
  assert.equal(reduced.handoffTurns?.length, 1);
  assert.equal(reduced.handoffTurns?.[0].turnId, 1);
  assert.equal(reduced.handoffTurns?.[0].status, "completed");
  assert.deepEqual(reduced.handoffTurns?.[0].steps[0].blocks, [
    { kind: "text", content: "first response" },
  ]);
});
