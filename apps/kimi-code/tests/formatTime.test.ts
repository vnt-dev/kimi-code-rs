import assert from "node:assert/strict";
import test from "node:test";

import { formatTime } from "../src/utils/format.ts";

const now = new Date(2026, 7, 8, 14, 30);

test("formats same-day user message timestamps as time only", () => {
  assert.equal(formatTime(new Date(2026, 7, 8, 9, 5).getTime(), now), "09:05");
});

test("adds month and day for earlier messages in the current year", () => {
  assert.equal(
    formatTime(new Date(2026, 6, 31, 9, 5).getTime(), now),
    "07-31 09:05",
  );
});

test("adds the year for messages from previous years", () => {
  assert.equal(
    formatTime(new Date(2025, 11, 31, 9, 5).getTime(), now),
    "2025-12-31 09:05",
  );
});
