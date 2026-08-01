import assert from "node:assert/strict";
import test from "node:test";

import {
  MOBILE_LAYOUT_MAX_WIDTH,
  resolveSidebarCollapsed,
  shouldUseWebMobileLayout,
} from "../src/responsive.ts";

test("mobile layout is limited to narrow Web viewports", () => {
  assert.equal(MOBILE_LAYOUT_MAX_WIDTH, 760);
  assert.equal(shouldUseWebMobileLayout(false, true), true);
  assert.equal(shouldUseWebMobileLayout(false, false), false);
  assert.equal(shouldUseWebMobileLayout(true, true), false);
});

test("mobile drawer state is independent from desktop sidebar collapse", () => {
  assert.equal(resolveSidebarCollapsed(true, false, false), true);
  assert.equal(resolveSidebarCollapsed(true, true, true), false);
  assert.equal(resolveSidebarCollapsed(false, false, false), false);
  assert.equal(resolveSidebarCollapsed(false, true, true), true);
});
