import assert from "node:assert/strict";
import test from "node:test";

import { cronTaskBadge, formatCronDate } from "../src/cronTasks.ts";

test("cron task badge caps large counts without hiding zero", () => {
  assert.equal(cronTaskBadge(0), "0");
  assert.equal(cronTaskBadge(12), "12");
  assert.equal(cronTaskBadge(120), "99+");
  assert.equal(cronTaskBadge(-3), "0");
});

test("cron date formatter rejects invalid values", () => {
  assert.equal(formatCronDate(), undefined);
  assert.equal(formatCronDate("not-a-date"), undefined);
  assert.equal(typeof formatCronDate("2026-08-10T09:00:00+08:00"), "string");
});
