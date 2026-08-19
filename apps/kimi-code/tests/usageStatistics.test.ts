import assert from "node:assert/strict";
import test from "node:test";

import {
  buildHeatmap,
  cacheHitRate,
  formatHeatmapDate,
  formatHeatmapTokenCount,
  formatTaskDuration,
  formatTokenCount,
  HEATMAP_WEEKS,
  heatmapTooltipDatum,
  summarizeTokenUsage,
  usageStatisticsForModels,
} from "../src/usageStatistics.ts";

const today = new Date(2026, 7, 11, 12, 0, 0);
const days = [
  { date: "2025-01-01", totalTokens: 40 },
  { date: "2026-08-09", totalTokens: 10 },
  { date: "2026-08-10", totalTokens: 20 },
  { date: "2026-08-11", totalTokens: 30 },
];

test("builds a Monday-aligned 53-week daily heatmap", () => {
  const heatmap = buildHeatmap(days, "daily", today, "en-US");
  assert.equal(heatmap.cells.at(-1)?.date, "2026-08-11");
  assert.equal(heatmap.cells.at(-1)?.column, HEATMAP_WEEKS - 1);
  assert.equal(heatmap.cells.at(-1)?.row, 1);
  assert.equal(
    heatmap.cells.find((cell) => cell.date === "2026-08-10")?.intensityValue,
    20,
  );
  assert.equal(heatmap.maxIntensity, 30);
  assert.ok(heatmap.monthLabels.length >= 12);
});

test("weekly mode uses one total and tooltip period for the whole column", () => {
  const heatmap = buildHeatmap(days, "weekly", today, "en-US");
  const monday = heatmap.cells.find((cell) => cell.date === "2026-08-10");
  const tuesday = heatmap.cells.find((cell) => cell.date === "2026-08-11");
  assert.equal(monday?.intensityValue, 50);
  assert.equal(tuesday?.intensityValue, 50);
  assert.equal(monday?.dayTokens, 20);
  assert.equal(tuesday?.dayTokens, 30);
  assert.deepEqual(heatmapTooltipDatum(monday!, "weekly"), {
    date: "2026-08-10",
    tokens: 50,
    usage: {
      inputUncachedTokens: 50,
      inputCachedTokens: 0,
      outputTokens: 0,
    },
  });
  assert.deepEqual(heatmapTooltipDatum(tuesday!, "weekly"), {
    date: "2026-08-10",
    tokens: 50,
    usage: {
      inputUncachedTokens: 50,
      inputCachedTokens: 0,
      outputTokens: 0,
    },
  });
  const currentWeek = heatmap.weeks.at(-1);
  assert.equal(currentWeek?.weekStartDate, "2026-08-10");
  assert.equal(currentWeek?.weekEndDate, "2026-08-16");
  assert.equal(currentWeek?.totalTokens, 50);
  assert.equal(currentWeek?.filledCells, 7);
  assert.equal(currentWeek?.level, 4);
});

test("weekly columns use bottom-aligned discrete heights with one cell minimum", () => {
  const heatmap = buildHeatmap(
    [
      { date: "2026-08-03", totalTokens: 1 },
      { date: "2026-08-10", totalTokens: 100 },
    ],
    "weekly",
    today,
    "en-US",
  );
  const lowWeek = heatmap.weeks.at(-2);
  const highWeek = heatmap.weeks.at(-1);
  assert.equal(lowWeek?.totalTokens, 1);
  assert.equal(lowWeek?.filledCells, 1);
  assert.equal(highWeek?.totalTokens, 100);
  assert.equal(highWeek?.filledCells, 7);
});

test("cumulative mode includes usage before the visible grid", () => {
  const heatmap = buildHeatmap(days, "cumulative", today, "en-US");
  assert.equal(
    heatmap.cells.find((cell) => cell.date === "2026-08-09")?.intensityValue,
    50,
  );
  assert.equal(
    heatmap.cells.find((cell) => cell.date === "2026-08-11")?.intensityValue,
    100,
  );
  assert.equal(heatmap.maxIntensity, 100);
  assert.equal(
    heatmap.cells.some((cell) => cell.date === "2026-08-12"),
    false,
  );
  const currentWeek = heatmap.weeks.at(-1);
  assert.equal(currentWeek?.weekEndDate, "2026-08-16");
  assert.equal(currentWeek?.cumulativeTokens, 100);
  assert.equal(currentWeek?.cumulativeFilledCells, 7);
  assert.equal(currentWeek?.cumulativeLevel, 4);
  assert.equal(heatmap.weeks[0]?.cumulativeTokens, 40);
  assert.equal(heatmap.weeks[0]?.cumulativeFilledCells, 3);
});

