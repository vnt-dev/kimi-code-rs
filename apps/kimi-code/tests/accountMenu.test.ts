import assert from "node:assert/strict";
import test from "node:test";

import { resolveAccountMenuVisibility } from "../src/accountMenu.ts";

test("signed-out account menu only offers sign-in", () => {
  assert.deepEqual(resolveAccountMenuVisibility(false), {
    showLogin: true,
    showUsage: false,
    showSignOut: false,
  });
});

test("signed-in account menu shows usage and sign-out", () => {
  assert.deepEqual(resolveAccountMenuVisibility(true), {
    showLogin: false,
    showUsage: true,
    showSignOut: true,
  });
});
