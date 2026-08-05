import assert from "node:assert/strict";
import test from "node:test";

import {
  isSameLiveUserMessage,
  projectLiveUserMessage,
} from "../src/liveUserMessage.ts";
import type { LiveUserMessage } from "../src/types.ts";

const message: LiveUserMessage = {
  promptId: "prompt-1",
  userMessageId: "message-1",
  createdAt: "2026-08-01T00:00:00.000Z",
  content: [
    { type: "text", text: "hello " },
    { type: "text", text: "world" },
    {
      type: "image",
      source: { kind: "base64", media_type: "image/png", data: "aGVsbG8=" },
    },
    {
      type: "video",
      source: { kind: "url", url: "data:video/mp4;base64,dmlkZW8=" },
    },
    {
      type: "file",
      file_id: "file-1",
      name: "notes.txt",
      media_type: "text/plain",
      size: 42,
    },
  ],
};

test("structured live user messages rebuild text and attachments", () => {
  const projected = projectLiveUserMessage(message);
  assert.equal(projected.text, "hello world");
  assert.deepEqual(projected.attachments, [
    {
      id: "message-1-2",
      fileId: undefined,
      name: "image-3",
      dataUrl: "data:image/png;base64,aGVsbG8=",
      kind: "image",
      mediaType: "image/png",
      size: 0,
    },
    {
      id: "message-1-3",
      fileId: undefined,
      name: "video-4",
      dataUrl: "data:video/mp4;base64,dmlkZW8=",
      kind: "video",
      mediaType: "video/*",
      size: 0,
    },
    {
      id: "file-1",
      fileId: "file-1",
      name: "notes.txt",
      kind: "file",
      mediaType: "text/plain",
      size: 42,
    },
  ]);
});

test("prompt and user message identifiers both deduplicate replay", () => {
  assert.equal(isSameLiveUserMessage({ promptId: "prompt-1" }, message), true);
  assert.equal(
    isSameLiveUserMessage({ userMessageId: "message-1" }, message),
    true,
  );
  assert.equal(isSameLiveUserMessage({ promptId: "other" }, message), false);
});

test("user-slash plugin commands display the command instead of expanded markdown", () => {
  const origin = {
    kind: "plugin_command",
    activationId: "activation-1",
    pluginId: "vercel-plugin",
    commandName: "status",
    commandArgs: "--json",
    trigger: "user-slash",
  };
  const projected = projectLiveUserMessage({
    ...message,
    content: [{ type: "text", text: "# Expanded plugin instructions" }],
    origin,
  });
  assert.equal(projected.text, "/vercel-plugin:status --json");
  assert.deepEqual(projected.pluginCommand, {
    pluginId: "vercel-plugin",
    commandName: "status",
    args: "--json",
  });

});

test("non-command messages continue to display their original content", () => {
  const projected = projectLiveUserMessage({
    ...message,
    content: [{ type: "text", text: "ordinary prompt" }],
    origin: { kind: "user" },
  });
  assert.equal(projected.text, "ordinary prompt");
  assert.equal(projected.pluginCommand, undefined);
});
