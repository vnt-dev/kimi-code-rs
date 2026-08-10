import assert from "node:assert/strict";
import test from "node:test";

import { hasVisibleLiveUserMessage } from "../src/chat/liveTurnVisibility.ts";

test("hides a model-facing background task turn without visible user content", () => {
  assert.equal(
    hasVisibleLiveUserMessage({
      prompt: "",
      attachments: [],
      pluginCommand: undefined,
    }),
    false,
  );
});

test("keeps visible prompts, attachments, and plugin commands", () => {
  assert.equal(
    hasVisibleLiveUserMessage({
      prompt: "hello",
      attachments: [],
      pluginCommand: undefined,
    }),
    true,
  );
  assert.equal(
    hasVisibleLiveUserMessage({
      prompt: "",
      attachments: [
        {
          id: "attachment-1",
          name: "image.png",
          kind: "image",
          mediaType: "image/png",
          size: 1,
        },
      ],
      pluginCommand: undefined,
    }),
    true,
  );
  assert.equal(
    hasVisibleLiveUserMessage({
      prompt: "",
      attachments: [],
      pluginCommand: {
        pluginId: "plugin-1",
        commandName: "run",
        args: "",
      },
    }),
    true,
  );
});
