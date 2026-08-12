import assert from "node:assert/strict";
import test from "node:test";

import {
  collectHistoricalSubagentRuns,
  isSubagentEvent,
  mergeSubagentEvent,
  mergeSubagentRuns,
  parseAgentResult,
  parseAgentSwarmResult,
  subagentInvocationMessages,
  subagentRunsWithSwarmItems,
  type SubagentRunsByTool,
} from "../src/subagentEvents.ts";
import type { MessageContent, ProtocolMessage, Role } from "../src/types.ts";

function message(
  id: string,
  role: Role,
  content: MessageContent[],
): ProtocolMessage {
  return {
    id,
    role,
    session_id: "session-1",
    content,
    created_at: `2026-01-01T00:00:${id.padStart(2, "0")}Z`,
  };
}

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
        historyAvailable: true,
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
        historyAvailable: false,
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
      historyAvailable: true,
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
    historyAvailable: true,
  });
  assert.equal(merged[1].subagentId, "agent-2");
});

test("historical Agent results expose the persisted agent and invocation prompt", () => {
  assert.deepEqual(
    parseAgentResult(
      [
        "agent_id: agent-7",
        "actual_subagent_type: explore",
        "status: completed",
        "",
        "[summary]",
        "finished",
      ].join("\n"),
      {
        prompt: "inspect the repository",
        description: "inspect repository",
      },
    ),
    [{
      subagentId: "agent-7",
      subagentName: "explore",
      description: "inspect repository",
      runInBackground: false,
      status: "completed",
      resultSummary: "finished",
      error: undefined,
      historyPrompt: "inspect the repository",
      historyAvailable: true,
    }],
  );
});

test("historical invocations number repeated resume prompts from the end", () => {
  const input = {
    prompt: "continue",
    description: "resume work",
    resume: "agent-7",
  };
  const output = (summary: string) => [
    "agent_id: agent-7",
    "actual_subagent_type: explore",
    "status: completed",
    "",
    "[summary]",
    summary,
  ].join("\n");
  const runs = collectHistoricalSubagentRuns([
    message("1", "assistant", [{
      type: "tool_use",
      tool_call_id: "call-1",
      tool_name: "Agent",
      input,
    }]),
    message("2", "tool", [{
      type: "tool_result",
      tool_call_id: "call-1",
      output: output("first"),
    }]),
    message("3", "assistant", [{
      type: "tool_use",
      tool_call_id: "call-2",
      tool_name: "Agent",
      input,
    }]),
    message("4", "tool", [{
      type: "tool_result",
      tool_call_id: "call-2",
      output: output("second"),
    }]),
  ]);

  assert.equal(runs["call-1"][0].historyOccurrenceFromEnd, 1);
  assert.equal(runs["call-1"][0].historyNextPrompt, "continue");
  assert.equal(runs["call-2"][0].historyOccurrenceFromEnd, 0);
});

test("subagent invocation slicing keeps continuation turns and stops at its summary", () => {
  const transcript = [
    message("1", "user", [{ type: "text", text: "continue" }]),
    message("2", "assistant", [{ type: "text", text: "working" }]),
    message("3", "user", [{ type: "text", text: "write a final summary" }]),
    message("4", "assistant", [{ type: "text", text: "first result" }]),
    message("5", "user", [{ type: "text", text: "continue" }]),
    message("6", "assistant", [{ type: "text", text: "second result" }]),
  ];
  const selected = subagentInvocationMessages(transcript, {
    subagentId: "agent-7",
    subagentName: "explore",
    runInBackground: false,
    status: "completed",
    resultSummary: "first result",
    historyPrompt: "continue",
    historyOccurrenceFromEnd: 1,
    historyAvailable: true,
  });

  assert.deepEqual(selected.map((item) => item.id), ["1", "2", "3", "4"]);
});

test("subagent invocation slicing matches prompts after persisted context prefixes", () => {
  const prompt = "Review packages and report the package count.";
  const transcript = [
    message("1", "user", [{
      type: "text",
      text: [
        "<git-context>",
        "Working directory: D:/kimi/kimi-code",
        "</git-context>",
        "",
        prompt,
      ].join("\n"),
    }]),
    message("2", "assistant", [{ type: "text", text: "There are 17 packages." }]),
  ];
  const selected = subagentInvocationMessages(transcript, {
    subagentId: "agent-7",
    subagentName: "explore",
    runInBackground: false,
    status: "completed",
    resultSummary: "There are 17 packages.",
    historyPrompt: prompt,
    historyAvailable: true,
  });

  assert.deepEqual(selected.map((item) => item.id), ["1", "2"]);
});

test("AgentSwarm history prompts keep resume entries before item expansions", () => {
  const runs = parseAgentSwarmResult(
    [
      "<agent_swarm_result>",
      "<summary>completed: 2</summary>",
      '<subagent mode="resume" agent_id="agent-old" outcome="completed">resumed</subagent>',
      '<subagent agent_id="agent-new" item="files" outcome="completed">spawned</subagent>',
      "</agent_swarm_result>",
    ].join("\n"),
    "call-swarm",
    {
      resume_agent_ids: { "agent-old": "continue review" },
      prompt_template: "Review {{item}}",
      items: ["files"],
    },
  );

  assert.equal(runs[0].historyPrompt, "continue review");
  assert.equal(runs[1].historyPrompt, "Review files");
});

test("failed invocation slicing stops at the next known parent prompt", () => {
  const transcript = [
    message("1", "user", [{ type: "text", text: "first task" }]),
    message("2", "assistant", [{ type: "text", text: "partial" }]),
    message("3", "user", [{ type: "text", text: "second task" }]),
    message("4", "assistant", [{ type: "text", text: "done" }]),
  ];
  const selected = subagentInvocationMessages(transcript, {
    subagentId: "agent-7",
    subagentName: "explore",
    runInBackground: false,
    status: "failed",
    error: "failed",
    historyPrompt: "first task",
    historyNextPrompt: "second task",
    historyAvailable: true,
  });

  assert.deepEqual(selected.map((item) => item.id), ["1", "2"]);
});
