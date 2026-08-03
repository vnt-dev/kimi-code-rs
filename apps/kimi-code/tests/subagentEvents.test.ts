import assert from "node:assert/strict";
import test from "node:test";

import {
  isSubagentEvent,
  mergeSubagentEvent,
  mergeSubagentRuns,
  parseAgentSwarmResult,
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

test("historical AgentSwarm results restore every subagent and preserve markdown", () => {
  const completedBody = [
    "## Result",
    "",
    "literal </subagent> marker",
    "",
    "```ts",
    "const answer = 42;",
    "```",
  ].join("\n");
  const output = [
    "<agent_swarm_result>",
    "<summary>completed: 1, failed: 1</summary>",
    `<subagent agent_id="agent-1" item="A &amp; &lt;B&gt;&quot;" state="started" outcome="completed">${completedBody}</subagent>`,
    '<subagent item="second" state="not_started" outcome="failed">provider failed</subagent>',
    "</agent_swarm_result>",
  ].join("\n");

  assert.deepEqual(
    parseAgentSwarmResult(output, "call-1", {
      subagent_type: "explore",
    }),
    [
      {
        subagentId: "agent-1",
        subagentName: "explore",
        description: 'A & <B>"',
        swarmIndex: 1,
        runInBackground: false,
        status: "completed",
        resultSummary: completedBody,
        error: undefined,
      },
      {
        subagentId: "call-1:history:2",
        subagentName: "explore",
        description: "second",
        swarmIndex: 2,
        runInBackground: false,
        status: "failed",
        resultSummary: undefined,
        error: "provider failed",
      },
    ],
  );
});

test("historical resumed and aborted subagents use stable fallback data", () => {
  const output = [
    "<agent_swarm_result>",
    "<summary>aborted: 1</summary>",
    '<subagent mode="resume" agent_id="agent-old" state="started" outcome="aborted">cancelled</subagent>',
    "</agent_swarm_result>",
  ].join("\n");

  assert.deepEqual(parseAgentSwarmResult(output, "call-2", {}), [
    {
      subagentId: "agent-old",
      subagentName: "agent",
      description: undefined,
      swarmIndex: 1,
      runInBackground: false,
      status: "failed",
      resultSummary: undefined,
      error: "cancelled",
    },
  ]);
  assert.deepEqual(parseAgentSwarmResult("not a swarm result", "call-2", {}), []);
  assert.deepEqual(parseAgentSwarmResult({ output }, "call-2", {}), []);
});

test("live subagent state overrides matching historical state without hiding siblings", () => {
  const historical = parseAgentSwarmResult(
    [
      "<agent_swarm_result>",
      "<summary>completed: 2</summary>",
      '<subagent agent_id="agent-1" item="first" outcome="completed">old first</subagent>',
      '<subagent agent_id="agent-2" item="second" outcome="completed">old second</subagent>',
      "</agent_swarm_result>",
    ].join("\n"),
    "call-3",
    {},
  );
  const merged = mergeSubagentRuns(historical, [
    {
      subagentId: "agent-1",
      subagentName: "researcher",
      description: "generic live description",
      swarmIndex: 1,
      runInBackground: false,
      status: "running",
    },
  ]);

  assert.equal(merged.length, 2);
  assert.deepEqual(merged[0], {
    subagentId: "agent-1",
    subagentName: "researcher",
    description: "first",
    swarmIndex: 1,
    runInBackground: false,
    status: "running",
    resultSummary: "old first",
    error: undefined,
  });
  assert.equal(merged[1].subagentId, "agent-2");
});
