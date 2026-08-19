import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, RefreshCw } from "lucide-react";

import { t, type Language } from "./i18n";
import {
  buildHeatmap,
  formatHeatmapDate,
  formatHeatmapTokenCount,
  formatTaskDuration,
  formatTokenCount,
  formatUsageCacheHitRate,
  heatmapTooltipDatum,
  inputTokenCount,
  summarizeTokenUsage,
  usageStatisticsForModels,
  type HeatmapCell,
  type HeatmapMode,
  type TokenUsageBreakdown,
  type UsageStatistics,
  type WeeklyHeatmapColumn,
} from "./usageStatistics";

const CELL_SIZE = 11;
const CELL_GAP = 3;
const CELL_STEP = CELL_SIZE + CELL_GAP;
const GRID_WIDTH = 53 * CELL_STEP - CELL_GAP;
const GRID_HEIGHT = 7 * CELL_STEP - CELL_GAP;
const LABEL_HEIGHT = 18;
const UNKNOWN_MODEL = "__unknown__";

interface TooltipState {
  title: string;
  usage: TokenUsageBreakdown;
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
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [modelSelection, setModelSelection] = useState<string[]>();
  const modelMenuRef = useRef<HTMLDivElement>(null);
  const locale = language === "zh" ? "zh-CN" : "en-US";
  const availableModels = useMemo(
    () => Object.keys(statistics?.byModel ?? {}).sort((left, right) => left.localeCompare(right)),
    [statistics?.byModel],
  );
  const selectedModels = useMemo(() => {
    if (!modelSelection) return availableModels;
    const available = new Set(availableModels);
    const valid = modelSelection.filter((model) => available.has(model));
    return valid.length ? valid : availableModels;
  }, [availableModels, modelSelection]);
  const filteredStatistics = useMemo(
    () => statistics && usageStatisticsForModels(statistics, selectedModels),
    [selectedModels, statistics],
  );
  const heatmap = useMemo(
    () => buildHeatmap(filteredStatistics?.days ?? [], mode, new Date(), locale),
    [filteredStatistics?.days, locale, mode],
  );

  useEffect(() => {
    if (!modelMenuOpen) return;
    const closeMenu = (event: PointerEvent): void => {
      if (!modelMenuRef.current?.contains(event.target as Node)) {
        setModelMenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (event.key === "Escape") setModelMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeMenu);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeMenu);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [modelMenuOpen]);

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
    byModel: {},
  };
  const displayedData = filteredStatistics ?? data;
  const usageTotals = summarizeTokenUsage(displayedData.days);
  const metrics = [
    {
      value: formatTokenCount(inputTokenCount(usageTotals), locale),
      label: t("settings.usageInputTokens"),
    },
    {
      value: formatTokenCount(usageTotals.outputTokens, locale),
      label: t("settings.usageOutputTokens"),
    },
    {
      value: formatUsageCacheHitRate(usageTotals, locale),
      label: t("settings.usageCacheHitRate"),
    },
    {
      value: formatTaskDuration(displayedData.longestTaskMs, locale),
      label: t("settings.usageLongestTask"),
    },
    {
      value: t("settings.usageDaysValue", { count: displayedData.currentStreakDays }),
      label: t("settings.usageCurrentStreak"),
    },
    {
      value: t("settings.usageDaysValue", { count: displayedData.longestStreakDays }),
      label: t("settings.usageLongestStreak"),
    },
  ];
  const allModelsSelected = selectedModels.length === availableModels.length;
  const modelLabel = (model: string): string =>
    model === UNKNOWN_MODEL ? t("settings.usageUnknownModel") : model;
  const selectionLabel = allModelsSelected
    ? t("settings.usageAllModels")
    : selectedModels.length === 1
      ? modelLabel(selectedModels[0])
      : t("settings.usageSelectedModels", { count: selectedModels.length });

  const selectOnlyModel = (model: string): void => {
    setModelSelection([model]);
    setModelMenuOpen(false);
    setHoveredWeek(undefined);
    setTooltip(undefined);
  };

  const toggleModel = (model: string): void => {
    const next = selectedModels.includes(model)
      ? selectedModels.filter((selected) => selected !== model)
      : [...selectedModels, model];
    if (!next.length) return;
    setModelSelection(next.length === availableModels.length ? undefined : next);
    setHoveredWeek(undefined);
    setTooltip(undefined);
  };

