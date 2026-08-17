import assert from "node:assert/strict";
import test from "node:test";

import {
  compactionSummaryForLiveTurn,
  currentLiveCompactionAnchor,
  groupHistoryMessages,
  updateLiveCompaction,
} from "../src/chat/conversationTimeline.ts";
import type { InFlightTurn } from "../src/chat/liveTurns.ts";
import type {
  MessageContent,
  ProtocolMessage,
  Role,
} from "../src/types.ts";

function liveTurn(): InFlightTurn {
  return {
    prompt: "question",
    attachments: [],
    skills: [],
    steeredPrompts: [],
    createdAt: "2026-01-01T00:00:00Z",
    turnId: 1,
    status: "running",
    steps: [
      {
        step: 98,
        status: "running",
        blocks: [{ kind: "text", content: "before" }],
      },
    ],
  };
}

function message(
  id: string,
  role: Role,
  text: string,
  origin?: string,
): ProtocolMessage {
  const content: MessageContent[] = [{ type: "text", text }];
  return {
    id,
    role,
    session_id: "session-1",
    content,
    created_at: "2026-01-01T00:00:00Z",
    metadata: origin ? { origin: { kind: origin } } : undefined,
  };
}

test("live compaction stays between output emitted before and after it", () => {
  const initial = liveTurn();
  assert.deepEqual(currentLiveCompactionAnchor(initial), {
    turnKey: initial.createdAt,
    stepKey: "step-98",
    afterBlockIndex: 0,
    liveBlockId: "compaction-0",
  });

  let turn = updateLiveCompaction(initial, { phase: "started" });
  turn = updateLiveCompaction(turn, {
    phase: "completed",
    tokensBefore: 18_300,
    tokensAfter: 2_600,
  });
  turn.steps[0].blocks.push({ kind: "text", content: "after" });

  assert.deepEqual(turn.steps[0].blocks, [
    { kind: "text", content: "before" },
    {
      kind: "compaction",
      id: "compaction-0",
      event: {
        phase: "completed",
        tokensBefore: 18_300,
        tokensAfter: 2_600,
      },
    },
    { kind: "text", content: "after" },
  ]);
});

test("replayed compaction events update one divider instead of duplicating it", () => {
  let turn = updateLiveCompaction(liveTurn(), { phase: "started" });
  turn = updateLiveCompaction(turn, { phase: "started", trigger: "auto" });
  turn = updateLiveCompaction(turn, { phase: "completed" });
  turn = updateLiveCompaction(turn, {
    phase: "completed",
    tokensBefore: 10_000,
    tokensAfter: 2_000,
  });

  const dividers = turn.steps.flatMap((step) =>
    step.blocks.filter((block) => block.kind === "compaction"),
  );
  assert.equal(dividers.length, 1);
  assert.deepEqual(dividers[0].event, {
    phase: "completed",
    tokensBefore: 10_000,
    tokensAfter: 2_000,
  });
});

test("history keeps compaction and continuation in the same user turn", () => {
  const turns = groupHistoryMessages([
    message("user-1", "user", "question"),
    message("assistant-1", "assistant", "before"),
    message("summary-1", "user", "summary", "compaction_summary"),
    message("assistant-2", "assistant", "after"),
  ]);

  assert.equal(turns.length, 1);
  assert.equal(turns[0].user?.id, "user-1");
  assert.deepEqual(
    turns[0].responses.map((item) => item.id),
    ["assistant-1", "summary-1", "assistant-2"],
  );
});

test("live compaction resolves the summary persisted after its user message", () => {
  const turn = {
    ...liveTurn(),
    userMessageId: "user-1",
  };
  const oldSummary = message(
    "summary-old",
    "user",
    "old summary",
    "compaction_summary",
  );
  const currentSummary = message(
    "summary-1",
    "user",
    "current summary",
    "compaction_summary",
  );
  const messages = [
    oldSummary,
    message("user-1", "user", "question"),
    message("assistant-1", "assistant", "before"),
    currentSummary,
  ];

  assert.equal(
    compactionSummaryForLiveTurn(messages, turn)?.id,
    "summary-1",
  );
});
