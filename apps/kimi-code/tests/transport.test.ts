import assert from "node:assert/strict";
import test from "node:test";

import {
  credentialFromFragment,
  credentialFromStoredValue,
  isDesktop,
  storedCredentialValue,
} from "../src/transport.ts";

test("browser transport reads token fragments without including other fields", () => {
  assert.equal(credentialFromFragment("#token=secret-value"), "secret-value");
  assert.equal(
    credentialFromFragment("#view=chat&token=encoded%2Dtoken"),
    "encoded-token",
  );
  assert.equal(credentialFromFragment("#view=chat"), undefined);
});

test("stored Web credentials expire after seven days", () => {
  const now = Date.UTC(2026, 7, 1);
  const serialized = storedCredentialValue("secret-value", now);
  assert.equal(
    credentialFromStoredValue(serialized, now + 7 * 24 * 60 * 60 * 1000 - 1),
    "secret-value",
  );
  assert.equal(
    credentialFromStoredValue(serialized, now + 7 * 24 * 60 * 60 * 1000),
    undefined,
  );
  assert.equal(credentialFromStoredValue("not-json", now), undefined);
});

test("node test environment selects the Web adapter", () => {
  assert.equal(isDesktop(), false);
});
