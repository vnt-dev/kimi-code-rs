import assert from "node:assert/strict";
import test from "node:test";

import {
  buildToolLineDiff,
  historyTurnFileChanges,
  liveTurnFileChanges,
  normalizeToolFilePath,
  toolFileChangesFromSuccessfulCalls,
} from "../src/chat/fileChanges.ts";
import type { InFlightTurn } from "../src/chat/liveTurns.ts";
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
    created_at: "2026-01-01T00:00:00Z",
  };
}

test("builds an aligned line diff with old and new line numbers", () => {
  assert.deepEqual(buildToolLineDiff("same\nold\ntail\n", "same\nnew\ntail\n"), [
    { kind: "context", text: "same", oldLine: 1, newLine: 1 },
    { kind: "removed", text: "old", oldLine: 2 },
    { kind: "added", text: "new", newLine: 2 },
    { kind: "context", text: "tail", oldLine: 3, newLine: 3 },
  ]);
});

test("groups successful Edit and Write calls by normalized path in call order", () => {
  const changes = toolFileChangesFromSuccessfulCalls([
    {
      name: "Edit",
      input: {
        path: ".\\src\\app.ts",
        old_string: "const oldName = 1;\n",
        new_string: "const newName = 1;\n",
      },
    },
    {
      name: "Write",
      input: {
        path: "src/app.ts",
        content: "extra\nline\n",
        mode: "append",
      },
    },
  ]);

  assert.equal(changes.length, 1);
  assert.equal(changes[0].path, "src/app.ts");
  assert.equal(changes[0].additions, 3);
  assert.equal(changes[0].deletions, 1);
  assert.deepEqual(
    changes[0].operations.map((operation) => operation.kind),
    ["edit", "write"],
  );
  assert.equal(changes[0].operations[1].kind, "write");
  if (changes[0].operations[1].kind === "write") {
    assert.equal(changes[0].operations[1].mode, "append");
  }
});

test("preserves replace-all and defaults Write to overwrite", () => {
  const changes = toolFileChangesFromSuccessfulCalls([
    {
      name: "Edit",
      input: {
        path: "src/app.ts",
        old_string: "old",
        new_string: "new",
        replace_all: true,
      },
    },
    {
      name: "Write",
      input: { path: "generated.txt", content: "one\ntwo\n" },
    },
  ]);

  assert.equal(changes[0].operations[0].kind, "edit");
  if (changes[0].operations[0].kind === "edit") {
    assert.equal(changes[0].operations[0].replaceAll, true);
  }
  assert.equal(changes[1].operations[0].kind, "write");
  if (changes[1].operations[0].kind === "write") {
    assert.equal(changes[1].operations[0].mode, "overwrite");
    assert.equal(changes[1].operations[0].additions, 2);
  }
});

test("excludes failed, incomplete, invalid, no-op, and unrelated tool calls", () => {
  const calls = [
    message("assistant", "assistant", [
      {
        type: "tool_use",
        tool_call_id: "ok",
        tool_name: "Edit",
        input: { path: "ok.ts", old_string: "a", new_string: "b" },
      },
      {
        type: "tool_use",
        tool_call_id: "failed",
        tool_name: "Write",
        input: { path: "failed.ts", content: "x" },
      },
      {
        type: "tool_use",
        tool_call_id: "missing-result",
        tool_name: "Write",
        input: { path: "pending.ts", content: "x" },
      },
      {
        type: "tool_use",
        tool_call_id: "bash",
        tool_name: "Bash",
        input: { command: "echo x > ignored.ts" },
      },
      {
        type: "tool_use",
        tool_call_id: "noop",
        tool_name: "Edit",
        input: { path: "noop.ts", old_string: "same", new_string: "same" },
      },
    ]),
  ];
  const results = new Map([
    ["ok", { is_error: false }],
    ["failed", { is_error: true }],
    ["bash", { is_error: false }],
    ["noop", { is_error: false }],
  ]);

  assert.deepEqual(
    historyTurnFileChanges(calls, results).map((change) => change.path),
    ["ok.ts"],
  );
});

test("keeps parent and subagent transcript changes independent", () => {
  const successfulResult = new Map([["call", { is_error: false }]]);
  const transcript = (path: string) => [
    message("assistant", "assistant", [
      {
        type: "tool_use" as const,
        tool_call_id: "call",
        tool_name: "Write",
        input: { path, content: "content" },
      },
    ]),
  ];

  assert.equal(
    historyTurnFileChanges(transcript("parent.ts"), successfulResult)[0].path,
    "parent.ts",
  );
  assert.equal(
    historyTurnFileChanges(transcript("child.ts"), successfulResult)[0].path,
    "child.ts",
  );
});

test("live cancelled turns retain completed writes and ignore failed writes", () => {
  const turn: InFlightTurn = {
    prompt: "",
    attachments: [],
    skills: [],
    steeredPrompts: [],
    createdAt: "2026-01-01T00:00:00Z",
    status: "cancelled",
    steps: [
      {
        step: 1,
        status: "interrupted",
        blocks: [
          {
            kind: "tool",
            toolCallId: "completed",
            name: "Write",
            argumentsText: "",
            input: { path: "saved.txt", content: "saved" },
            status: "completed",
            updates: [],
            isError: false,
          },
          {
            kind: "tool",
            toolCallId: "failed",
            name: "Write",
            argumentsText: "",
            input: { path: "failed.txt", content: "failed" },
            status: "error",
            updates: [],
            isError: true,
          },
        ],
      },
    ],
  };

  assert.deepEqual(
    liveTurnFileChanges(turn).map((change) => change.path),
    ["saved.txt"],
  );
});

test("large replacements use the bounded fallback and preserve shared edges", () => {
  const before = [
    "shared-start",
    ...Array.from({ length: 500 }, (_, index) => `old-${index}`),
    "shared-end",
  ].join("\n");
  const after = [
    "shared-start",
    ...Array.from({ length: 500 }, (_, index) => `new-${index}`),
    "shared-end",
  ].join("\n");
  const lines = buildToolLineDiff(before, after);

  assert.deepEqual(lines[0], {
    kind: "context",
    text: "shared-start",
    oldLine: 1,
    newLine: 1,
  });
  assert.deepEqual(lines.at(-1), {
    kind: "context",
    text: "shared-end",
    oldLine: 502,
    newLine: 502,
  });
  assert.equal(lines.filter((line) => line.kind === "removed").length, 500);
  assert.equal(lines.filter((line) => line.kind === "added").length, 500);
});

test("normalizes separators without changing absolute or parent paths", () => {
  assert.equal(normalizeToolFilePath("./src\\main.ts"), "src/main.ts");
  assert.equal(normalizeToolFilePath("C:\\repo\\main.ts"), "C:/repo/main.ts");
  assert.equal(normalizeToolFilePath("../shared/main.ts"), "../shared/main.ts");
});
