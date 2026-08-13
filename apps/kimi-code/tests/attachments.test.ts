import assert from "node:assert/strict";
import test from "node:test";

import { buildAgentPromptInput } from "../src/prompt/attachments.ts";
import type { PromptAttachment } from "../src/chat/liveTurns.ts";

function attachment(
  kind: PromptAttachment["kind"],
  fileId: string,
): PromptAttachment {
  return {
    id: fileId,
    fileId,
    name: `${kind}.bin`,
    dataUrl: `data:${kind}/*;base64,AAAA`,
    kind,
    mediaType: `${kind}/*`,
    size: 4,
  };
}

test("prompt media uses uploaded file references instead of inline data URLs", () => {
  const input = buildAgentPromptInput("inspect", [
    attachment("image", "f_image"),
    attachment("audio", "f_audio"),
    attachment("video", "f_video"),
  ]);

  assert.deepEqual(input, [
    { type: "text", text: "inspect" },
    { type: "image_file", file_id: "f_image" },
    { type: "audio_file", file_id: "f_audio" },
    { type: "video_file", file_id: "f_video" },
  ]);
  assert.equal(JSON.stringify(input).includes("base64"), false);
});

