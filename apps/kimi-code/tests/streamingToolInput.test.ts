import assert from "node:assert/strict";
import test from "node:test";

import { parseStreamingToolInput } from "../src/chat/streamingToolInput.ts";

test("reads Write content before its JSON string is complete", () => {
  assert.deepEqual(
    parseStreamingToolInput('{"path":"src/app.ts","content":"first\\nsecond'),
    { path: "src/app.ts", content: "first\nsecond" },
  );
});

test("reads each available Edit side independently", () => {
  assert.deepEqual(
    parseStreamingToolInput('{"old_string":"before","new_string":"after'),
    { old_string: "before", new_string: "after" },
  );
  assert.deepEqual(parseStreamingToolInput('{"old_string":"before"'), {
    old_string: "before",
  });
});

test("preserves escaped quotes and waits for incomplete escape sequences", () => {
  assert.deepEqual(parseStreamingToolInput('{"content":"say \\"hello\\""}'), {
    content: 'say "hello"',
  });
  assert.deepEqual(parseStreamingToolInput('{"content":"line\\'), {
    content: "line",
  });
});
