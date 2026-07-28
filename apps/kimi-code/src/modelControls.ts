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
      return "快速响应，适合简单任务";
    case "medium":
      return "速度与推理深度平衡";
    case "high":
      return "更深入分析复杂问题";
    case "max":
      return "最大思考深度，适合最复杂的任务";
    default:
      return `模型支持的 ${level} 思考强度`;
  }
}
