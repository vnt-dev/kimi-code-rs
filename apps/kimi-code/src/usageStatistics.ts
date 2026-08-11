export interface DailyTokenUsage {
  date: string;
  totalTokens: number;
}

export interface UsageStatistics {
  totalTokens: number;
  peakDailyTokens: number;
  longestTaskMs: number;
  currentStreakDays: number;
  longestStreakDays: number;
  days: DailyTokenUsage[];
}

export type HeatmapMode = "daily" | "weekly" | "cumulative";

export interface HeatmapCell {
  date: string;
  weekStartDate: string;
  column: number;
  row: number;
  dayTokens: number;
  intensityValue: number;
  level: number;
}

export interface HeatmapMonthLabel {
  column: number;
  label: string;
}

export interface WeeklyHeatmapColumn {
  column: number;
  weekStartDate: string;
  weekEndDate: string;
  totalTokens: number;
  filledCells: number;
  level: number;
  cumulativeTokens: number;
  cumulativeFilledCells: number;
  cumulativeLevel: number;
}

export interface HeatmapData {
  cells: HeatmapCell[];
  weeks: WeeklyHeatmapColumn[];
  monthLabels: HeatmapMonthLabel[];
  maxIntensity: number;
}

export const HEATMAP_WEEKS = 53;
export const HEATMAP_ROWS = 7;

function localDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function addDays(date: Date, count: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + count);
}

function intensityLevel(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0;
  return Math.min(4, Math.ceil((value / max) * 4));
}

export function buildHeatmap(
  days: DailyTokenUsage[],
  mode: HeatmapMode,
  todayInput = new Date(),
  locale = "zh-CN",
): HeatmapData {
  const today = startOfDay(todayInput);
  const daysSinceMonday = (today.getDay() + 6) % HEATMAP_ROWS;
  const currentWeekMonday = addDays(today, -daysSinceMonday);
  const gridStart = addDays(
    currentWeekMonday,
    -(HEATMAP_WEEKS - 1) * HEATMAP_ROWS,
  );
  const gridStartKey = localDateKey(gridStart);
  const tokensByDay = new Map<string, number>();
  for (const day of days) {
    if (!Number.isFinite(day.totalTokens) || day.totalTokens <= 0) continue;
    tokensByDay.set(
      day.date,
      (tokensByDay.get(day.date) ?? 0) + day.totalTokens,
    );
  }

  const weekTotals = Array.from({ length: HEATMAP_WEEKS }, (_, column) => {
    let total = 0;
    for (let row = 0; row < HEATMAP_ROWS; row += 1) {
      const date = addDays(gridStart, column * HEATMAP_ROWS + row);
      if (date <= today) total += tokensByDay.get(localDateKey(date)) ?? 0;
    }
    return total;
  });

  const cumulativeBeforeGrid = days.reduce(
    (total, day) =>
      day.date < gridStartKey && Number.isFinite(day.totalTokens)
        ? total + Math.max(0, day.totalTokens)
        : total,
    0,
  );
  let cumulative = cumulativeBeforeGrid;
  const rawCells: Omit<HeatmapCell, "level">[] = [];
  for (let column = 0; column < HEATMAP_WEEKS; column += 1) {
    for (let row = 0; row < HEATMAP_ROWS; row += 1) {
      const date = addDays(gridStart, column * HEATMAP_ROWS + row);
      if (date > today) continue;
      const dateKey = localDateKey(date);
      const weekStartDate = localDateKey(
        addDays(gridStart, column * HEATMAP_ROWS),
      );
      const dayTokens = tokensByDay.get(dateKey) ?? 0;
      cumulative += dayTokens;
      const intensityValue =
        mode === "weekly"
          ? weekTotals[column]
          : mode === "cumulative"
            ? cumulative
            : dayTokens;
      rawCells.push({
        date: dateKey,
        weekStartDate,
        column,
        row,
        dayTokens,
        intensityValue,
      });
    }
  }

  const maxIntensity = rawCells.reduce(
    (max, cell) => Math.max(max, cell.intensityValue),
    0,
  );
  const cells = rawCells.map((cell) => ({
    ...cell,
    level: intensityLevel(cell.intensityValue, maxIntensity),
  }));

  const maxWeeklyTokens = weekTotals.reduce(
    (max, total) => Math.max(max, total),
    0,
  );
  let cumulativeByWeek = cumulativeBeforeGrid;
  const cumulativeWeekTotals = weekTotals.map((totalTokens) => {
    cumulativeByWeek += totalTokens;
    return cumulativeByWeek;
  });
  const maxCumulativeTokens = cumulativeWeekTotals.reduce(
    (max, total) => Math.max(max, total),
    0,
  );
  const weeks = weekTotals.map((totalTokens, column) => {
    const weekStart = addDays(gridStart, column * HEATMAP_ROWS);
    const cumulativeTokens = cumulativeWeekTotals[column];
    return {
      column,
      weekStartDate: localDateKey(weekStart),
      weekEndDate: localDateKey(addDays(weekStart, HEATMAP_ROWS - 1)),
      totalTokens,
      filledCells:
        totalTokens > 0 && maxWeeklyTokens > 0
          ? Math.max(1, Math.ceil((totalTokens / maxWeeklyTokens) * HEATMAP_ROWS))
          : 0,
      level: intensityLevel(totalTokens, maxWeeklyTokens),
      cumulativeTokens,
      cumulativeFilledCells:
        cumulativeTokens > 0 && maxCumulativeTokens > 0
          ? Math.max(
              1,
              Math.ceil(
                (cumulativeTokens / maxCumulativeTokens) * HEATMAP_ROWS,
              ),
            )
          : 0,
      cumulativeLevel: intensityLevel(
        cumulativeTokens,
        maxCumulativeTokens,
      ),
    };
  });

  const monthFormatter = new Intl.DateTimeFormat(locale, { month: "short" });
  const monthLabels: HeatmapMonthLabel[] = [];
  let previousMonth = -1;
  for (let column = 0; column < HEATMAP_WEEKS; column += 1) {
    const date = addDays(gridStart, column * HEATMAP_ROWS);
    if (date > today) break;
    if (date.getMonth() === previousMonth) continue;
    previousMonth = date.getMonth();
    monthLabels.push({
      column,
      label: monthFormatter.format(date),
    });
  }

  return { cells, weeks, monthLabels, maxIntensity };
}

