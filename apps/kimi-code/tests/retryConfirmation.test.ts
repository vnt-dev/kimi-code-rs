import assert from "node:assert/strict";
import test from "node:test";

import {
  isRetryConfirmationPayload,
  retryConfirmationResponse,
} from "../src/retryConfirmation.ts";

test("recognizes only the dedicated retry confirmation presentation", () => {
  assert.equal(
    isRetryConfirmationPayload({ presentation: "retry_confirmation" }),
    true,
  );
  assert.equal(isRetryConfirmationPayload({ presentation: "plan_review" }), false);
  assert.equal(isRetryConfirmationPayload(null), false);
});

test("continue returns the backend retry answer", () => {
  assert.deepEqual(retryConfirmationResponse(), {
    answers: { retry_confirmation: "Retry" },
    method: "enter",
  });
});
