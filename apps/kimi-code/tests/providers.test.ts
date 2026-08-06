import assert from "node:assert/strict";
import test from "node:test";

import {
  createProviderDraft,
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
        adaptiveThinking: true,
      },
    ],
  };
  const draft = providerDraft(provider);
  assert.equal(draft.apiKey, "");
  assert.equal(draft.replaceApiKey, false);
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
        adaptiveThinking: true,
      },
    ],
  });
});
