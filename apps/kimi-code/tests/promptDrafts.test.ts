import assert from "node:assert/strict";
import test from "node:test";

import {
  promptDraftFor,
  removePromptDrafts,
  updatePromptDraft,
  type PromptDrafts,
} from "../src/promptDrafts.ts";

test("composer drafts remain isolated when conversations switch", () => {
  let drafts: PromptDrafts<string, string> = {};
  drafts = updatePromptDraft(drafts, "conversation-a", "text", "draft A");
  drafts = updatePromptDraft(drafts, "conversation-a", "attachments", [
    "a.png",
  ]);
  drafts = updatePromptDraft(drafts, "conversation-a", "skills", ["review"]);
  drafts = updatePromptDraft(drafts, "conversation-b", "text", "draft B");

  assert.deepEqual(promptDraftFor(drafts, "conversation-a"), {
    text: "draft A",
    attachments: ["a.png"],
    skills: ["review"],
  });
  assert.deepEqual(promptDraftFor(drafts, "conversation-b"), {
    text: "draft B",
    attachments: [],
    skills: [],
  });
});

test("functional updates and removals only affect the targeted conversation", () => {
  let drafts: PromptDrafts<string, string> = {};
  drafts = updatePromptDraft(drafts, "conversation-a", "attachments", [
    "first.png",
  ]);
  drafts = updatePromptDraft(
    drafts,
    "conversation-a",
    "attachments",
    (current) => [...current, "second.png"],
  );
  drafts = updatePromptDraft(drafts, "conversation-b", "text", "keep me");
  drafts = removePromptDrafts(drafts, new Set(["conversation-a"]));

  assert.deepEqual(promptDraftFor(drafts, "conversation-a"), {
    text: "",
    attachments: [],
    skills: [],
  });
  assert.equal(promptDraftFor(drafts, "conversation-b").text, "keep me");
});

test("clearing a submitted draft leaves other conversation input untouched", () => {
  let drafts: PromptDrafts<string, string> = {};
  for (const conversationId of ["conversation-a", "conversation-b"]) {
    drafts = updatePromptDraft(
      drafts,
      conversationId,
      "text",
      `draft ${conversationId}`,
    );
    drafts = updatePromptDraft(drafts, conversationId, "attachments", [
      `${conversationId}.png`,
    ]);
    drafts = updatePromptDraft(drafts, conversationId, "skills", ["review"]);
  }

  drafts = updatePromptDraft(drafts, "conversation-a", "text", "");
  drafts = updatePromptDraft(drafts, "conversation-a", "attachments", []);
  drafts = updatePromptDraft(drafts, "conversation-a", "skills", []);

  assert.deepEqual(promptDraftFor(drafts, "conversation-b"), {
    text: "draft conversation-b",
    attachments: ["conversation-b.png"],
    skills: ["review"],
  });
});
