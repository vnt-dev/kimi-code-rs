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
  defaultEffort?: string;
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
  defaultEffort: string;
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
  | "contextInvalid"
  | "defaultEffortInvalid";

export const DEFAULT_CONTEXT_SIZE = 256 * 1024;

export const PROVIDER_PROTOCOLS: readonly ProviderProtocol[] = [
  "kimi",
  "openai",
  "openai_responses",
  "anthropic",
  "google-genai",
];

export const PROVIDER_CAPABILITIES = [
  "tool_use",
  "thinking",
  "always_thinking",
  "image_in",
  "video_in",
  "audio_in",
  "dynamically_loaded_tools",
] as const;

export const PROVIDER_EFFORTS = ["low", "medium", "high", "max"] as const;

export function parseContextSize(input: string): number | undefined {
  const match = /^(\d+(?:\.\d+)?)\s*([kKmM])?$/.exec(input.trim());
  if (!match) return undefined;
  const multiplier = match[2]?.toLowerCase() === "k"
    ? 1024
    : match[2]?.toLowerCase() === "m"
      ? 1024 * 1024
      : 1;
  const size = Math.round(Number(match[1]) * multiplier);
  return size >= 1 ? size : undefined;
}

export function formatContextSize(size: number): string {
  if (size >= 1024 * 1024 && size % (1024 * 1024) === 0) {
    return `${size / (1024 * 1024)}M`;
  }
  if (size >= 1024 && size % 1024 === 0) {
    return `${size / 1024}K`;
  }
  return String(size);
}

export function createProviderModelDraft(): ProviderModelDraft {
  return {
    model: "",
    displayName: "",
    maxContextSize: "",
    capabilities: ["tool_use", "thinking"],
    supportEfforts: [],
    defaultEffort: "",
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
      maxContextSize: model.maxContextSize ? formatContextSize(model.maxContextSize) : "",
      capabilities: [...model.capabilities],
      supportEfforts: [...model.supportEfforts],
      defaultEffort: model.defaultEffort ?? "",
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
    if (!model.maxContextSize.trim()) continue;
    if (parseContextSize(model.maxContextSize) === undefined) {
      return "contextInvalid";
    }
    if (
      model.defaultEffort &&
      !model.supportEfforts.includes(model.defaultEffort)
    ) {
      return "defaultEffortInvalid";
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
      maxContextSize: parseContextSize(model.maxContextSize) ?? DEFAULT_CONTEXT_SIZE,
      capabilities: [...model.capabilities],
      supportEfforts: [...model.supportEfforts],
      defaultEffort: model.defaultEffort || undefined,
      adaptiveThinking: model.adaptiveThinking,
    })),
  };
}
