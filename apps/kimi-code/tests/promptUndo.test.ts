import assert from "node:assert/strict";
import test from "node:test";

import {
  canUndoPromptEdit,
  createPromptUndoHistory,
  recordPromptEdit,
  recordPromptInput,
  undoPromptEdit,
} from "../src/promptUndo.ts";

test("undo stops at the cleared draft before restoring the previous text", () => {
  let history = createPromptUndoHistory();
  history = recordPromptEdit(history, "你", 0);
  history = recordPromptEdit(history, "你好", 100);
  history = recordPromptEdit(history, "你", 200);
  history = recordPromptEdit(history, "", 300);
  history = recordPromptEdit(history, "1", 400);
  history = recordPromptEdit(history, "12", 500);
  history = recordPromptEdit(history, "123", 600);

  history = undoPromptEdit(history);
  assert.equal(history.current, "");
  history = undoPromptEdit(history);
  assert.equal(history.current, "你好");
});

test("continuous typing is undone as one edit", () => {
  let history = createPromptUndoHistory();
  history = recordPromptEdit(history, "h", 0);
  history = recordPromptEdit(history, "he", 100);
  history = recordPromptEdit(history, "hello", 200);

  assert.equal(canUndoPromptEdit(history), true);
  history = undoPromptEdit(history);
  assert.equal(history.current, "");
  assert.equal(canUndoPromptEdit(history), false);
});

test("IME composition only records the committed text", () => {
  let history = createPromptUndoHistory();
  history = recordPromptInput(history, "ni", {
    isComposing: true,
    at: 100,
  });
  history = recordPromptInput(history, "ni'hao", {
    isComposing: true,
    at: 200,
  });
  history = recordPromptInput(history, "你好", {
    isComposing: false,
    at: 500,
  });

  assert.deepEqual(history.past, [""]);
  assert.equal(history.past.includes("ni'hao"), false);
  assert.equal(undoPromptEdit(history).current, "");
});

test("separate typing bursts keep separate undo checkpoints", () => {
  let history = createPromptUndoHistory();
  history = recordPromptEdit(history, "hello", 0);
  history = recordPromptEdit(history, "hello world", 1_500);

  assert.equal(undoPromptEdit(history).current, "hello");
});

test("replacing selected text can be undone", () => {
  let history = createPromptUndoHistory("你好");
  history = recordPromptEdit(history, "123", 0);

  assert.equal(undoPromptEdit(history).current, "你好");
});
