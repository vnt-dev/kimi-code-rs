import { t } from "./i18n.ts";
import type { Model } from "./types";

export function thinkingLevelsForModel(model?: Model): string[] {
  return [
    ...new Set(
      (model?.supportEfforts ?? [])
        .map((effort) => effort.trim())
        .filter(Boolean),
    ),
  ];
}

export function normalizeThinkingLevel(
  level: string | undefined,
  model?: Model,
): string {
  const supported = thinkingLevelsForModel(model);
  if (!supported.length) return model?.supportsReasoning ? "on" : "off";
  if (level && supported.includes(level)) return level;
  if (model?.defaultEffort && supported.includes(model.defaultEffort)) {
    return model.defaultEffort;
  }
  return supported[Math.floor(supported.length / 2)];
}

export function thinkingLevelDescription(level: string): string {
  switch (level) {
    case "low":
      return t("thinking.low");
    case "medium":
      return t("thinking.medium");
    case "high":
      return t("thinking.high");
    case "max":
      return t("thinking.max");
    default:
      return t("thinking.custom", { level });
  }
}
