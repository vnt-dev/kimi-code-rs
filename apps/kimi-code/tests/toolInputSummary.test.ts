import assert from "node:assert/strict";
import test from "node:test";

import { toolInputSummary } from "../src/chat/toolInputSummary.ts";

test("tool input summary follows the configured parameter priority", () => {
  assert.equal(
    toolInputSummary({ url: "https://example.com", path: "src/app.ts", command: "pnpm test", description: "Run tests" }),
    "Run tests",
  );
  assert.equal(
    toolInputSummary({ url: "https://example.com", path: "src/app.ts", command: "pnpm test" }),
    "pnpm test",
  );
  assert.equal(toolInputSummary({ path: "src/app.ts", pattern: "TODO" }), "TODO");
  assert.equal(toolInputSummary({ url: "https://example.com", query: "tool cards" }), "tool cards");
  assert.equal(toolInputSummary({ url: "https://example.com", pattern: "TODO" }), "TODO");
});

test("tool input summary skips blank and unsupported values", () => {
  assert.equal(toolInputSummary({ description: "   ", path: "src/app.ts" }), "src/app.ts");
  assert.equal(toolInputSummary({ description: "  Run tests  " }), "Run tests");
  assert.equal(toolInputSummary({ command: 42, url: "https://example.com" }), "https://example.com");
  assert.equal(toolInputSummary(["command"]), undefined);
  assert.equal(toolInputSummary(undefined), undefined);
});
