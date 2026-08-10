export interface CronTaskDescriptor {
  id: string;
  cron: string;
  prompt: string;
  createdAt: number;
  recurring: boolean;
  lastFiredAt?: number;
  humanSchedule: string;
  nextFireAt?: string;
  stale: boolean;
}

export interface CreateCronTaskInput {
  sessionId: string;
  cron: string;
  prompt: string;
  recurring: boolean;
}

export interface DeleteCronTaskInput {
  sessionId: string;
  id: string;
}

export const CRON_QUICK_PRESETS = [
  { key: "fifteenMinutes", cron: "*/15 * * * *" },
  { key: "hourly", cron: "7 * * * *" },
  { key: "daily", cron: "0 9 * * *" },
  { key: "weekdays", cron: "0 9 * * 1-5" },
] as const;

export function cronTaskBadge(count: number): string {
  return count > 99 ? "99+" : String(Math.max(0, count));
}

export function formatCronDate(value?: string | number): string | undefined {
  if (value === undefined) return undefined;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return undefined;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
