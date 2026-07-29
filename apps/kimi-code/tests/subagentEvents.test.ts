import assert from "node:assert/strict";
import test from "node:test";

import {
  isSubagentEvent,
  mergeSubagentEvent,
  subagentRunsWithSwarmItems,
  type SubagentRunsByTool,
} from "../src/subagentEvents.ts";

test("subagent lifecycle is grouped by parent tool call and keeps result data", () => {
  let runs: SubagentRunsByTool = {};
  runs = mergeSubagentEvent(runs, {
    type: "subagent.spawned",
    subagentId: "agent-1",
    subagentName: "researcher",
    parentToolCallId: "call-1",
    description: "查询上海天气",
    swarmIndex: 2,
    runInBackground: false,
  });
  runs = mergeSubagentEvent(runs, {
    type: "subagent.started",
    subagentId: "agent-1",
  });
  runs = mergeSubagentEvent(runs, {
    type: "subagent.completed",
    subagentId: "agent-1",
    resultSummary: "上海晴，32℃",
    usage: {
      inputOther: 100,
      output: 20,
      inputCacheRead: 5,
      inputCacheCreation: 0,
    },
    contextTokens: 4096,
  });

  assert.deepEqual(runs["call-1"], [
    {
      subagentId: "agent-1",
      subagentName: "researcher",
      description: "查询上海天气",
      swarmIndex: 2,
      runInBackground: false,
      status: "completed",
      resultSummary: "上海晴，32℃",
      usage: {
        inputOther: 100,
        output: 20,
        inputCacheRead: 5,
        inputCacheCreation: 0,
      },
      contextTokens: 4096,
      error: undefined,
    },
  ]);
});

test("concurrent subagents are displayed in swarm item order", () => {
  let runs: SubagentRunsByTool = {};
  for (const [subagentId, swarmIndex] of [
    ["agent-2", 2],
    ["agent-1", 1],
  ] as const) {
    runs = mergeSubagentEvent(runs, {
      type: "subagent.spawned",
      subagentId,
      subagentName: "agent",
      parentToolCallId: "call-1",
      description: subagentId,
      swarmIndex,
      runInBackground: false,
    });
  }

  assert.deepEqual(
    runs["call-1"].map((run) => run.subagentId),
    ["agent-1", "agent-2"],
  );
});

test("suspended runs return to running and failures preserve their reason", () => {
  let runs = mergeSubagentEvent(
    {},
    {
      type: "subagent.spawned",
      subagentId: "agent-1",
      subagentName: "agent",
      parentToolCallId: "call-1",
      runInBackground: false,
    },
  );
  runs = mergeSubagentEvent(runs, {
    type: "subagent.suspended",
    subagentId: "agent-1",
    reason: "rate limited",
  });
  assert.equal(runs["call-1"][0].status, "suspended");
  assert.equal(runs["call-1"][0].error, "rate limited");

  runs = mergeSubagentEvent(runs, {
    type: "subagent.started",
    subagentId: "agent-1",
  });
  assert.equal(runs["call-1"][0].status, "running");
  assert.equal(runs["call-1"][0].error, undefined);

  runs = mergeSubagentEvent(runs, {
    type: "subagent.failed",
    subagentId: "agent-1",
    error: "provider failed",
  });
  assert.equal(runs["call-1"][0].status, "failed");
  assert.equal(runs["call-1"][0].error, "provider failed");
});

test("unmatched lifecycle events do not allocate orphan state", () => {
  const runs = {};
  assert.equal(
    mergeSubagentEvent(runs, {
      type: "subagent.started",
      subagentId: "missing",
    }),
    runs,
  );
});

test("wire event validation rejects incomplete lifecycle records", () => {
  assert.equal(
    isSubagentEvent({
      type: "subagent.spawned",
      subagentId: "agent-1",
      subagentName: "agent",
      parentToolCallId: "call-1",
      runInBackground: false,
    }),
    true,
  );
  assert.equal(
    isSubagentEvent({
      type: "subagent.completed",
      subagentId: "agent-1",
    }),
    false,
  );
});

test("swarm item text is used as the user-facing task title", () => {
  const runs = mergeSubagentEvent(
    {},
    {
      type: "subagent.spawned",
      subagentId: "agent-1",
      subagentName: "agent",
      parentToolCallId: "call-1",
      description: "并行查询天气 #1 (agent)",
      swarmIndex: 1,
      runInBackground: false,
    },
  )["call-1"];

  const displayed = subagentRunsWithSwarmItems(runs, {
    items: ["查询上海天气", "查询北京天气"],
  });
  assert.equal(displayed[0].description, "查询上海天气");
  assert.equal(runs[0].description, "并行查询天气 #1 (agent)");
});