test("preserves input cache and output breakdown across heatmap periods", () => {
  const heatmap = buildHeatmap(
    [
      {
        date: "2026-08-10",
        totalTokens: 11,
        inputUncachedTokens: 2,
        inputCachedTokens: 8,
        outputTokens: 1,
      },
      {
        date: "2026-08-11",
        totalTokens: 12,
        inputUncachedTokens: 3,
        inputCachedTokens: 7,
        outputTokens: 2,
      },
    ],
    "weekly",
    today,
    "en-US",
  );
  assert.deepEqual(heatmap.weeks.at(-1)?.usage, {
    inputUncachedTokens: 5,
    inputCachedTokens: 15,
    outputTokens: 3,
  });
  assert.equal(cacheHitRate(heatmap.weeks.at(-1)!.usage), 0.75);
  assert.deepEqual(heatmapTooltipDatum(heatmap.cells.at(-1)!, "weekly").usage, {
    inputUncachedTokens: 5,
    inputCachedTokens: 15,
    outputTokens: 3,
  });
});

test("formats compact tokens and localized task durations", () => {
  assert.equal(formatTokenCount(0, "zh-CN"), "0");
  assert.match(formatTokenCount(123_000_000, "zh-CN"), /1\.2亿|1\.23亿/);
  assert.equal(formatTaskDuration(3 * 60 * 60_000 + 3 * 60_000, "zh-CN"), "3 小时 3 分");
  assert.equal(formatTaskDuration(3 * 60 * 60_000, "en-US"), "3h");
});

test("formats heatmap tokens with Chinese and English unit conventions", () => {
  assert.equal(formatHeatmapTokenCount(9_999, "zh-CN"), "9,999");
  assert.equal(formatHeatmapTokenCount(12_345, "zh-CN"), "1.2万");
  assert.equal(formatHeatmapTokenCount(123_456_789, "zh-CN"), "1.2亿");
  assert.equal(formatHeatmapTokenCount(999, "en-US"), "999");
  assert.equal(formatHeatmapTokenCount(1_234, "en-US"), "1.2K");
  assert.equal(formatHeatmapTokenCount(1_234_567, "en-US"), "1.2M");
  assert.equal(formatHeatmapDate("2026-07-26", "zh-CN"), "2026年7月26日");
  assert.equal(formatHeatmapDate("2026-07-26", "en-US"), "July 26, 2026");
});

test("combines any selected models into one usage series", () => {
  const statistics = {
    totalTokens: 65,
    peakDailyTokens: 35,
    longestTaskMs: 120_000,
    currentStreakDays: 3,
    longestStreakDays: 3,
    days: [
      { date: "2026-08-09", totalTokens: 10 },
      { date: "2026-08-10", totalTokens: 20 },
      { date: "2026-08-11", totalTokens: 35 },
    ],
    byModel: {
      a: {
        totalTokens: 35,
        peakDailyTokens: 30,
        days: [
          {
            date: "2026-08-09",
            totalTokens: 5,
            inputUncachedTokens: 1,
            inputCachedTokens: 3,
            outputTokens: 1,
          },
          {
            date: "2026-08-11",
            totalTokens: 30,
            inputUncachedTokens: 5,
            inputCachedTokens: 20,
            outputTokens: 5,
          },
        ],
      },
      b: {
        totalTokens: 30,
        peakDailyTokens: 20,
        days: [
          {
            date: "2026-08-09",
            totalTokens: 5,
            inputUncachedTokens: 2,
            inputCachedTokens: 2,
            outputTokens: 1,
          },
          {
            date: "2026-08-10",
            totalTokens: 20,
            inputUncachedTokens: 4,
            inputCachedTokens: 12,
            outputTokens: 4,
          },
          {
            date: "2026-08-11",
            totalTokens: 5,
            inputUncachedTokens: 1,
            inputCachedTokens: 3,
            outputTokens: 1,
          },
        ],
      },
    },
  };

  const oneModel = usageStatisticsForModels(statistics, ["a"], today);
  assert.equal(oneModel.totalTokens, 35);
  assert.equal(oneModel.peakDailyTokens, 30);
  assert.equal(oneModel.currentStreakDays, 1);
  assert.equal(oneModel.longestStreakDays, 1);
  assert.equal(oneModel.longestTaskMs, 120_000);
  const oneModelUsage = summarizeTokenUsage(oneModel.days);
  assert.deepEqual(oneModelUsage, {
    inputUncachedTokens: 6,
    inputCachedTokens: 23,
    outputTokens: 6,
  });
  assert.equal(cacheHitRate(oneModelUsage), 23 / 29);

  const allModels = usageStatisticsForModels(statistics, ["a", "b"], today);
  assert.equal(allModels, statistics);
});
