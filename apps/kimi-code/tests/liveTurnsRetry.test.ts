import assert from "node:assert/strict";
import test from "node:test";

import {
  clearRetryStatus,
  isVisibleRetryStep,
  normalizeRetryAttempt,
} from "../src/chat/retryStatus.ts";

test("normalizes the failed attempt into a user-facing retry number", () => {
  assert.equal(normalizeRetryAttempt(2), 2);
  assert.equal(normalizeRetryAttempt(2.9), 2);
  assert.equal(normalizeRetryAttempt(0), 1);
  assert.equal(normalizeRetryAttempt(Number.NaN), 1);
});

test("empty retry steps do not take up space in the conversation", () => {
  const steps = Array.from({ length: 7 }, (_, index) => ({
    step: index + 1,
    stepId: `attempt-${index + 1}`,
    blocks: [],
  }));

  assert.equal(steps.length, 7);
  assert.equal(
    steps.filter((step) => isVisibleRetryStep(step, [])).length,
    0,
  );
});

test("the retry notice clears as soon as the model responds again", () => {
  const recovered = clearRetryStatus({
    status: "running",
    retry: { attempt: 3 },
  });

  assert.equal(recovered.retry, undefined);
});
