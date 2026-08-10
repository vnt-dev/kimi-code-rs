import assert from "node:assert/strict";
import test from "node:test";

import { parseCronFireMessage } from "../src/cronFire.ts";

test("parses a cron fire envelope without exposing its job id", () => {
  const parsed = parseCronFireMessage(`<cron-fire jobId="job-secret" cron="22 19 10 8 *" recurring="false" coalescedCount="1" stale="false">
<prompt>
提醒用户：一分钟到了。
</prompt>
</cron-fire>`);

  assert.deepEqual(parsed, {
    cron: "22 19 10 8 *",
    recurring: false,
    coalescedCount: 1,
    stale: false,
    prompt: "提醒用户：一分钟到了。",
  });
  assert.equal(JSON.stringify(parsed).includes("job-secret"), false);
});

test("decodes attributes and preserves embedded prompt markup", () => {
  const parsed = parseCronFireMessage(`<cron-fire jobId="a&amp;b" cron="0 &quot; * * *" recurring="true" coalescedCount="3" stale="true">
<prompt>
检查 <status>ok</status>
</prompt>
</cron-fire>`);

  assert.equal(parsed?.cron, '0 " * * *');
  assert.equal(parsed?.prompt, "检查 <status>ok</status>");
  assert.equal(parsed?.coalescedCount, 3);
  assert.equal(parsed?.stale, true);
});

test("leaves incomplete or invalid cron markup as a regular message", () => {
  assert.equal(parseCronFireMessage("<cron-fire>hello</cron-fire>"), undefined);
  assert.equal(
    parseCronFireMessage(`<cron-fire cron="* * * * *" recurring="true" coalescedCount="0" stale="false"><prompt>hello</prompt></cron-fire>`),
    undefined,
  );
});
