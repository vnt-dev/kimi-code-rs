import assert from "node:assert/strict";
import test from "node:test";

import {
  normalizeThinkingLevel,
  thinkingLevelsForModel,
} from "../src/modelControls.ts";
import type { Model } from "../src/types.ts";

function model(overrides: Partial<Model>): Model {
  return {
    id: "test/model",
    model: "model",
    providerId: "test",
    isDefault: false,
    displayName: "Test model",
    contextLength: 128_000,
    supportsReasoning: true,
    supportsImage: false,
    supportsVideo: false,
    supportsTools: true,
    protocol: "openai",
    supportEfforts: [],
    ...overrides,
  };
}

test("K3 exposes exactly its configured thinking efforts including max", () => {
  const k3 = model({
    id: "kimi-code/k3",
    model: "k3",
    displayName: "K3",
    supportEfforts: ["low", "high", "max"],
    defaultEffort: "high",
  });

  assert.deepEqual(thinkingLevelsForModel(k3), ["low", "high", "max"]);
  assert.equal(normalizeThinkingLevel("max", k3), "max");
  assert.equal(normalizeThinkingLevel("medium", k3), "high");
});

test("a thinking model without support_efforts has no selectable effort", () => {
  const kimiForCoding = model({
    id: "kimi-code/kimi-for-coding",
    model: "kimi-for-coding",
    displayName: "K2.7 Coding",
  });

  assert.deepEqual(thinkingLevelsForModel(kimiForCoding), []);
  assert.equal(normalizeThinkingLevel("high", kimiForCoding), "on");
});

test("configured efforts are trimmed, deduplicated, and keep their order", () => {
  const configured = model({
    supportEfforts: [" low ", "high", "low", "", "max"],
  });

  assert.deepEqual(thinkingLevelsForModel(configured), ["low", "high", "max"]);
});
