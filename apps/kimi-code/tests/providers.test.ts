import assert from "node:assert/strict";
import test from "node:test";

import {
  createProviderDraft,
  formatContextSize,
  parseContextSize,
  providerDraft,
  saveProviderInput,
  validateProviderDraft,
  type ProviderSummary,
} from "../src/providers.ts";

test("new providers require a valid endpoint, key, and model", () => {
  const draft = createProviderDraft();
  assert.equal(validateProviderDraft(draft, true), "idRequired");

  draft.id = "example-provider";
  draft.apiKey = "YOUR_API_KEY";
  draft.baseUrl = "https://api.example.test/v1";
  draft.models[0]!.model = "example-model";
  draft.models[0]!.maxContextSize = "262144";
  assert.equal(validateProviderDraft(draft, true), undefined);

  draft.models.push({ ...draft.models[0]! });
  assert.equal(validateProviderDraft(draft, true), "modelDuplicate");
});

test("a model default effort must be declared as supported", () => {
  const draft = createProviderDraft();
  draft.id = "example-provider";
  draft.apiKey = "YOUR_API_KEY";
  draft.baseUrl = "https://api.example.test/v1";
  draft.models[0]!.model = "example-model";
  draft.models[0]!.maxContextSize = "262144";
  draft.models[0]!.supportEfforts = ["low", "high"];
  draft.models[0]!.defaultEffort = "max";
  assert.equal(validateProviderDraft(draft, true), "defaultEffortInvalid");

  draft.models[0]!.defaultEffort = "high";
  assert.equal(validateProviderDraft(draft, true), undefined);
  assert.equal(saveProviderInput(draft).models[0]!.defaultEffort, "high");
});

test("context size accepts plain numbers and K/M units", () => {
  assert.equal(parseContextSize("262144"), 262144);
  assert.equal(parseContextSize("256K"), 262144);
  assert.equal(parseContextSize("128k"), 131072);
  assert.equal(parseContextSize("1M"), 1048576);
  assert.equal(parseContextSize("0.5M"), 524288);
  assert.equal(parseContextSize("200 K"), 204800);
  assert.equal(parseContextSize(""), undefined);
  assert.equal(parseContextSize("abc"), undefined);
  assert.equal(parseContextSize("10G"), undefined);
  assert.equal(parseContextSize("0"), undefined);

  assert.equal(formatContextSize(262144), "256K");
  assert.equal(formatContextSize(131072), "128K");
  assert.equal(formatContextSize(1048576), "1M");
  assert.equal(formatContextSize(204800), "200K");
  assert.equal(formatContextSize(100000), "100000");
  assert.equal(formatContextSize(512), "512");

  const draft = createProviderDraft();
  draft.id = "example-provider";
  draft.apiKey = "YOUR_API_KEY";
  draft.baseUrl = "https://api.example.test/v1";
  draft.models[0]!.model = "example-model";
  draft.models[0]!.maxContextSize = "256K";
  assert.equal(validateProviderDraft(draft, true), undefined);
  assert.equal(saveProviderInput(draft).models[0]!.maxContextSize, 262144);
});

test("editing a provider never requires or returns its stored key", () => {
  const provider: ProviderSummary = {
    id: "example-provider",
    type: "openai",
    baseUrl: "https://api.example.test/v1",
    defaultModel: "example-model",
    hasApiKey: true,
    managed: false,
    models: [
      {
        model: "example-model",
        displayName: "Example Model",
        maxContextSize: 131072,
        capabilities: ["tool_use", "thinking"],
        supportEfforts: [],
        defaultEffort: undefined,
        adaptiveThinking: true,
      },
    ],
  };
  const draft = providerDraft(provider);
  assert.equal(draft.apiKey, "");
  assert.equal(draft.replaceApiKey, false);
  assert.equal(draft.models[0]!.maxContextSize, "128K");
  assert.equal(validateProviderDraft(draft, false), undefined);
  assert.deepEqual(saveProviderInput(draft, provider.id), {
    originalId: "example-provider",
    id: "example-provider",
    type: "openai",
    apiKey: undefined,
    replaceApiKey: false,
    baseUrl: "https://api.example.test/v1",
    defaultModel: "example-model",
    models: [
      {
        model: "example-model",
        displayName: "Example Model",
        maxContextSize: 131072,
        capabilities: ["tool_use", "thinking"],
        supportEfforts: [],
        defaultEffort: undefined,
        adaptiveThinking: true,
      },
    ],
  });
});
