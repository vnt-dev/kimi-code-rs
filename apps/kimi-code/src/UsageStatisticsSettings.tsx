import { useMemo, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { RefreshCw } from "lucide-react";

import { t, type Language } from "./i18n";
import {
  buildHeatmap,
  formatHeatmapDate,
  formatHeatmapTokenCount,
  formatTaskDuration,
  formatTokenCount,
  heatmapTooltipDatum,
  type HeatmapCell,
  type HeatmapMode,
  type UsageStatistics,
  type WeeklyHeatmapColumn,
} from "./usageStatistics";

const CELL_SIZE = 11;
const CELL_GAP = 3;
const CELL_STEP = CELL_SIZE + CELL_GAP;
const GRID_WIDTH = 53 * CELL_STEP - CELL_GAP;
const GRID_HEIGHT = 7 * CELL_STEP - CELL_GAP;
const LABEL_HEIGHT = 18;

interface TooltipState {
  text: string;
  left: number;
  top: number;
}

export default function UsageStatisticsSettings({
  statistics,
  busy,
  error,
  language,
  onRetry,
}: {
  statistics?: UsageStatistics;
  busy: boolean;
  error?: string;
  language: Language;
  onRetry: () => void;
}) {
  const [mode, setMode] = useState<HeatmapMode>("daily");
  const [tooltip, setTooltip] = useState<TooltipState>();
  const [hoveredWeek, setHoveredWeek] = useState<number>();
  const locale = language === "zh" ? "zh-CN" : "en-US";
  const heatmap = useMemo(
    () => buildHeatmap(statistics?.days ?? [], mode, new Date(), locale),
    [locale, mode, statistics?.days],
  );

  if (!statistics && busy) {
    return (
      <div className="settings-usage-state" role="status">
        <RefreshCw className="spinning" size={18} />
        <span>{t("settings.usageLoading")}</span>
      </div>
    );
  }

  if (!statistics && error) {
    return (
      <div className="settings-usage-state settings-usage-error" role="alert">
        <strong>{t("settings.usageUnavailable")}</strong>
        <span>{error}</span>
        <button type="button" onClick={onRetry}>
          {t("settings.usageRetry")}
        </button>
      </div>
    );
  }

  const data: UsageStatistics = statistics ?? {
    totalTokens: 0,
    peakDailyTokens: 0,
    longestTaskMs: 0,
    currentStreakDays: 0,
    longestStreakDays: 0,
    days: [],
  };
  const metrics = [
    {
      value: formatTokenCount(data.totalTokens, locale),
      label: t("settings.usageTotalTokens"),
    },
    {
      value: formatTokenCount(data.peakDailyTokens, locale),
      label: t("settings.usagePeakTokens"),
    },
    {
      value: formatTaskDuration(data.longestTaskMs, locale),
      label: t("settings.usageLongestTask"),
    },
    {
      value: t("settings.usageDaysValue", { count: data.currentStreakDays }),
      label: t("settings.usageCurrentStreak"),
    },
    {
      value: t("settings.usageDaysValue", { count: data.longestStreakDays }),
      label: t("settings.usageLongestStreak"),
    },
  ];

  const showTooltip = (
    event: MouseEvent<SVGRectElement>,
    cell: HeatmapCell,
  ): void => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const datum = heatmapTooltipDatum(cell, mode);
    const text =
      datum.tokens > 0
        ? t(mode === "weekly" ? "settings.usageHeatmapWeeklyTooltip" : "settings.usageHeatmapTooltip", {
            date:
              mode === "weekly"
                ? formatHeatmapDate(datum.date, locale)
                : datum.date,
            tokens: formatHeatmapTokenCount(datum.tokens, locale),
          })
        : t(mode === "weekly" ? "settings.usageHeatmapWeeklyEmpty" : "settings.usageHeatmapEmpty", {
            date:
              mode === "weekly"
                ? formatHeatmapDate(datum.date, locale)
                : datum.date,
          });
    setTooltip({
      text,
      left: bounds.left + bounds.width / 2,
      top: bounds.top,
    });
  };

  const showWeekColumnTooltip = (
    event: MouseEvent<SVGRectElement>,
    week: WeeklyHeatmapColumn,
  ): void => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const cumulativeMode = mode === "cumulative";
    const tokens = cumulativeMode ? week.cumulativeTokens : week.totalTokens;
    const text = cumulativeMode
      ? tokens > 0
        ? t("settings.usageHeatmapCumulativeTooltip", {
            date: formatHeatmapDate(week.weekEndDate, locale),
            tokens: formatHeatmapTokenCount(tokens, locale),
          })
        : t("settings.usageHeatmapCumulativeEmpty", {
            date: formatHeatmapDate(week.weekEndDate, locale),
          })
      : tokens > 0
        ? t("settings.usageHeatmapWeeklyTooltip", {
            date: formatHeatmapDate(week.weekStartDate, locale),
            tokens: formatHeatmapTokenCount(tokens, locale),
          })
        : t("settings.usageHeatmapWeeklyEmpty", {
            date: formatHeatmapDate(week.weekStartDate, locale),
          });
    setHoveredWeek(week.column);
    setTooltip({
      text,
      left: bounds.left + bounds.width / 2,
      top: bounds.top,
    });
  };

  return (
    <section className="settings-usage" aria-labelledby="usage-heatmap-heading">
      <div className="settings-usage-summary">
        {metrics.map((metric) => (
          <div className="settings-usage-metric" key={metric.label}>
            <strong title={metric.value}>{metric.value}</strong>
            <span>{metric.label}</span>
          </div>
        ))}
      </div>

      {error && (
        <div className="settings-usage-inline-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={onRetry}>
            {t("settings.usageRetry")}
          </button>
        </div>
      )}

      <div className="settings-usage-activity-header">
        <h3 id="usage-heatmap-heading">{t("settings.usageHeatmapTitle")}</h3>
        <div className="settings-usage-modes" role="group" aria-label={t("settings.usageHeatmapModes")}>
          {(["daily", "weekly", "cumulative"] as const).map((value) => (
            <button
              className={mode === value ? "active" : undefined}
              type="button"
              aria-pressed={mode === value}
              key={value}
              onClick={() => {
                setMode(value);
                setHoveredWeek(undefined);
                setTooltip(undefined);
              }}
            >
              {t(`settings.usageMode.${value}`)}
            </button>
          ))}
        </div>
      </div>

      <div className="settings-usage-heatmap-scroll">
        <svg
          className="settings-usage-heatmap"
          width={GRID_WIDTH}
          height={GRID_HEIGHT + LABEL_HEIGHT}
          viewBox={`0 0 ${GRID_WIDTH} ${GRID_HEIGHT + LABEL_HEIGHT}`}
          role="img"
          aria-label={t("settings.usageHeatmapAria")}
        >
          {mode !== "daily"
            ? heatmap.weeks.flatMap((week) =>
                Array.from({ length: 7 }, (_, row) => {
                  const filledCells =
                    mode === "cumulative"
                      ? week.cumulativeFilledCells
                      : week.filledCells;
                  const level =
                    mode === "cumulative" ? week.cumulativeLevel : week.level;
                  const filled = row >= 7 - filledCells;
                  return (
                    <rect
                      className={`settings-usage-cell settings-usage-week-cell ${
                        filled ? `level-${level}` : "level-0"
                      }${hoveredWeek === week.column ? " column-hovered" : ""}`}
                      key={`${mode}-${week.weekStartDate}-${row}`}
                      x={week.column * CELL_STEP}
                      y={row * CELL_STEP}
                      width={CELL_SIZE}
                      height={CELL_SIZE}
                      rx={2}
                      style={{
                        animationDelay: `${week.column * 9 + row * 5}ms`,
                      }}
                    />
                  );
                }),
              )
            : heatmap.cells.map((cell) => (
                <rect
                  className={`settings-usage-cell level-${cell.level}`}
                  key={cell.date}
                  x={cell.column * CELL_STEP}
                  y={cell.row * CELL_STEP}
                  width={CELL_SIZE}
                  height={CELL_SIZE}
                  rx={2}
                  style={{
                    animationDelay: `${cell.column * 9 + cell.row * 5}ms`,
                  }}
                  onMouseEnter={(event) => showTooltip(event, cell)}
                  onMouseLeave={() => setTooltip(undefined)}
                />
              ))}
          {mode !== "daily" &&
            heatmap.weeks.map((week) => (
              <rect
                className="settings-usage-week-hitbox"
                key={`hitbox-${week.weekStartDate}`}
                x={week.column * CELL_STEP - 1}
                y={-1}
                width={CELL_SIZE + 2}
                height={GRID_HEIGHT + 2}
                onMouseEnter={(event) => showWeekColumnTooltip(event, week)}
                onMouseLeave={() => {
                  setHoveredWeek(undefined);
                  setTooltip(undefined);
                }}
              />
            ))}
          {heatmap.monthLabels.map((month) => (
            <text
              className="settings-usage-month"
              key={`${month.column}-${month.label}`}
              x={month.column * CELL_STEP}
              y={GRID_HEIGHT + LABEL_HEIGHT - 3}
            >
              {month.label}
            </text>
          ))}
        </svg>
      </div>

      {busy && statistics && (
        <span className="settings-usage-refreshing" role="status">
          <RefreshCw className="spinning" size={12} />
          {t("settings.usageRefreshing")}
        </span>
      )}

      {tooltip &&
        createPortal(
          <div
            className="settings-usage-tooltip"
            role="tooltip"
            style={{ left: tooltip.left, top: tooltip.top }}
          >
            {tooltip.text}
          </div>,
          document.body,
        )}
    </section>
  );
}
