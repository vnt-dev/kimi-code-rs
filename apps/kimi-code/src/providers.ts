export type ProviderProtocol =
  | "kimi"
  | "openai"
  | "openai_responses"
  | "anthropic"
  | "google-genai";

export interface ProviderModel {
  model: string;
  displayName?: string;
  maxContextSize: number;
  capabilities: string[];
  supportEfforts: string[];
  adaptiveThinking?: boolean;
}

export interface ProviderSummary {
  id: string;
  type: ProviderProtocol;
  baseUrl?: string;
  defaultModel?: string;
  hasApiKey: boolean;
  managed: boolean;
  models: ProviderModel[];
}

export interface ProviderModelDraft {
  model: string;
  displayName: string;
  maxContextSize: string;
  capabilities: string[];
  supportEfforts: string[];
  adaptiveThinking?: boolean;
}

export interface ProviderDraft {
  id: string;
  type: ProviderProtocol;
  apiKey: string;
  replaceApiKey: boolean;
  baseUrl: string;
  defaultModel: string;
  models: ProviderModelDraft[];
}

export interface SaveProviderInput {
  originalId?: string;
  id: string;
  type: ProviderProtocol;
  apiKey?: string;
  replaceApiKey: boolean;
  baseUrl: string;
  defaultModel?: string;
  models: ProviderModel[];
}

export type ProviderValidationError =
  | "idRequired"
  | "idInvalid"
  | "apiKeyRequired"
  | "baseUrlRequired"
  | "baseUrlInvalid"
  | "modelRequired"
  | "modelDuplicate"
  | "contextRequired"
  | "contextInvalid";

export const PROVIDER_PROTOCOLS: readonly ProviderProtocol[] = [
  "kimi",
  "openai",
  "openai_responses",
  "anthropic",
  "google-genai",
];

export function createProviderModelDraft(): ProviderModelDraft {
  return {
    model: "",
    displayName: "",
    maxContextSize: "",
    capabilities: ["tool_use", "thinking"],
    supportEfforts: [],
    adaptiveThinking: true,
  };
}

export function createProviderDraft(): ProviderDraft {
  return {
    id: "",
    type: "openai",
    apiKey: "",
    replaceApiKey: true,
    baseUrl: "",
    defaultModel: "",
    models: [createProviderModelDraft()],
  };
}

export function providerDraft(provider: ProviderSummary): ProviderDraft {
  return {
    id: provider.id,
    type: provider.type,
    apiKey: "",
    replaceApiKey: false,
    baseUrl: provider.baseUrl ?? "",
    defaultModel: provider.defaultModel ?? provider.models[0]?.model ?? "",
    models: provider.models.map((model) => ({
      model: model.model,
      displayName: model.displayName ?? "",
      maxContextSize: model.maxContextSize ? String(model.maxContextSize) : "",
      capabilities: [...model.capabilities],
      supportEfforts: [...model.supportEfforts],
      adaptiveThinking: model.adaptiveThinking,
    })),
  };
}

export function validateProviderDraft(
  draft: ProviderDraft,
  adding: boolean,
): ProviderValidationError | undefined {
  const id = draft.id.trim();
  if (!id) return "idRequired";
  if (!/^[\p{L}\p{N}][\p{L}\p{N}\-_ ]*$/u.test(id) || id.length > 64) {
    return "idInvalid";
  }
  if (adding && !draft.apiKey.trim()) return "apiKeyRequired";
  const baseUrl = draft.baseUrl.trim();
  if (!baseUrl) return "baseUrlRequired";
  try {
    const parsed = new URL(baseUrl);
    if (!/^https?:$/.test(parsed.protocol) || !parsed.hostname) return "baseUrlInvalid";
  } catch {
    return "baseUrlInvalid";
  }
  if (draft.models.length === 0) return "modelRequired";
  const modelIds = new Set<string>();
  for (const model of draft.models) {
    const id = model.model.trim();
    if (!id) return "modelRequired";
    if (modelIds.has(id)) return "modelDuplicate";
    modelIds.add(id);
    if (!model.maxContextSize.trim()) return "contextRequired";
    if (!/^\d+$/.test(model.maxContextSize.trim()) || Number(model.maxContextSize) < 1) {
      return "contextInvalid";
    }
  }
  return undefined;
}

export function saveProviderInput(
  draft: ProviderDraft,
  originalId?: string,
): SaveProviderInput {
  const apiKey = draft.apiKey.trim();
  return {
    originalId,
    id: draft.id.trim(),
    type: draft.type,
    apiKey: apiKey || undefined,
    replaceApiKey: draft.replaceApiKey,
    baseUrl: draft.baseUrl.trim(),
    defaultModel: draft.defaultModel.trim() || draft.models[0]?.model.trim() || undefined,
    models: draft.models.map((model) => ({
      model: model.model.trim(),
      displayName: model.displayName.trim() || undefined,
      maxContextSize: Number(model.maxContextSize.trim()),
      capabilities: [...model.capabilities],
      supportEfforts: [...model.supportEfforts],
      adaptiveThinking: model.adaptiveThinking,
    })),
  };
}