  const showTooltip = (
    event: MouseEvent<SVGRectElement>,
    cell: HeatmapCell,
  ): void => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const datum = heatmapTooltipDatum(cell, mode);
    setTooltip({
      title:
        mode === "weekly"
          ? t("settings.usageTooltipWeek", {
              date: formatHeatmapDate(datum.date, locale),
            })
          : formatHeatmapDate(datum.date, locale),
      usage: datum.usage,
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
    setHoveredWeek(week.column);
    setTooltip({
      title: cumulativeMode
        ? t("settings.usageTooltipCumulative", {
            date: formatHeatmapDate(week.weekEndDate, locale),
          })
        : t("settings.usageTooltipWeek", {
            date: formatHeatmapDate(week.weekStartDate, locale),
          }),
      usage: cumulativeMode ? week.cumulativeUsage : week.usage,
      left: bounds.left + bounds.width / 2,
      top: bounds.top,
    });
  };

  return (
    <section className="settings-usage" aria-labelledby="usage-heatmap-heading">
      {availableModels.length > 0 && (
        <div className="settings-usage-filter">
          <span>{t("settings.usageModelFilter")}</span>
          <div className="settings-usage-model-picker" ref={modelMenuRef}>
            <button
              className="settings-usage-model-trigger"
              type="button"
              aria-haspopup="menu"
              aria-expanded={modelMenuOpen}
              onClick={() => setModelMenuOpen((open) => !open)}
            >
              <span title={selectionLabel}>{selectionLabel}</span>
              <ChevronDown size={14} aria-hidden="true" />
            </button>
            {modelMenuOpen && (
              <div className="settings-usage-model-menu" role="menu">
                <button
                  className="settings-usage-model-option all"
                  type="button"
                  role="menuitemcheckbox"
                  aria-checked={allModelsSelected}
                  onClick={() => {
                    setModelSelection(undefined);
                    setHoveredWeek(undefined);
                    setTooltip(undefined);
                  }}
                >
                  <span className="settings-usage-model-check">
                    {allModelsSelected && <Check size={12} aria-hidden="true" />}
                  </span>
                  <span>{t("settings.usageAllModels")}</span>
                  <small>{availableModels.length}</small>
                </button>
                <div className="settings-usage-model-divider" />
                {availableModels.map((model) => (
                  <div className="settings-usage-model-row" key={model}>
                    <button
                      className="settings-usage-model-option"
                      type="button"
                      role="menuitemcheckbox"
                      aria-checked={selectedModels.includes(model)}
                      onClick={() => toggleModel(model)}
                    >
                      <span className="settings-usage-model-check">
                        {selectedModels.includes(model) && (
                          <Check size={12} aria-hidden="true" />
                        )}
                      </span>
                      <span title={modelLabel(model)}>{modelLabel(model)}</span>
                    </button>
                    <button
                      className="settings-usage-model-only"
                      type="button"
                      onClick={() => selectOnlyModel(model)}
                    >
                      {t("settings.usageOnlyModel")}
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

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
            <strong>{tooltip.title}</strong>
            <div className="settings-usage-tooltip-row input">
              <span>{t("settings.usageInputTokens")}</span>
              <b>
                {formatHeatmapTokenCount(inputTokenCount(tooltip.usage), locale)}
              </b>
              <small>
                {t("settings.usageInputBreakdown", {
                  uncached: formatHeatmapTokenCount(
                    tooltip.usage.inputUncachedTokens,
                    locale,
                  ),
                  cached: formatHeatmapTokenCount(
                    tooltip.usage.inputCachedTokens,
                    locale,
                  ),
                })}
              </small>
            </div>
            <div className="settings-usage-tooltip-row">
              <span>{t("settings.usageOutputTokens")}</span>
              <b>
                {formatHeatmapTokenCount(tooltip.usage.outputTokens, locale)}
              </b>
            </div>
            <div className="settings-usage-tooltip-row">
              <span>{t("settings.usageCacheHitRate")}</span>
              <b>{formatUsageCacheHitRate(tooltip.usage, locale)}</b>
            </div>
          </div>,
          document.body,
        )}
    </section>
  );
}
