import type { TokenUsage } from "../types";

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

export function formatTime(
  timestamp: string | number,
  now = new Date(),
): string {
  const date = new Date(timestamp);
  const time = `${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
  const isToday =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();

  if (isToday) return time;

  const monthAndDay = `${twoDigits(date.getMonth() + 1)}-${twoDigits(date.getDate())}`;
  return date.getFullYear() === now.getFullYear()
    ? `${monthAndDay} ${time}`
    : `${date.getFullYear()}-${monthAndDay} ${time}`;
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
