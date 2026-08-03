import { localeTag } from "../i18n";
import type { TokenUsage } from "../types";

export function formatTime(timestamp: string | number): string {
  return new Intl.DateTimeFormat(localeTag(), {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

export function formatContext(value: number): string {
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return `${value}`;
}

export function formatBytes(value: number): string {
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  if (value >= 1024) return `${Math.ceil(value / 1024)} KB`;
  return `${value} B`;
}

export function formatTokenCount(value: number): string {
  return Math.max(0, Math.round(value)).toLocaleString("en-US");
}

export function formatCompactTokenCount(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(Math.max(0, Math.round(value)));
}

export function inputTokenUsage(usage?: TokenUsage): number {
  if (!usage) return 0;
  return (
    usage.inputOther +
    usage.inputCacheRead +
    usage.inputCacheCreation
  );
}

export function formatCacheHitRate(usage?: TokenUsage): string {
  if (!usage) return "—";
  const totalInput = inputTokenUsage(usage);
  if (totalInput <= 0) return "0%";
  return `${Math.round((usage.inputCacheRead / totalInput) * 100)}%`;
}