export function formatTokenCount(value: number, locale: string): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  return new Intl.NumberFormat(locale, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

export function formatHeatmapTokenCount(value: number, locale: string): string {
  const normalized = Number.isFinite(value) ? Math.max(0, value) : 0;
  if (locale.startsWith("zh")) {
    const compact = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });
    if (normalized >= 100_000_000) {
      return `${compact.format(normalized / 100_000_000)}亿`;
    }
    if (normalized >= 10_000) {
      return `${compact.format(normalized / 10_000)}万`;
    }
    return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(
      normalized,
    );
  }
  if (normalized < 1_000) {
    return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(
      normalized,
    );
  }
  return new Intl.NumberFormat(locale, {
    notation: "compact",
    compactDisplay: "short",
    maximumFractionDigits: 1,
  }).format(normalized);
}

export function heatmapTooltipDatum(
  cell: HeatmapCell,
  mode: HeatmapMode,
): { date: string; tokens: number } {
  return mode === "weekly"
    ? { date: cell.weekStartDate, tokens: cell.intensityValue }
    : { date: cell.date, tokens: cell.dayTokens };
}

export function formatHeatmapDate(dateKey: string, locale: string): string {
  const [year, month, day] = dateKey.split("-").map(Number);
  const date = new Date(year, month - 1, day);
  if (
    !Number.isInteger(year) ||
    !Number.isInteger(month) ||
    !Number.isInteger(day) ||
    Number.isNaN(date.getTime())
  ) {
    return dateKey;
  }
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "long",
    day: "numeric",
  }).format(date);
}

export function formatTaskDuration(milliseconds: number, locale: string): string {
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) {
    return locale.startsWith("zh") ? "0 分" : "0m";
  }
  const totalMinutes = Math.max(1, Math.floor(milliseconds / 60_000));
  const days = Math.floor(totalMinutes / (24 * 60));
  const hours = Math.floor((totalMinutes % (24 * 60)) / 60);
  const minutes = totalMinutes % 60;

  if (locale.startsWith("zh")) {
    if (days > 0) {
      return hours > 0 ? `${days} 天 ${hours} 小时` : `${days} 天`;
    }
    if (hours > 0) {
      return minutes > 0 ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`;
    }
    return `${totalMinutes} 分`;
  }
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${totalMinutes}m`;
}
