import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_AUTO_CONVERSATION_TITLES,
  loadAutoConversationTitlesEnabled,
  loadConversationTitleModel,
} from "../src/conversationTitles.ts";

test("automatic conversation titles default to enabled", () => {
  assert.equal(DEFAULT_AUTO_CONVERSATION_TITLES, true);
  assert.equal(loadAutoConversationTitlesEnabled(), true);
  assert.equal(loadConversationTitleModel(), undefined);
});
