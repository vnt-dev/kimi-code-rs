import assert from "node:assert/strict";
import test from "node:test";

import { parseToolMediaOutput } from "../src/chat/toolMediaOutput.ts";

test("parses tagged ReadMediaFile image output", () => {
  const output = [
    { type: "text", text: '<image path="D:/outputs/req-desc-img.png">' },
    {
      type: "image_url",
      imageUrl: { url: "data:image/png;base64,AAAA" },
    },
    { type: "text", text: "</image>" },
  ];

  assert.deepEqual(parseToolMediaOutput(output), {
    items: [
      {
        kind: "image",
        url: "data:image/png;base64,AAAA",
        path: "D:/outputs/req-desc-img.png",
      },
    ],
    remaining: [],
  });
});

test("parses video output and preserves additional result text", () => {
  const output = [
    { type: "text", text: "<video path='/tmp/demo.mp4'>" },
    {
      type: "video_url",
      videoUrl: { url: "data:video/mp4;base64,AAAA" },
    },
    { type: "text", text: "</video>" },
    { type: "text", text: "Video was downsampled." },
  ];

  assert.deepEqual(parseToolMediaOutput(output), {
    items: [
      {
        kind: "video",
        url: "data:video/mp4;base64,AAAA",
        path: "/tmp/demo.mp4",
      },
    ],
    remaining: [{ type: "text", text: "Video was downsampled." }],
  });
});

test("does not claim ordinary or malformed tool results", () => {
  assert.equal(parseToolMediaOutput("done"), undefined);
  assert.equal(
    parseToolMediaOutput([{ type: "image_url", imageUrl: { url: "" } }]),
    undefined,
  );
  assert.equal(
    parseToolMediaOutput([{ type: "text", text: "ordinary output" }]),
    undefined,
  );
});
